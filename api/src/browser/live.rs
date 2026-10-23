//! The live view of a conversation's tab: Chromium's screencast frames go to the
//! page as binary JPEG messages, and state (address, title, who has control, frame
//! size) as JSON text. The page's mouse and keyboard events, address bar and
//! back/forward/reload reach the tab only while the person has control. The page
//! may size the tab to fit its pane at any time.
use super::{manager, Browsers, Page};
use crate::{error::ApiError, AppState};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Longest a live view stays open.
const SESSION: Duration = Duration::from_secs(4 * 60 * 60);
/// Smallest and largest tab the page may ask for, in CSS pixels.
const MIN_VIEWPORT: (u64, u64) = (320, 240);
const MAX_VIEWPORT: (u64, u64) = (1920, 2400);

#[derive(Deserialize)]
pub struct Wake {
    #[serde(default)]
    wake: bool,
}

pub async fn open(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<Wake>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (browsers, user) = manager(&s, &headers, false).await?;
    // Cookies ride along on cross-site WebSockets, so only Chat's own pages may connect.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(browsers.chat_origin()) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Cross-origin WebSocket refused".into(),
        ));
    }
    if !browsers.is_on(user.id, id).await {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The browser is off in this chat".into(),
        ));
    }
    // Watching does not start a sleeping browser unless asked to.
    let running = browsers
        .get(user.id)
        .await?
        .is_some_and(|b| b.status == "running");
    if !running && !query.wake {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The browser is asleep".into(),
        ));
    }
    let page = browsers
        .open(user.id, id)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(upgrade.on_upgrade(move |client| bridge(client, page, browsers, user.id, id)))
}

async fn bridge(
    client: WebSocket,
    page: Page,
    browsers: Browsers,
    owner: Uuid,
    conversation: Uuid,
) {
    let (mut client_tx, mut client_rx) = client.split();
    let mut events = page.cdp.events();
    let session = page.session.clone();
    let started = async {
        page.cdp
            .call("Page.enable", json!({}), Some(&session))
            .await?;
        page.cdp
            .call(
                "Page.startScreencast",
                json!({"format":"jpeg","quality":60,"maxWidth":MAX_VIEWPORT.0,"maxHeight":MAX_VIEWPORT.1,"everyNthFrame":1}),
                Some(&session),
            )
            .await
    };
    if let Err(error) = started.await {
        tracing::warn!("could not start the browser live view: {error:#}");
        return;
    }
    let mut controller = page.tab.controller.clone();
    let mut sent = Value::Null;
    let mut size = (0u64, 0u64);
    let mut poll = tokio::time::interval(Duration::from_millis(1500));
    let mut touched = Instant::now();
    let deadline = tokio::time::sleep(SESSION);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                if event["sessionId"] != session.as_str() || event["method"] != "Page.screencastFrame" {
                    continue;
                }
                let params = &event["params"];
                let _ = page.cdp.call("Page.screencastFrameAck", json!({"sessionId":params["sessionId"]}), Some(&session)).await;
                let frame = (
                    params.pointer("/metadata/deviceWidth").and_then(Value::as_f64).unwrap_or(0.0) as u64,
                    params.pointer("/metadata/deviceHeight").and_then(Value::as_f64).unwrap_or(0.0) as u64,
                );
                // A step's thumbnail renders the tab small for a moment; that frame is not the page.
                if frame.0 < MIN_VIEWPORT.0 || frame.1 < MIN_VIEWPORT.1 {
                    continue;
                }
                if frame != size {
                    size = frame;
                    let message = json!({"t":"size","w":size.0,"h":size.1}).to_string();
                    if client_tx.send(Message::Text(message.into())).await.is_err() { break; }
                }
                let Some(Ok(jpeg)) = params["data"].as_str().map(|d| STANDARD.decode(d)) else { continue };
                if client_tx.send(Message::Binary(jpeg.into())).await.is_err() { break; }
            }
            message = client_rx.next() => {
                let Some(Ok(message)) = message else { break };
                let Message::Text(text) = message else {
                    if matches!(message, Message::Close(_)) { break; }
                    continue;
                };
                let Ok(input) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                // Fitting the tab to the pane is not input: it works whoever has control.
                if input["t"] == "viewport" {
                    if let Err(error) = fit(&page, &input).await {
                        tracing::debug!("could not size the browser tab: {error:#}");
                    }
                    continue;
                }
                // Take over may have just happened: ask before dropping the input.
                if controller != "user" {
                    if let Ok(Some(tab)) = browsers.get_tab(owner, conversation).await {
                        controller = tab.controller;
                    }
                    if controller != "user" { continue; }
                }
                if let Err(error) = forward(&page, &input).await {
                    tracing::debug!("browser input failed: {error:#}");
                }
                if touched.elapsed() > Duration::from_secs(30) {
                    browsers.touch(page.browser.id).await;
                    touched = Instant::now();
                }
            }
            _ = poll.tick() => {
                if let Ok(Some(tab)) = browsers.get_tab(owner, conversation).await {
                    controller = tab.controller;
                }
                let info = page.cdp.call("Target.getTargetInfo", json!({"targetId":page.target}), None).await;
                let Ok(info) = info else { break };
                let state = json!({"t":"state","controller":controller,"url":info.pointer("/targetInfo/url"),"title":info.pointer("/targetInfo/title")});
                if state != sent {
                    if client_tx.send(Message::Text(state.to_string().into())).await.is_err() { break; }
                    sent = state;
                }
                // Someone watching keeps the browser awake.
                if touched.elapsed() > Duration::from_secs(60) {
                    browsers.touch(page.browser.id).await;
                    touched = Instant::now();
                }
            }
            _ = &mut deadline => break,
        }
    }
    let _ = page
        .cdp
        .call("Page.stopScreencast", json!({}), Some(&session))
        .await;
    let _ = client_tx.close().await;
}

/// Sizes the tab to the pane showing it. The size lasts while this live view is
/// connected; afterwards the tab returns to the browser's window size.
async fn fit(page: &Page, input: &Value) -> anyhow::Result<()> {
    let side = |key: &str, low: u64, high: u64| input[key].as_u64().map(|n| n.clamp(low, high));
    let (Some(width), Some(height)) = (
        side("w", MIN_VIEWPORT.0, MAX_VIEWPORT.0),
        side("h", MIN_VIEWPORT.1, MAX_VIEWPORT.1),
    ) else {
        return Ok(());
    };
    page.cdp
        .call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
            Some(&page.session),
        )
        .await?;
    Ok(())
}

/// Replays one input event from the page on the tab, after checking its fields.
async fn forward(page: &Page, input: &Value) -> anyhow::Result<()> {
    let session = Some(page.session.as_str());
    let number = |key: &str, low: f64, high: f64| {
        input[key]
            .as_f64()
            .filter(|n| n.is_finite())
            .map(|n| n.clamp(low, high))
            .unwrap_or(0.0)
    };
    let modifiers = input["modifiers"].as_u64().unwrap_or(0) & 15;
    match input["t"].as_str().unwrap_or("") {
        "mouse" => {
            let kind = input["type"].as_str().unwrap_or("");
            if !matches!(
                kind,
                "mousePressed" | "mouseReleased" | "mouseMoved" | "mouseWheel"
            ) {
                return Ok(());
            }
            let button = input["button"]
                .as_str()
                .filter(|b| matches!(*b, "left" | "middle" | "right"))
                .unwrap_or("none");
            let mut event = json!({"type":kind,"x":number("x",0.0,4000.0),"y":number("y",0.0,4000.0),"modifiers":modifiers,"button":button,"clickCount":input["clickCount"].as_u64().unwrap_or(0).min(3)});
            if kind == "mouseWheel" {
                event["deltaX"] = json!(number("deltaX", -5000.0, 5000.0));
                event["deltaY"] = json!(number("deltaY", -5000.0, 5000.0));
            }
            page.cdp
                .call("Input.dispatchMouseEvent", event, session)
                .await?;
        }
        "key" => {
            let kind = input["type"].as_str().unwrap_or("");
            if !matches!(kind, "keyDown" | "keyUp" | "rawKeyDown" | "char") {
                return Ok(());
            }
            let text = |key: &str, limit: usize| -> String {
                input[key]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(limit)
                    .collect()
            };
            let mut event = json!({"type":kind,"modifiers":modifiers,"key":text("key",32),"code":text("code",32),"windowsVirtualKeyCode":input["keyCode"].as_u64().unwrap_or(0).min(255),"nativeVirtualKeyCode":input["keyCode"].as_u64().unwrap_or(0).min(255)});
            let typed = text("text", 4);
            if !typed.is_empty() {
                event["text"] = json!(typed);
                event["unmodifiedText"] = json!(typed);
            }
            page.cdp
                .call("Input.dispatchKeyEvent", event, session)
                .await?;
        }
        "text" => {
            let text: String = input["text"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(5000)
                .collect();
            if !text.is_empty() {
                page.cdp
                    .call("Input.insertText", json!({"text":text}), session)
                    .await?;
            }
        }
        "go" => {
            let raw = input["url"].as_str().unwrap_or("").trim();
            let address = if raw.contains("://") {
                raw.to_owned()
            } else {
                format!("https://{raw}")
            };
            if let Ok(url) = url::Url::parse(&address) {
                if matches!(url.scheme(), "http" | "https") {
                    page.cdp
                        .call("Page.navigate", json!({"url":url.as_str()}), session)
                        .await?;
                }
            }
        }
        "back" => {
            page.cdp.eval(&page.session, "history.back()").await?;
        }
        "forward" => {
            page.cdp.eval(&page.session, "history.forward()").await?;
        }
        "reload" => {
            page.cdp.call("Page.reload", json!({}), session).await?;
        }
        _ => {}
    }
    Ok(())
}
