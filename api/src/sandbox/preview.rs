//! Serves `https://<slug>.<preview domain>` from the workspace's dev server. Each
//! workspace gets its own origin, so app code never runs on Chat's origin, never
//! sees Chat's `__Host-` session cookie, and cannot read another workspace. Access
//! needs a host-only cookie minted from a one-minute ticket that Chat issues to the
//! workspace owner. Only Chat's own page can redeem a ticket (a POST that the
//! browser marks as coming from Chat's origin), so a ticket sent to someone else
//! opens nothing for them. Ticket and cookie carry the Chat sign-in they came from,
//! so signing out, revoking that session or disabling the account ends preview
//! access within seconds, and closes open WebSockets within a minute.
use super::Sandboxes;
use crate::error::ApiError;
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, FromRequestParts, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};
use uuid::Uuid;

pub const ENTER: &str = "enter";
const VIEW: &str = "view";
const COOKIE: &str = "obleth_preview";
/// The cookie's name on https, where the prefix stops sibling hosts shadowing it.
const HOST_COOKIE: &str = "__Host-obleth_preview";
const VIEW_SECONDS: u64 = 3600;
/// A page load renews a cookie with less than this left, so an open preview keeps
/// working while the sign-in it came from lasts.
const RENEW_SECONDS: u64 = 1800;
/// How long a confirmed sign-in is trusted for proxied requests, so a page's many
/// assets do not each check the session.
const SIGN_IN_CHECK: Duration = Duration::from_secs(5);
/// How often an open WebSocket checks that its sign-in is still live.
const SOCKET_CHECK: Duration = Duration::from_secs(60);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Who a ticket or view cookie was issued to, and the Chat sign-in it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Grant {
    pub user: Uuid,
    pub session: Uuid,
}

pub fn sign(key: &[u8], purpose: &str, slug: &str, grant: Grant, seconds: u64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(
        json!({"p":purpose,"s":slug,"u":grant.user,"a":grant.session,"e":now() + seconds})
            .to_string(),
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    format!(
        "{payload}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

/// Returns who a token was issued to when it is authentic, unexpired, and for this
/// purpose and workspace.
pub fn verify(key: &[u8], token: &str, purpose: &str, slug: &str) -> Option<Grant> {
    verify_until(key, token, purpose, slug).map(|(grant, _)| grant)
}

/// `verify`, with when the token expires (seconds since the epoch).
fn verify_until(key: &[u8], token: &str, purpose: &str, slug: &str) -> Option<(Grant, u64)> {
    let (payload, signature) = token.split_once('.')?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&URL_SAFE_NO_PAD.decode(signature).ok()?)
        .ok()?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    let expires = claims["e"].as_u64()?;
    if claims["p"] != purpose || claims["s"] != slug || expires <= now() {
        return None;
    }
    Some((
        Grant {
            user: claims["u"].as_str()?.parse().ok()?,
            session: claims["a"].as_str()?.parse().ok()?,
        },
        expires,
    ))
}

/// Whether the sign-in behind a grant is still live and may still use workspaces.
async fn still_signed_in(sb: &Sandboxes, grant: Grant) -> Result<bool, ApiError> {
    if let Some(grants) = sb.inner.grants.get() {
        if !grants.confirm(grant.session).await? {
            return Ok(false);
        }
    }
    let owner = crate::auth::session_owner(&sb.inner.pool, grant.session).await?;
    Ok(owner.is_some_and(|who| who.id == grant.user && sb.allowed(&who)))
}

/// `still_signed_in`, trusting a confirmation for `SIGN_IN_CHECK`.
async fn signed_in(sb: &Sandboxes, grant: Grant) -> Result<bool, ApiError> {
    let fresh = sb
        .inner
        .sign_ins
        .lock()
        .unwrap()
        .get(&grant)
        .is_some_and(|at| at.elapsed() < SIGN_IN_CHECK);
    if fresh {
        return Ok(true);
    }
    let live = still_signed_in(sb, grant).await?;
    let mut sign_ins = sb.inner.sign_ins.lock().unwrap();
    sign_ins.retain(|_, at| at.elapsed() < SIGN_IN_CHECK);
    if live {
        sign_ins.insert(grant, Instant::now());
    }
    Ok(live)
}

fn cookie_name(sb: &Sandboxes) -> &'static str {
    if sb.config().preview_scheme == "https" {
        HOST_COOKIE
    } else {
        COOKIE
    }
}

/// The workspace slug when the request is addressed to a preview host.
fn preview_slug(headers: &HeaderMap, domain: &str) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let host = host.rsplit_once(':').map_or(host, |(name, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            name
        } else {
            host
        }
    });
    let slug = host
        .to_ascii_lowercase()
        .strip_suffix(&format!(".{domain}"))?
        .to_owned();
    (slug.len() == 16
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
    .then_some(slug)
}

/// Outermost middleware: preview hosts never reach Chat's own routes.
pub async fn dispatch(
    State(sandboxes): State<Option<Sandboxes>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(sb) = sandboxes else {
        return next.run(request).await;
    };
    let Some(domain) = sb.config().preview_domain.clone() else {
        return next.run(request).await;
    };
    let Some(slug) = preview_slug(request.headers(), &domain) else {
        if request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.to_ascii_lowercase().contains(&domain))
        {
            return page(
                StatusCode::NOT_FOUND,
                "This preview address does not exist.",
                &sb,
            );
        }
        return next.run(request).await;
    };
    match serve(&sb, &slug, request).await {
        Ok(response) => response,
        Err(error) => page(error.0, &error.1, &sb),
    }
}

fn page(status: StatusCode, message: &str, sb: &Sandboxes) -> Response {
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Preview</title>\
         <style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#09090b;color:#a1a1aa;font:14px system-ui,sans-serif}}p{{max-width:28rem;text-align:center;padding:0 16px}}</style><p>{}</p>",
        message.replace('&', "&amp;").replace('<', "&lt;")
    );
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response();
    frame_headers(response.headers_mut(), sb);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn frame_headers(headers: &mut HeaderMap, sb: &Sandboxes) {
    if let Ok(value) =
        HeaderValue::from_str(&format!("frame-ancestors {}", sb.config().chat_origin))
    {
        headers.append(header::CONTENT_SECURITY_POLICY, value);
    }
    headers.remove(header::X_FRAME_OPTIONS);
    // Same-origin requests keep their Origin, which `cross_origin` checks.
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
}

/// Whether a request that may change something comes from a page other than the
/// preview itself, such as another workspace's preview under the same domain,
/// which browsers treat as the same site and send the preview cookie from.
fn cross_origin(headers: &HeaderMap, origin: &str) -> bool {
    let value = |name| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(site) = value("sec-fetch-site") {
        return site != "same-origin";
    }
    value(header::ORIGIN.as_str()).is_some_and(|sent| sent != origin)
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
}

/// Cookie header for the dev server, without Chat's preview cookie.
fn upstream_cookies(headers: &HeaderMap) -> Option<String> {
    let rest = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|pair| {
            let name = pair.split_once('=').map_or(*pair, |(name, _)| name);
            !pair.is_empty() && name != COOKIE && name != HOST_COOKIE
        })
        .collect::<Vec<_>>()
        .join("; ");
    (!rest.is_empty()).then_some(rest)
}

/// A dev server's Set-Cookie made host-only, so it never reaches other hosts under
/// the preview domain. Cookies named like Chat's own are dropped.
fn app_cookie(value: &str) -> Option<String> {
    let mut parts = value.split(';').map(str::trim);
    let pair = parts.next()?;
    let name = pair
        .split_once('=')
        .map_or(pair, |(name, _)| name)
        .trim()
        .to_ascii_lowercase();
    if name == COOKIE || name.starts_with("__host-obleth") || name.starts_with("__secure-obleth") {
        return None;
    }
    let attributes = parts.filter(|attribute| {
        let key = attribute.split_once('=').map_or(*attribute, |(key, _)| key);
        !attribute.is_empty() && !key.trim().eq_ignore_ascii_case("domain")
    });
    Some(
        std::iter::once(pair)
            .chain(attributes)
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// A page inside the preview to land on: a path in the project, never another site.
pub fn landing(path: &str) -> Option<String> {
    let path = path.trim();
    let path = path
        .strip_prefix("/workspace/")
        .map_or(path.to_owned(), |rest| match rest.split_once('/') {
            Some((_, inside)) => format!("/{inside}"),
            None => "/".into(),
        });
    let path = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    let safe = path.len() <= 1024
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.split('/').any(|part| part == "..")
        && !path.chars().any(char::is_control);
    (safe && path != "/").then_some(path)
}

/// Percent-encodes a path for a URL, keeping its slashes.
pub fn encode_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The preview cookie for a grant, valid for `VIEW_SECONDS`.
fn view_cookie(sb: &Sandboxes, key: &[u8], slug: &str, grant: Grant) -> Option<HeaderValue> {
    let secure = if sb.config().preview_scheme == "https" {
        "; Secure"
    } else {
        ""
    };
    let value = sign(key, VIEW, slug, grant, VIEW_SECONDS);
    let name = cookie_name(sb);
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={VIEW_SECONDS}{secure}"
    ))
    .ok()
}

/// Redeems a ticket for the preview cookie. Chat's page sends the ticket with a
/// credentialed `fetch`, which always carries Chat's origin; a link or form on any
/// other page cannot, so a ticket only ever opens the preview for its owner's browser.
async fn enter(sb: &Sandboxes, slug: &str, key: &[u8], request: Request) -> Response {
    let chat = sb.config().chat_origin.clone();
    let from_chat = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        == Some(chat.as_str());
    if request.method() != Method::POST || !from_chat {
        return page(
            StatusCode::FORBIDDEN,
            "Open this preview from its conversation in Chat.",
            sb,
        );
    }
    let answer = |status: StatusCode, body: Value, cookie: Option<HeaderValue>| {
        let mut response = (status, axum::Json(body)).into_response();
        let headers = response.headers_mut();
        if let Ok(origin) = HeaderValue::from_str(&chat) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        }
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        headers.insert(header::VARY, HeaderValue::from_static("origin"));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Some(cookie) = cookie {
            headers.insert(header::SET_COOKIE, cookie);
        }
        response
    };
    let expired = || {
        answer(
            StatusCode::FORBIDDEN,
            json!({"error":{"message":"This preview link has expired. Reopen the preview."}}),
            None,
        )
    };
    let Ok(body) = axum::body::to_bytes(request.into_body(), 8 * 1024).await else {
        return expired();
    };
    let ticket = url::form_urlencoded::parse(&body)
        .find(|(name, _)| name == "ticket")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    let Some(grant) = verify(key, &ticket, ENTER, slug) else {
        return expired();
    };
    match still_signed_in(sb, grant).await {
        Ok(true) => answer(
            StatusCode::OK,
            json!({"ok": true}),
            view_cookie(sb, key, slug, grant),
        ),
        Ok(false) => expired(),
        Err(error) => answer(error.0, json!({"error":{"message":error.1}}), None),
    }
}

/// Whether a request should renew a view cookie expiring at `expires`: a page load,
/// as opposed to an asset or an API call, once less than `RENEW_SECONDS` is left.
fn renews(method: &Method, headers: &HeaderMap, expires: u64) -> bool {
    let value = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let page = match value("sec-fetch-dest") {
        Some(dest) => dest == "document" || dest == "iframe",
        None => value(header::ACCEPT.as_str()).is_some_and(|a| a.contains("text/html")),
    };
    *method == Method::GET && page && expires.saturating_sub(now()) < RENEW_SECONDS
}

async fn serve(sb: &Sandboxes, slug: &str, request: Request) -> Result<Response, ApiError> {
    let key = sb.key().await?.to_vec();
    if request.uri().path() == "/__obleth/enter" {
        return Ok(enter(sb, slug, &key, request).await);
    }
    let signed_out = || {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Open this preview from its conversation in Chat.".into(),
        )
    };
    let (grant, expires) = cookie(request.headers(), cookie_name(sb))
        .and_then(|token| verify_until(&key, &token, VIEW, slug))
        .ok_or_else(signed_out)?;
    if !signed_in(sb, grant).await? {
        return Err(signed_out());
    }
    let sandbox = sb
        .by_slug(slug)
        .await?
        .filter(|s| s.owner_id == grant.user)
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                "This workspace no longer exists.".into(),
            )
        })?;
    let not_running = || {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "The preview is not running. Ask the assistant to start it.".into(),
        )
    };
    let (Some(ip), Some(port), "running") = (
        sandbox.pod_ip.clone(),
        sandbox.preview_port,
        sandbox.status.as_str(),
    ) else {
        return Err(not_running());
    };
    // The address on record may since have gone to another pod.
    if !sb.confirm_pod(&sandbox).await? {
        return Err(not_running());
    }
    let origin = sb.preview_origin(&sandbox).unwrap_or_default();
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if !safe && cross_origin(request.headers(), &origin) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the preview itself can send this request.".into(),
        ));
    }
    sb.touch(&sandbox).await;
    let target = format!("{ip}:{port}");
    let is_upgrade = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if is_upgrade {
        return websocket(sb, &sandbox, &target, request, grant, expires).await;
    }
    let renew = renews(request.method(), request.headers(), expires)
        .then(|| view_cookie(sb, &key, slug, grant))
        .flatten();
    let mut response = proxy(sb, &target, request).await?;
    if let Some(cookie) = renew {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(response)
}

const HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
];

async fn proxy(sb: &Sandboxes, target: &str, request: Request) -> Result<Response, ApiError> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let url = format!("http://{target}{path}");
    let mut upstream = sb.http().request(parts.method.clone(), url);
    for (name, value) in &parts.headers {
        if HOP.contains(&name.as_str()) || name == header::COOKIE {
            continue;
        }
        upstream = upstream.header(name, value);
    }
    if let Some(cookies) = upstream_cookies(&parts.headers) {
        upstream = upstream.header(header::COOKIE, cookies);
    }
    if let Some(host) = parts.headers.get(header::HOST) {
        upstream = upstream.header("x-forwarded-host", host);
    }
    upstream = upstream.header("x-forwarded-proto", sb.config().preview_scheme.as_str());
    let response = upstream
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::BAD_GATEWAY,
                "The preview server did not answer. Check the server logs in the conversation."
                    .into(),
            )
        })?;
    let mut builder = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if HOP.contains(&name.as_str()) {
            continue;
        }
        if name == header::SET_COOKIE {
            if let Some(value) = value
                .to_str()
                .ok()
                .and_then(app_cookie)
                .and_then(|v| HeaderValue::from_str(&v).ok())
            {
                builder = builder.header(name, value);
            }
            continue;
        }
        builder = builder.header(name, value);
    }
    let mut out = builder
        .body(Body::from_stream(response.bytes_stream()))
        .map_err(|_| ApiError::internal())?;
    frame_headers(out.headers_mut(), sb);
    Ok(out)
}

/// Relays a WebSocket to the dev server until either side closes it, the sign-in it
/// came from ends, or the preview cookie it was opened with expires.
async fn websocket(
    sb: &Sandboxes,
    sandbox: &super::Sandbox,
    target: &str,
    request: Request,
    grant: Grant,
    expires: u64,
) -> Result<Response, ApiError> {
    // Only the preview page itself may open sockets to its dev server.
    let origin = sb.preview_origin(sandbox).unwrap_or_default();
    if request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        != Some(origin.as_str())
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Cross-origin WebSocket refused".into(),
        ));
    }
    let (mut parts, _) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "/".into());
    let protocols: Vec<String> = parts
        .headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut upstream = format!("ws://{target}{path}")
        .into_client_request()
        .map_err(|_| ApiError::bad("Invalid WebSocket address"))?;
    {
        let headers = upstream.headers_mut();
        if !protocols.is_empty() {
            if let Ok(value) = HeaderValue::from_str(&protocols.join(", ")) {
                headers.insert(header::SEC_WEBSOCKET_PROTOCOL, value);
            }
        }
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_str(&origin).map_err(|_| ApiError::internal())?,
        );
        if let Some(cookies) =
            upstream_cookies(&parts.headers).and_then(|c| HeaderValue::from_str(&c).ok())
        {
            headers.insert(header::COOKIE, cookies);
        }
    }
    let (server, handshake) = tokio_tungstenite::connect_async(upstream)
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::BAD_GATEWAY,
                "The preview server refused the WebSocket".into(),
            )
        })?;
    let chosen = handshake
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| ApiError::bad("Invalid WebSocket request"))?;
    let upgrade = match chosen {
        Some(protocol) => upgrade.protocols([protocol]),
        None => upgrade,
    };
    let sb_watch = sb.clone();
    let mut response = upgrade.on_upgrade(move |client| async move {
        let (mut client_tx, mut client_rx) = client.split();
        let (mut server_tx, mut server_rx) = server.split();
        let to_server = async {
            while let Some(Ok(message)) = client_rx.next().await {
                let message = match message {
                    axum::extract::ws::Message::Text(t) => {
                        tungstenite::Message::Text(t.as_str().into())
                    }
                    axum::extract::ws::Message::Binary(b) => tungstenite::Message::Binary(b),
                    axum::extract::ws::Message::Ping(p) => tungstenite::Message::Ping(p),
                    axum::extract::ws::Message::Pong(p) => tungstenite::Message::Pong(p),
                    axum::extract::ws::Message::Close(_) => break,
                };
                if server_tx.send(message).await.is_err() {
                    break;
                }
            }
        };
        let to_client = async {
            while let Some(Ok(message)) = server_rx.next().await {
                let message = match message {
                    tungstenite::Message::Text(t) => {
                        axum::extract::ws::Message::Text(t.as_str().into())
                    }
                    tungstenite::Message::Binary(b) => axum::extract::ws::Message::Binary(b),
                    tungstenite::Message::Ping(p) => axum::extract::ws::Message::Ping(p),
                    tungstenite::Message::Pong(p) => axum::extract::ws::Message::Pong(p),
                    tungstenite::Message::Close(_) | tungstenite::Message::Frame(_) => break,
                };
                if client_tx.send(message).await.is_err() {
                    break;
                }
            }
        };
        let watch = async {
            let lifetime = Duration::from_secs(expires.saturating_sub(now()));
            let deadline = tokio::time::Instant::now() + lifetime;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    _ = tokio::time::sleep(SOCKET_CHECK) => {}
                }
                // A failed check is not a sign-out; the next one decides.
                if let Ok(false) = still_signed_in(&sb_watch, grant).await {
                    break;
                }
            }
        };
        tokio::select! { _ = to_server => {}, _ = to_client => {}, _ = watch => {} }
    });
    frame_headers(response.headers_mut(), sb);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_pages_stay_inside_the_preview() {
        assert_eq!(landing("about.html").as_deref(), Some("/about.html"));
        assert_eq!(landing("/docs/a b.html").as_deref(), Some("/docs/a b.html"));
        assert_eq!(
            landing("/workspace/site-1/pages/x.html").as_deref(),
            Some("/pages/x.html")
        );
        for bad in ["", "/", "//evil.example/x", "/a/../b", "/a\\b", "/a\nb"] {
            assert_eq!(landing(bad), None, "{bad:?}");
        }
        assert_eq!(encode_path("/docs/a b.html"), "/docs/a%20b.html");
    }

    #[test]
    fn tokens_are_bound_to_purpose_workspace_and_key() {
        let key = [7u8; 32];
        let user = Grant {
            user: Uuid::new_v4(),
            session: Uuid::new_v4(),
        };
        let token = sign(&key, ENTER, "abcdefghijklmnop", user, 60);
        assert_eq!(verify(&key, &token, ENTER, "abcdefghijklmnop"), Some(user));
        assert_eq!(verify(&key, &token, VIEW, "abcdefghijklmnop"), None);
        assert_eq!(verify(&key, &token, ENTER, "bbcdefghijklmnop"), None);
        assert_eq!(verify(&[8u8; 32], &token, ENTER, "abcdefghijklmnop"), None);
        let (payload, _) = token.split_once('.').unwrap();
        assert_eq!(
            verify(&key, &format!("{payload}.AAAA"), ENTER, "abcdefghijklmnop"),
            None
        );
        let expired = sign(&key, ENTER, "abcdefghijklmnop", user, 0);
        assert_eq!(verify(&key, &expired, ENTER, "abcdefghijklmnop"), None);
        // Tokens from before sign-ins were bound carry no session.
        let payload = URL_SAFE_NO_PAD.encode(
            json!({"p":ENTER,"s":"abcdefghijklmnop","u":user.user,"e":now() + 60}).to_string(),
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
        mac.update(payload.as_bytes());
        let unbound = format!(
            "{payload}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        );
        assert_eq!(verify(&key, &unbound, ENTER, "abcdefghijklmnop"), None);
    }

    #[test]
    fn page_loads_renew_a_cookie_near_its_end() {
        let headers = |pairs: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.insert(*name, HeaderValue::from_static(value));
            }
            headers
        };
        let page = headers(&[("sec-fetch-dest", "iframe")]);
        let soon = now() + 600;
        assert!(renews(&Method::GET, &page, soon));
        assert!(renews(
            &Method::GET,
            &headers(&[("sec-fetch-dest", "document")]),
            soon
        ));
        assert!(renews(
            &Method::GET,
            &headers(&[("accept", "text/html,*/*")]),
            soon
        ));
        assert!(!renews(&Method::GET, &page, now() + VIEW_SECONDS));
        assert!(!renews(&Method::POST, &page, soon));
        assert!(!renews(
            &Method::GET,
            &headers(&[("sec-fetch-dest", "script")]),
            soon
        ));
        assert!(!renews(
            &Method::GET,
            &headers(&[("sec-fetch-dest", "empty"), ("accept", "text/html")]),
            soon
        ));
    }

    #[test]
    fn only_the_preview_itself_may_send_changes() {
        let origin = "https://abcdefghijklmnop.sbx.example.com";
        let headers = |pairs: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.insert(*name, HeaderValue::from_static(value));
            }
            headers
        };
        assert!(!cross_origin(
            &headers(&[("sec-fetch-site", "same-origin")]),
            origin
        ));
        assert!(!cross_origin(
            &headers(&[("origin", "https://abcdefghijklmnop.sbx.example.com")]),
            origin
        ));
        // Requests from no browser at all carry neither header.
        assert!(!cross_origin(&headers(&[]), origin));
        for refused in [
            headers(&[("sec-fetch-site", "same-site")]),
            headers(&[("sec-fetch-site", "cross-site")]),
            headers(&[
                ("sec-fetch-site", "same-site"),
                ("origin", "https://abcdefghijklmnop.sbx.example.com"),
            ]),
            headers(&[("origin", "https://bbcdefghijklmnop.sbx.example.com")]),
            headers(&[("origin", "null")]),
        ] {
            assert!(cross_origin(&refused, origin), "{refused:?}");
        }
    }

    #[test]
    fn preview_hosts_are_recognised_exactly() {
        let host = |h: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_str(h).unwrap());
            preview_slug(&headers, "sbx.example.com")
        };
        assert_eq!(
            host("abcdefghijklmnop.sbx.example.com").as_deref(),
            Some("abcdefghijklmnop")
        );
        assert_eq!(
            host("ABCDEFGHIJKLMNOP.sbx.example.com:443").as_deref(),
            Some("abcdefghijklmnop")
        );
        assert_eq!(host("chat.example.com"), None);
        assert_eq!(host("a.b.sbx.example.com"), None);
        assert_eq!(host("abcdefghijklmnop.sbx.example.com.evil.test"), None);
    }

    #[test]
    fn chat_preview_cookie_is_not_forwarded() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static(
                "obleth_preview=secret; theme=dark; __Host-obleth_preview=other",
            ),
        );
        assert_eq!(upstream_cookies(&headers).as_deref(), Some("theme=dark"));
        assert_eq!(cookie(&headers, COOKIE).as_deref(), Some("secret"));
        assert_eq!(cookie(&headers, HOST_COOKIE).as_deref(), Some("other"));
    }

    #[test]
    fn app_cookies_stay_on_their_own_host() {
        assert_eq!(
            app_cookie("sid=1; Domain=example.com; Path=/; HttpOnly").as_deref(),
            Some("sid=1; Path=/; HttpOnly")
        );
        assert_eq!(
            app_cookie("sid=1;domain = .example.com;Secure;DOMAIN").as_deref(),
            Some("sid=1; Secure")
        );
        assert_eq!(
            app_cookie("theme=dark; Max-Age=60").as_deref(),
            Some("theme=dark; Max-Age=60")
        );
        for chat in [
            "obleth_preview=forged; Path=/",
            "__Host-obleth_preview=forged; Path=/; Secure",
            "__Secure-obleth_chat=x; Secure",
            "__host-OBLETH_x=y",
        ] {
            assert_eq!(app_cookie(chat), None, "{chat}");
        }
    }
}
