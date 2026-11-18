use axum::{
    http::StatusCode,
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
