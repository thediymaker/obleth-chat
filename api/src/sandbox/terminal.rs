//! An interactive shell in a conversation's project folder, bridged from a browser
//! WebSocket to a Kubernetes exec session with a TTY. Browser binary frames are
//! keystrokes, text frames are JSON resize requests; output returns as binary.
use super::{manager, tools, Sandbox, Sandboxes};
use crate::{error::ApiError, AppState};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite;
use uuid::Uuid;

/// Longest a terminal stays open.
const SESSION: Duration = Duration::from_secs(12 * 60 * 60);

// Interactive bash in the project folder; the prompt shows paths relative to it.
const SHELL: &str = "export TERM=xterm-256color OBLETH_DIR=\"$PWD\"; \
    export PS1='\\[\\e[36m\\]${PWD/#$OBLETH_DIR/project}\\[\\e[0m\\] \\$ '; exec bash --norc -i";

pub async fn open(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, false).await?;
    // Cookies ride along on cross-site WebSockets, so only Chat's own pages may connect.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(sandboxes.config().chat_origin.as_str()) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Cross-origin WebSocket refused".into(),
        ));
    }
    let sandbox = sandboxes
        .get(user.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let sandbox = sandboxes
        .ensure_running(&sandbox)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    let script = tools::in_project(&sandboxes, &sandbox, SHELL);
    let shell = sandboxes
        .exec_socket(&sandbox, &["sh", "-c", &script])
        .await
        .map_err(|error| {
            tracing::warn!("could not open a workspace terminal: {error:#}");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "The terminal could not start".into(),
            )
        })?;
    Ok(upgrade.on_upgrade(move |client| bridge(client, shell, sandboxes, sandbox)))
}

async fn bridge(
    client: WebSocket,
    shell: super::kube::ExecSocket,
    sandboxes: Sandboxes,
    sandbox: Sandbox,
) {
    let (mut client_tx, mut client_rx) = client.split();
    let (mut shell_tx, mut shell_rx) = shell.split();
    let keys = async {
        let mut touched = Instant::now();
        while let Some(Ok(message)) = client_rx.next().await {
            let frame = match message {
                Message::Binary(data) => [&[0u8][..], &data].concat(),
                Message::Text(text) => match resize(text.as_str()) {
                    Some(size) => [&[4u8][..], size.as_bytes()].concat(),
                    None => continue,
                },
                Message::Close(_) => break,
                _ => continue,
            };
            if shell_tx
                .send(tungstenite::Message::Binary(frame.into()))
                .await
                .is_err()
            {
                break;
            }
            // Typing keeps the workspace awake.
            if touched.elapsed() > Duration::from_secs(30) {
                sandboxes.touch(&sandbox).await;
                touched = Instant::now();
            }
        }
    };
    let output = async {
        while let Some(Ok(message)) = shell_rx.next().await {
            let tungstenite::Message::Binary(data) = message else {
                if matches!(message, tungstenite::Message::Close(_)) {
                    break;
                }
                continue;
            };
            match data.split_first() {
                Some((1 | 2, payload)) => {
                    let payload = payload.to_vec();
                    if client_tx
                        .send(Message::Binary(payload.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some((3, _)) => break,
                _ => {}
            }
        }
        let _ = client_tx
            .send(Message::Text(json!({"type":"exit"}).to_string().into()))
            .await;
        let _ = client_tx.close().await;
    };
    tokio::select! {
        _ = keys => {}
        _ = output => {}
        _ = tokio::time::sleep(SESSION) => {}
    }
}

/// `{"type":"resize","cols":80,"rows":24}` as the exec resize message.
fn resize(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("type")?.as_str()? != "resize" {
        return None;
    }
    let size = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_u64)
            .filter(|n| (1..=1000).contains(n))
    };
    Some(json!({"Width": size("cols")?, "Height": size("rows")?}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_requests_are_validated() {
        assert_eq!(
            resize(r#"{"type":"resize","cols":120,"rows":40}"#).as_deref(),
            Some(r#"{"Height":40,"Width":120}"#)
        );
        assert_eq!(resize(r#"{"type":"resize","cols":0,"rows":40}"#), None);
        assert_eq!(resize(r#"{"type":"other"}"#), None);
        assert_eq!(resize("not json"), None);
    }
}
