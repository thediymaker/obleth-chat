//! Serves `https://<slug>.<preview domain>` from the workspace's dev server. Each
//! workspace gets its own origin, so app code never runs on Chat's origin, never
//! sees Chat's `__Host-` session cookie, and cannot read another workspace. Access
//! needs a host-only cookie minted from a one-minute ticket that Chat issues to the
//! workspace owner.
use super::Sandboxes;
use crate::error::ApiError;
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, FromRequestParts, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};
use uuid::Uuid;

pub const ENTER: &str = "enter";
const VIEW: &str = "view";
const COOKIE: &str = "obleth_preview";
const VIEW_SECONDS: u64 = 8 * 3600;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn sign(key: &[u8], purpose: &str, slug: &str, user: Uuid, seconds: u64) -> String {
    let payload = URL_SAFE_NO_PAD
        .encode(json!({"p":purpose,"s":slug,"u":user,"e":now() + seconds}).to_string());
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    format!(
        "{payload}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

/// Returns the user a token was issued to when it is authentic, unexpired, and for
/// this purpose and workspace.
pub fn verify(key: &[u8], token: &str, purpose: &str, slug: &str) -> Option<Uuid> {
    let (payload, signature) = token.split_once('.')?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&URL_SAFE_NO_PAD.decode(signature).ok()?)
        .ok()?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    (claims["p"] == purpose && claims["s"] == slug && claims["e"].as_u64()? > now())
        .then(|| claims["u"].as_str()?.parse().ok())
        .flatten()
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
    let domain = sb.config().preview_domain.clone();
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
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
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
        .filter(|pair| !pair.is_empty() && !pair.starts_with(&format!("{COOKIE}=")))
        .collect::<Vec<_>>()
        .join("; ");
    (!rest.is_empty()).then_some(rest)
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

/// Percent-encodes a path for a Location header, keeping its slashes.
fn encode_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn serve(sb: &Sandboxes, slug: &str, request: Request) -> Result<Response, ApiError> {
    let key = sb.key().await?.to_vec();
    let path = request.uri().path().to_owned();
    if path == "/__obleth/enter" {
        let query = |name: &str| {
            request.uri().query().and_then(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.into_owned())
            })
        };
        let ticket = query("ticket").unwrap_or_default();
        let next = query("next")
            .as_deref()
            .and_then(landing)
            .map_or_else(|| "/".into(), |page| encode_path(&page));
        let user = verify(&key, &ticket, ENTER, slug).ok_or_else(|| {
            ApiError(
                StatusCode::FORBIDDEN,
                "This preview link has expired. Reopen the preview from Chat.".into(),
            )
        })?;
        let secure = if sb.config().preview_scheme == "https" {
            "; Secure"
        } else {
            ""
        };
        let value = sign(&key, VIEW, slug, user, VIEW_SECONDS);
        let mut response = Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::LOCATION, next)
            .header(header::SET_COOKIE, format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={VIEW_SECONDS}{secure}"))
            .body(Body::empty())
            .map_err(|_| ApiError::internal())?;
        frame_headers(response.headers_mut(), sb);
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(response);
    }
    let user = cookie(request.headers(), COOKIE)
        .and_then(|token| verify(&key, &token, VIEW, slug))
        .ok_or_else(|| {
            ApiError(
                StatusCode::UNAUTHORIZED,
                "Open this preview from its conversation in Chat.".into(),
            )
        })?;
    let sandbox = sb
        .by_slug(slug)
        .await?
        .filter(|s| s.owner_id == user)
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                "This workspace no longer exists.".into(),
            )
        })?;
    let (Some(ip), Some(port), "running") = (
        sandbox.pod_ip.clone(),
        sandbox.preview_port,
        sandbox.status.as_str(),
    ) else {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "The preview is not running. Ask the assistant to start it.".into(),
        ));
    };
    sb.touch(&sandbox).await;
    let target = format!("{ip}:{port}");
    let is_upgrade = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if is_upgrade {
        return websocket(sb, &sandbox, &target, request).await;
    }
    proxy(sb, &target, request).await
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
        if name == header::SET_COOKIE
            && value
                .to_str()
                .is_ok_and(|v| v.trim_start().starts_with(&format!("{COOKIE}=")))
        {
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

async fn websocket(
    sb: &Sandboxes,
    sandbox: &super::Sandbox,
    target: &str,
    request: Request,
) -> Result<Response, ApiError> {
    // Only the preview page itself may open sockets to its dev server.
    let origin = sb.preview_origin(sandbox);
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
        tokio::select! { _ = to_server => {}, _ = to_client => {} }
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
        let user = Uuid::new_v4();
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
            HeaderValue::from_static("obleth_preview=secret; theme=dark"),
        );
        assert_eq!(upstream_cookies(&headers).as_deref(), Some("theme=dark"));
        assert_eq!(cookie(&headers, COOKIE).as_deref(), Some("secret"));
    }
}
