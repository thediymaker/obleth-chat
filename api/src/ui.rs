//! The web interface as its own server: the Next.js app in `ui/`. Chat stays the
//! front door and sends every request that no Chat route answers to that server, so
//! API calls, streams and websockets never pass through it. The site's branding
//! (`site.config.js` and `assets/`) comes from the branding folder, where installs
//! mount their own.
use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use std::{path::PathBuf, time::Duration};
use tower_http::services::ServeDir;
use url::Url;

/// Tells the interface which origins its workspace previews use, for its
/// Content Security Policy. Any copy a browser sends is dropped.
const PREVIEW_HEADER: &str = "x-obleth-chat-preview-origin";

/// Headers about one connection, not the request, which a proxy must not pass on.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

#[derive(Clone)]
pub struct Ui {
    base: Url,
    client: reqwest::Client,
    previews: Option<HeaderValue>,
}

impl Ui {
    /// The interface server from OBLETH_CHAT_UI_URL, if one is set.
    pub fn from_env(previews: Option<String>) -> anyhow::Result<Option<Self>> {
        let Ok(raw) = std::env::var("OBLETH_CHAT_UI_URL") else {
            return Ok(None);
        };
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        let base =
            Url::parse(raw).map_err(|e| anyhow::anyhow!("Invalid OBLETH_CHAT_UI_URL: {e}"))?;
        if !matches!(base.scheme(), "http" | "https") {
            anyhow::bail!("OBLETH_CHAT_UI_URL must be an http or https address");
        }
        // No pooled connections: a pooled one can still point at an interface pod
        // that is shutting down, while a new one goes only to a ready pod.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Some(Self {
            base,
            client,
            previews: previews.and_then(|p| HeaderValue::from_str(&p).ok()),
        }))
    }

    /// Branding from its folder; everything else from the interface server.
    pub fn router(self, branding_dir: PathBuf) -> Router {
        branding(branding_dir).fallback(move |request: Request| {
            let ui = self.clone();
            async move { ui.forward(request).await }
        })
    }

    async fn forward(&self, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let path = parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/");
        let Ok(url) = self.base.join(path) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let mut headers = forwardable(&parts.headers);
        headers.remove(PREVIEW_HEADER);
        if let Some(previews) = &self.previews {
            headers.insert(PREVIEW_HEADER, previews.clone());
        }
        if let Some(host) = parts.headers.get(header::HOST) {
            headers.insert("x-forwarded-host", host.clone());
        }
        headers.remove(header::HOST);
        // Page loads have no body, so one that fails on the way out is sent again,
        // as while the interface server restarts.
        let upstream = if matches!(parts.method, Method::GET | Method::HEAD) {
            let send = || {
                self.client
                    .request(parts.method.clone(), url.clone())
                    .headers(headers.clone())
                    .send()
            };
            match send().await {
                Err(error) if error.is_connect() || error.is_request() => send().await,
                result => result,
            }
        } else {
            self.client
                .request(parts.method, url)
                .headers(headers)
                .body(reqwest::Body::wrap_stream(body.into_data_stream()))
                .send()
                .await
        };
        let upstream = match upstream {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = ?error, "the web interface did not answer");
                return (
                    StatusCode::BAD_GATEWAY,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    "The web interface is not available right now. Try again in a minute.",
                )
                    .into_response();
            }
        };
        let mut response = Response::builder().status(upstream.status());
        if let Some(out) = response.headers_mut() {
            *out = forwardable(upstream.headers());
        }
        response
            .body(Body::from_stream(upstream.bytes_stream()))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
    }
}

/// The site's `site.config.js` and `assets/`, which installs replace with their own.
pub fn branding(branding_dir: PathBuf) -> Router {
    Router::new()
        .route_service(
            "/site.config.js",
            tower_http::services::ServeFile::new(branding_dir.join("site.config.js")),
        )
        .nest_service("/assets", ServeDir::new(branding_dir.join("assets")))
}

/// A copy of the headers without the hop-by-hop ones, including any a Connection
/// header names.
fn forwardable(headers: &HeaderMap) -> HeaderMap {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    let mut out = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        if HOP_BY_HOP.contains(&name.as_str()) || named.contains(name) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_headers_stay_behind() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("keep-alive, x-secret-hop"),
        );
        headers.insert("x-secret-hop", HeaderValue::from_static("1"));
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        headers.insert(header::COOKIE, HeaderValue::from_static("a=b"));
        headers.append(header::ACCEPT, HeaderValue::from_static("text/html"));
        let out = forwardable(&headers);
        assert!(out.get("x-secret-hop").is_none());
        assert!(out.get(header::TRANSFER_ENCODING).is_none());
        assert!(out.get(header::CONNECTION).is_none());
        assert_eq!(out.get(header::COOKIE).unwrap(), "a=b");
        assert_eq!(out.get(header::ACCEPT).unwrap(), "text/html");
    }
}
