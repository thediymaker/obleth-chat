//! A small Chrome DevTools Protocol client over one browser-level WebSocket. Pages
//! are reached through flattened sessions: each message carries its sessionId.
use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Longest a single command may take; navigation waits separately.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Cdp {
    out: mpsc::UnboundedSender<Message>,
    pending: Pending,
    events: Arc<Mutex<Option<mpsc::UnboundedSender<Value>>>>,
    next: AtomicU64,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Cdp {
    /// Connects to the browser's DevTools endpoint at `address` (host:port).
    pub async fn connect(http: &reqwest::Client, address: &str) -> Result<Self> {
        let version: Value = http
            .get(format!("http://{address}/json/version"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .context("The browser is not answering")?
            .json()
            .await
            .context("The browser sent an unreadable answer")?;
        let url = version
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("The browser did not offer a DevTools connection"))?;
        // Chromium names itself by the Host it was asked with; keep our address.
        let path = url
            .split_once("/devtools/")
            .map(|(_, path)| path)
            .ok_or_else(|| anyhow!("Unexpected DevTools address"))?;
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(format!("ws://{address}/devtools/{path}")),
        )
        .await
        .context("The browser connection timed out")??;
        let (mut sink, mut stream) = socket.split();
        let (out, mut outgoing) = mpsc::unbounded_channel::<Message>();
        let pending: Pending = Arc::default();
        let events: Arc<Mutex<Option<mpsc::UnboundedSender<Value>>>> = Arc::default();
        let writer = tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        let (reading, listeners) = (pending.clone(), events.clone());
        let reader = tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    let waiter = reading.lock().unwrap().remove(&id);
                    if let Some(waiter) = waiter {
                        let result = match value.get("error") {
                            Some(error) => Err(error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("DevTools error")
                                .to_owned()),
                            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = waiter.send(result);
                    }
                } else if let Some(listener) = listeners.lock().unwrap().as_ref() {
                    let _ = listener.send(value);
                }
            }
            // The connection ended: fail everything still waiting.
            reading.lock().unwrap().clear();
        });
        Ok(Self {
            out,
            pending,
            events,
            next: AtomicU64::new(1),
            tasks: vec![writer, reader],
        })
    }

    /// Sends a command, to a page when `session` is given, and waits for its result.
    pub async fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if self
            .out
            .send(Message::Text(message.to_string().into()))
            .is_err()
        {
            self.pending.lock().unwrap().remove(&id);
            bail!("The browser connection closed");
        }
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(error))) => bail!("{method}: {error}"),
            Ok(Err(_)) => bail!("The browser connection closed"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                bail!("The browser did not answer {method} in time")
            }
        }
    }

    /// Events from now on. Only one receiver is kept: a later call replaces it.
    pub fn events(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.events.lock().unwrap() = Some(tx);
        rx
    }

    /// Opens a flattened session on a page target.
    pub async fn attach(&self, target: &str) -> Result<String> {
        let attached = self
            .call(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                None,
            )
            .await?;
        attached
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("The browser did not open the tab"))
    }

    /// Evaluates an expression in a page and returns its value.
    pub async fn eval(&self, session: &str, expression: &str) -> Result<Value> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true,"awaitPromise":true}),
                Some(session),
            )
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            let text = details
                .pointer("/exception/description")
                .or_else(|| details.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("The page script failed");
            bail!("{}", text.lines().next().unwrap_or(text));
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }
}

impl Drop for Cdp {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
