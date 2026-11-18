use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);
impl ApiError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "Sign in to continue".into())
    }
    pub fn forbidden() -> Self {
        Self(StatusCode::FORBIDDEN, "Access denied".into())
    }
    pub fn not_found() -> Self {
        Self(StatusCode::NOT_FOUND, "Not found".into())
    }
    pub fn internal() -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The service could not complete this request".into(),
        )
    }
}
/// Reads and drops the rest of a request that was refused before its body was
/// read, up to a megabyte and for two seconds at most. Answering while the client
/// is still sending closes the connection under it, and a proxy in front then
/// reports a bad gateway instead of the answer. Nothing is kept, and a larger or
/// slower body is still cut off.
pub async fn discard(body: axum::body::Body) {
    use futures_util::StreamExt;
    const MOST: usize = 1024 * 1024;
    let mut stream = body.into_data_stream();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut seen = 0;
        while let Some(Ok(chunk)) = stream.next().await {
            seen += chunk.len();
            if seen > MOST {
                break;
            }
        }
    })
    .await;
}

/// The outcome of an upload's checks, with its body: kept for a request that
/// passed them, drained before the refusal of one that did not. Every upload
/// endpoint decides before it reads, so this is the one place a refusal and the
/// body meet.
pub async fn admitted<T>(
    body: axum::body::Body,
    checks: Result<T, ApiError>,
) -> Result<(T, axum::body::Body), ApiError> {
    match checks {
        Ok(passed) => Ok((passed, body)),
        Err(refused) => {
            discard(body).await;
            Err(refused)
        }
    }
}

/// Reads an upload of at most `most` bytes that keeps arriving. Thirty seconds
/// without a byte, or fifteen minutes in all, ends it, so a stalled upload does not
/// hold a turn or its memory for as long as its sender likes.
pub async fn upload(
    body: axum::body::Body,
    most: usize,
    too_large: impl Into<String>,
) -> Result<Vec<u8>, ApiError> {
    use std::time::Duration;
    const QUIET: Duration = Duration::from_secs(30);
    const WHOLE: Duration = Duration::from_secs(15 * 60);
    read_within(body, most, too_large.into(), QUIET, WHOLE).await
}

async fn read_within(
    body: axum::body::Body,
    most: usize,
    too_large: String,
    quiet: std::time::Duration,
    whole: std::time::Duration,
) -> Result<Vec<u8>, ApiError> {
    use futures_util::StreamExt;
    let stalled = || {
        ApiError(
            StatusCode::REQUEST_TIMEOUT,
            "The upload stopped arriving. Try again.".into(),
        )
    };
    let mut stream = body.into_data_stream();
    let read = async {
        let mut bytes = Vec::new();
        loop {
            match tokio::time::timeout(quiet, stream.next()).await {
                Err(_) => return Err(stalled()),
                Ok(None) => return Ok(bytes),
                Ok(Some(Err(_))) => {
                    return Err(ApiError::bad("The upload was cut off. Try again."))
                }
                Ok(Some(Ok(chunk))) => {
                    if bytes.len() + chunk.len() > most {
                        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, too_large));
                    }
                    bytes.extend_from_slice(&chunk);
                }
            }
        }
    };
    tokio::time::timeout(whole, read)
        .await
        .unwrap_or_else(|_| Err(stalled()))
}

/// Every answer under /api and /mcp is JSON. The extractors' plain-text
/// rejections (a malformed body, a wrong content type, a bad id in the path, a
/// body over the limit) and the bodiless 405 become the error envelope with the
/// same status and headers. The message is Chat's, not serde's, so field names
/// and positions never leak.
pub async fn envelope(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let api = path.starts_with("/api/") || path == "/mcp";
    let response = next.run(request).await;
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !api || !response.status().is_client_error() || json {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    let message = match parts.status {
        StatusCode::BAD_REQUEST => "The request could not be read. Check its body and path.",
        StatusCode::NOT_FOUND => "Not found",
        StatusCode::METHOD_NOT_ALLOWED => "That method is not allowed here",
        StatusCode::LENGTH_REQUIRED => "Send the request with its size (Content-Length)",
        StatusCode::PAYLOAD_TOO_LARGE => "The request is too large",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "Send JSON (Content-Type: application/json)",
        StatusCode::UNPROCESSABLE_ENTITY => "The request has a missing, unknown or malformed field",
        other => other
            .canonical_reason()
            .unwrap_or("The request was refused"),
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let body = json!({"error":{"message":message}}).to_string();
    Response::from_parts(parts, Body::from(body))
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":{"message":self.1}}))).into_response()
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        // A bounded class and SQLSTATE only: driver messages can quote values.
        let kind = match &error {
            sqlx::Error::PoolTimedOut => "pool_timeout",
            sqlx::Error::PoolClosed => "pool_closed",
            sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "connection",
            sqlx::Error::Protocol(_) => "protocol",
            sqlx::Error::RowNotFound => "row_not_found",
            sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "decode",
            sqlx::Error::Database(_) => "database",
            _ => "other",
        };
        let sqlstate = match &error {
            sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
            _ => None,
        };
        tracing::error!(
            kind,
            sqlstate = sqlstate.as_deref().unwrap_or(""),
            "database operation failed"
        );
        Self::internal()
    }
}

#[cfg(test)]
mod upload_tests {
    use super::*;
    use axum::body::{Body, Bytes};
    use futures_util::{stream, StreamExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A body of `chunks` that counts how many of them were read.
    fn counted(chunks: usize) -> (Body, Arc<AtomicUsize>) {
        let read = Arc::new(AtomicUsize::new(0));
        let seen = read.clone();
        let body = Body::from_stream(
            stream::iter(
                (0..chunks).map(|_| Ok::<_, std::io::Error>(Bytes::from_static(b"0123456789"))),
            )
            .inspect(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
            }),
        );
        (body, read)
    }

    #[tokio::test]
    async fn a_refused_upload_is_drained_before_its_answer() {
        let (body, read) = counted(8);
        let refused = admitted::<()>(body, Err(ApiError::bad("no"))).await;
        assert_eq!(refused.unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(read.load(Ordering::SeqCst), 8);

        let (body, read) = counted(8);
        let (passed, body) = admitted(body, Ok::<_, ApiError>("yes")).await.unwrap();
        assert_eq!((passed, read.load(Ordering::SeqCst)), ("yes", 0));
        assert_eq!(upload(body, 80, "big").await.unwrap().len(), 80);
    }

    #[tokio::test]
    async fn an_upload_over_its_limit_is_refused_as_too_large() {
        let (body, _) = counted(8);
        let err = upload(body, 79, "Choose a smaller file").await.unwrap_err();
        assert_eq!(
            (err.0, err.1.as_str()),
            (StatusCode::PAYLOAD_TOO_LARGE, "Choose a smaller file")
        );
    }

    #[tokio::test]
    async fn a_stalled_upload_ends_at_the_quiet_bound() {
        let body = Body::from_stream(stream::pending::<Result<Bytes, std::io::Error>>());
        let started = Instant::now();
        let err = read_within(
            body,
            10,
            "big".into(),
            Duration::from_millis(50),
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::REQUEST_TIMEOUT);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::*;
    use axum::{
        extract::{DefaultBodyLimit, Path},
        middleware,
        routing::{any, get, post},
        Router,
    };
    use serde::Deserialize;
    use serde_json::Value;
    use tower::ServiceExt;
    use uuid::Uuid;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        #[allow(dead_code)]
        name: String,
    }
    async fn take(Json(_): Json<Probe>) -> StatusCode {
        StatusCode::NO_CONTENT
    }
    async fn by_id(Path(_): Path<Uuid>) -> StatusCode {
        StatusCode::NO_CONTENT
    }
    fn app() -> Router {
        Router::new()
            .route("/api/probe", post(take))
            .route("/api/things/{id}", get(by_id))
            .route(
                "/mcp",
                post(take).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
            )
            .route("/api/{*rest}", any(|| async { ApiError::not_found() }))
            .route(
                "/page",
                get(|| async { (StatusCode::NOT_FOUND, "no such page") }),
            )
            .layer(DefaultBodyLimit::max(64))
            .layer(middleware::from_fn(envelope))
    }
    async fn call(
        method: &str,
        path: &str,
        content_type: &str,
        body: &str,
    ) -> (StatusCode, String, String) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body.to_owned()))
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        let status = response.status();
        let kind = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        (status, kind, String::from_utf8(bytes.to_vec()).unwrap())
    }
    fn message(body: &str) -> String {
        let value: Value = serde_json::from_str(body).expect(body);
        value["error"]["message"].as_str().expect(body).to_owned()
    }

    #[tokio::test]
    async fn extractor_rejections_and_unknown_routes_answer_with_the_envelope() {
        let json = "application/json";
        let cases = [
            (
                "POST",
                "/api/probe",
                json,
                "{not json",
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/probe",
                "text/plain",
                "{}",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                "POST",
                "/api/probe",
                json,
                r#"{"name":"a","extra":1}"#,
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "GET",
                "/api/things/not-a-uuid",
                json,
                "",
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/api/probe",
                json,
                &format!(r#"{{"name":"{}"}}"#, "x".repeat(100)),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
            (
                "PATCH",
                "/api/nowhere/at/all",
                json,
                "{}",
                StatusCode::NOT_FOUND,
            ),
            ("GET", "/mcp", json, "", StatusCode::METHOD_NOT_ALLOWED),
        ];
        for (method, path, content_type, body, expected) in cases {
            let (status, kind, answer) = call(method, path, content_type, body).await;
            assert_eq!(status, expected, "{method} {path}");
            assert_eq!(kind, "application/json", "{method} {path}");
            let text = message(&answer);
            assert!(
                !text.is_empty() && !text.contains("extra") && !text.contains("line "),
                "{text}"
            );
        }
        // A well-formed request is untouched, as is a page outside the API.
        assert_eq!(
            call("POST", "/api/probe", json, r#"{"name":"a"}"#).await.0,
            StatusCode::NO_CONTENT
        );
        let (status, kind, answer) = call("GET", "/page", json, "").await;
        assert_eq!(
            (status, kind.as_str(), answer.as_str()),
            (
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                "no such page"
            )
        );
    }

    /// Every JSON request body refuses fields it does not know, so a typo in a
    /// client is answered instead of ignored. The one exception is the cluster
    /// agent's protocol: agents already installed on clusters send fields Chat no
    /// longer reads, and they update on their own schedule.
    #[test]
    fn every_request_body_refuses_unknown_fields() {
        const AGENT_PROTOCOL: [&str; 3] = ["Hello", "Result_", "Bye"];
        fn sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push((
                        path.display().to_string(),
                        std::fs::read_to_string(&path).unwrap(),
                    ));
                }
            }
        }
        let mut files = Vec::new();
        sources(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/api/src")),
            &mut files,
        );
        let mut lax = Vec::new();
        let mut checked = 0;
        for (path, source) in &files {
            for (at, _) in source.match_indices("): Json<") {
                let name: String = source[at + 8..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                    .collect();
                let name = name.rsplit("::").next().unwrap().to_owned();
                if name.is_empty() || name == "Value" || AGENT_PROTOCOL.contains(&name.as_str()) {
                    continue;
                }
                // The struct is in the handler's file, or elsewhere for a path like store::X.
                let defined_in = |s: &str| {
                    s.find(&format!("struct {name} "))
                        .or_else(|| s.find(&format!("struct {name}{{")))
                };
                let (holder, def) = match defined_in(source) {
                    Some(def) => (source.as_str(), def),
                    None => files
                        .iter()
                        .find_map(|(_, s)| defined_in(s).map(|def| (s.as_str(), def)))
                        .unwrap_or_else(|| panic!("{path}: no struct {name}")),
                };
                let attrs = &holder[holder[..def].rfind("\n\n").map_or(0, |i| i + 2)..def];
                checked += 1;
                if !attrs.contains("deny_unknown_fields") {
                    lax.push(format!("{path}: {name}"));
                }
            }
        }
        assert!(checked > 60, "found only {checked} JSON request bodies");
        assert!(
            lax.is_empty(),
            "request bodies that ignore unknown fields:\n{}",
            lax.join("\n")
        );
    }
}
