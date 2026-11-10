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

/// Reads an upload of at most `most` bytes that keeps arriving. Thirty seconds
/// without a byte, or fifteen minutes in all, ends it, so a stalled upload does not
/// hold a turn or its memory for as long as its sender likes.
pub async fn upload(
    body: axum::body::Body,
    most: usize,
    too_large: &'static str,
) -> Result<Vec<u8>, ApiError> {
    use futures_util::StreamExt;
    use std::time::Duration;
    const QUIET: Duration = Duration::from_secs(30);
    const WHOLE: Duration = Duration::from_secs(15 * 60);
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
            match tokio::time::timeout(QUIET, stream.next()).await {
                Err(_) => return Err(stalled()),
                Ok(None) => return Ok(bytes),
                Ok(Some(Err(_))) => {
                    return Err(ApiError::bad("The upload was cut off. Try again."))
                }
                Ok(Some(Ok(chunk))) => {
                    if bytes.len() + chunk.len() > most {
                        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, too_large.into()));
                    }
                    bytes.extend_from_slice(&chunk);
                }
            }
        }
    };
    tokio::time::timeout(WHOLE, read)
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
