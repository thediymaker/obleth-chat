//! A Model Context Protocol endpoint, so a person's own MCP client, such as Claude
//! Code, can start chats, check on them and carry them on. It takes only personal API
//! tokens (auth::tokens), never the browser session, and a token reaches nothing else.
//! The chats run as they would from the browser and appear in the person's history,
//! under the same limits. Streamable HTTP answered with plain JSON: no sessions and
//! no server push, so every call stands alone.
use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::{
        accounts::record_audit,
        tokens::{TokenUser, READ, WRITE},
        Identity,
    },
    error::ApiError,
    launch_turn, launch_workspace, models, not_configured, AppState, Launch,
};

const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Characters of each message a client gets back; the chat itself keeps everything.
const MESSAGE_CHARS: usize = 8_000;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/mcp", post(handle).get(not_allowed).delete(not_allowed))
        .with_state(state)
}

/// No server-sent stream and no sessions to end.
async fn not_allowed() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // A browser page must not reach this with someone's token: refuse other origins.
    if let Some(origin) = headers.get(header::ORIGIN) {
        if origin.to_str().ok() != Some(s.auth.origin()) {
            return ApiError::forbidden().into_response();
        }
    }
    let token = match s.auth.require_token(&headers).await {
        Ok(token) => token,
        Err(error) => {
            let unauthorized = error.0 == StatusCode::UNAUTHORIZED;
            let mut response = error.into_response();
            if unauthorized {
                response.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static("Bearer realm=\"obleth-chat\""),
                );
            }
            return response;
        }
    };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return failure(Value::Null, -32700, "The request is not JSON");
    };
    if !message.is_object() || message["jsonrpc"] != "2.0" {
        return failure(Value::Null, -32600, "Send one JSON-RPC 2.0 message");
    }
    // Notifications and responses need no answer.
    let Some(id) = message.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or("") {
        "initialize" => initialize(params),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools(&token)}),
        "tools/call" => call(&s, &token, params).await,
        _ => return failure(id, -32601, "Unknown method"),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

fn failure(id: Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}

fn initialize(params: &Value) -> Value {
    let asked = params["protocolVersion"].as_str().unwrap_or("");
    let version = PROTOCOLS
        .iter()
        .find(|v| **v == asked)
        .unwrap_or(&PROTOCOLS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "obleth-chat", "version": env!("CARGO_PKG_VERSION")},
        "instructions": "Start and follow chats in the person's Chat account. Each chat runs on its own, as if the person had started it in the browser, and stays in their history, so they can open the link and pick it up there. Start a chat, then check on it with get_chat until it is no longer running. A chat that asks a question waits for an answer: answer it with send_message or leave it for the person."
    })
}

fn tools(token: &TokenUser) -> Vec<Value> {
    let id = json!({"type":"string","description":"The chat's id"});
    let mut list = vec![
        json!({"name":"list_chats","title":"List chats","description":"The person's chats, most recently active first.",
            "inputSchema":{"type":"object","properties":{
                "query":{"type":"string","description":"Only chats whose title or text matches"},
                "limit":{"type":"integer","minimum":1,"maximum":50,"description":"How many to return, 20 by default"}},
                "additionalProperties":false},
            "annotations":{"readOnlyHint":true}}),
        json!({"name":"get_chat","title":"Get a chat","description":"A chat's state: whether it is still running, what it is doing now, its latest messages, and any questions it is waiting on.",
            "inputSchema":{"type":"object","properties":{"id":id,
                "messages":{"type":"integer","minimum":1,"maximum":20,"description":"How many of the latest messages to return, 2 by default"}},
                "required":["id"],"additionalProperties":false},
            "annotations":{"readOnlyHint":true}}),
        json!({"name":"list_models","title":"List models","description":"The models the person may use. Auto lets the gateway pick one for each message.",
            "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
            "annotations":{"readOnlyHint":true}}),
    ];
    if token.allows(WRITE) {
        list.extend([
            json!({"name":"start_chat","title":"Start a chat","description":"Start a new chat with a first message. It runs on its own and returns at once; check on it with get_chat.",
                "inputSchema":{"type":"object","properties":{
                    "message":{"type":"string","description":"The first message, as the person would write it"},
                    "title":{"type":"string","description":"A title; the start of the message by default"},
                    "model":{"type":"string","description":"A model id from list_models; the person's default model otherwise"},
                    "workspace":{"type":"boolean","description":"Turn on the chat's workspace first, for building and running code"}},
                    "required":["message"],"additionalProperties":false},
                "annotations":{"readOnlyHint":false,"destructiveHint":false}}),
            json!({"name":"send_message","title":"Send a message","description":"Send the next message in a chat that is not running, such as an answer to its questions. Returns at once; check on it with get_chat.",
                "inputSchema":{"type":"object","properties":{"id":id,
                    "message":{"type":"string","description":"The message"},
                    "workspace":{"type":"boolean","description":"Turn on the chat's workspace first"}},
                    "required":["id","message"],"additionalProperties":false},
                "annotations":{"readOnlyHint":false,"destructiveHint":false}}),
            json!({"name":"stop_chat","title":"Stop a chat","description":"Stop the reply a chat is writing. What it wrote so far is kept.",
                "inputSchema":{"type":"object","properties":{"id":id},"required":["id"],"additionalProperties":false},
                "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true}}),
        ]);
    }
    list
}

async fn call(s: &AppState, token: &TokenUser, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];
    let write = matches!(name, "start_chat" | "send_message" | "stop_chat");
    let outcome = if !token.allows(if write { WRITE } else { READ }) {
        Err(ApiError::bad("This token may only read chats"))
    } else {
        match name {
            "list_chats" => list_chats(s, &token.user, args).await,
            "get_chat" => get_chat(s, &token.user, args).await,
            "list_models" => list_models(s, &token.user).await,
            "start_chat" => start_chat(s, token, args).await,
            "send_message" => send_message(s, token, args).await,
            "stop_chat" => stop_chat(s, token, args).await,
            _ => Err(ApiError::bad(format!("There is no tool named {name}"))),
        }
    };
    match outcome {
        Ok(value) => json!({
            "content":[{"type":"text","text":serde_json::to_string_pretty(&value).unwrap_or_default()}],
            "structuredContent": value,
        }),
        Err(error) => json!({"content":[{"type":"text","text":error.1}],"isError":true}),
    }
}

fn link(s: &AppState, chat: Uuid) -> String {
    format!("{}/#chat/{chat}", s.auth.origin())
}

fn chat_id(args: &Value) -> Result<Uuid, ApiError> {
    args["id"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| ApiError::bad("Give the chat's id"))
}

fn message(args: &Value) -> Result<String, ApiError> {
    args["message"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad("Write a message"))
}

/// The first `MESSAGE_CHARS` of a text, with a note of how much more there is.
fn clip(text: &str) -> String {
    let total = text.chars().count();
    if total <= MESSAGE_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().take(MESSAGE_CHARS).collect();
    format!(
        "{kept}\n\n[{} more characters: open the chat to read the rest]",
        total - MESSAGE_CHARS
    )
}

async fn list_chats(s: &AppState, user: &Identity, args: &Value) -> Result<Value, ApiError> {
    let query = crate::store::HistoryQuery {
        query: args["query"].as_str().map(str::to_owned),
        archived: None,
        limit: Some(args["limit"].as_i64().unwrap_or(20).clamp(1, 50)),
        offset: None,
    };
    let page = s.store.history(user.id, query).await?;
    let chats: Vec<Value> = page
        .conversations
        .iter()
        .map(|c| json!({"id":c.id,"title":c.title,"model":c.model,"updatedAt":c.updated_at,"url":link(s, c.id)}))
        .collect();
    Ok(json!({"chats":chats,"total":page.total,"hasMore":page.has_more}))
}

async fn get_chat(s: &AppState, user: &Identity, args: &Value) -> Result<Value, ApiError> {
    let id = chat_id(args)?;
    let chat = s.store.get(user.id, id).await?;
    let running = chat.active_generation_id.is_some();
    let count = args["messages"].as_u64().unwrap_or(2).clamp(1, 20) as usize;
    let start = chat.messages.len().saturating_sub(count);
    let messages: Vec<Value> = chat.messages[start..]
        .iter()
        .map(|m| {
            let mut v = json!({"role":m.role,"status":m.status,"content":clip(&m.content),"createdAt":m.created_at});
            if let Some(error) = &m.error_message {
                v["error"] = json!(error);
            }
            v
        })
        .collect();
    let last = chat.messages.last().filter(|m| m.role == "assistant");
    let activity = last
        .and_then(|m| m.activity.as_array())
        .cloned()
        .unwrap_or_default();
    // What it is doing now, and what it asked the person at the end of its reply.
    let now = running
        .then(|| activity.last().and_then(|e| e["summary"].as_str()))
        .flatten();
    let waiting: Vec<Value> = if running {
        vec![]
    } else {
        activity
            .iter()
            .filter(|e| e["status"] == "asked")
            .map(
                |e| json!({"kind":e["kind"],"summary":e["summary"],"questions":e.get("questions")}),
            )
            .collect()
    };
    let state = if running {
        "running"
    } else if !waiting.is_empty() {
        "waiting"
    } else {
        last.map(|m| m.status.as_str()).unwrap_or("idle")
    };
    Ok(json!({
        "id": chat.id, "title": chat.title, "model": chat.model, "url": link(s, chat.id),
        "state": state, "now": now, "waitingFor": waiting, "messages": messages,
        "messageCount": chat.messages.len(),
    }))
}

async fn list_models(s: &AppState, user: &Identity) -> Result<Value, ApiError> {
    let gateway = s.gateway.as_ref().ok_or_else(not_configured)?;
    let models = models::choices(&s.store.pool, gateway, user).await?;
    let models: Vec<Value> = models
        .iter()
        .filter(|m| m.available)
        .map(|m| json!({"id":m.id,"name":m.name,"description":m.description}))
        .collect();
    Ok(json!({"models":models}))
}

async fn start_chat(s: &AppState, token: &TokenUser, args: &Value) -> Result<Value, ApiError> {
    let user = &token.user;
    let content = message(args)?;
    let preferences = models::preferences(&s.store.pool, user.id).await;
    let model = args["model"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(&preferences.default_model)
        .to_owned();
    let title = args["title"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| content.lines().next().unwrap_or(&content))
        .chars()
        .take(80)
        .collect::<String>();
    let chat = s
        .store
        .create_with_effort(user.id, &title, &model, preferences.reasoning_effort)
        .await?;
    let started = begin(s, token, chat.id, content, model, args, &preferences).await;
    if let Err(error) = started {
        // A chat that could not start is not left behind empty.
        let _ = sqlx::query(
            "DELETE FROM conversations c WHERE id=$1 AND owner_id=$2 \
             AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id=c.id)",
        )
        .bind(chat.id)
        .bind(user.id)
        .execute(&s.store.pool)
        .await;
        return Err(error);
    }
    audit(s, token, "mcp_chat_started", chat.id).await?;
    Ok(json!({"id":chat.id,"title":chat.title,"url":link(s, chat.id),"state":"running"}))
}

async fn send_message(s: &AppState, token: &TokenUser, args: &Value) -> Result<Value, ApiError> {
    let id = chat_id(args)?;
    let content = message(args)?;
    let chat = s.store.get(token.user.id, id).await?;
    let preferences = models::preferences(&s.store.pool, token.user.id).await;
    begin(s, token, id, content, chat.model, args, &preferences).await?;
    audit(s, token, "mcp_message_sent", id).await?;
    Ok(json!({"id":id,"url":link(s, id),"state":"running"}))
}

/// Starts a turn as the browser's Send does, for no browser session: it runs to the
/// end on its own, and the person's limits on running replies apply.
async fn begin(
    s: &AppState,
    token: &TokenUser,
    id: Uuid,
    content: String,
    model: String,
    args: &Value,
    preferences: &models::Preferences,
) -> Result<(), ApiError> {
    let user = &token.user;
    let gateway = s.gateway.clone().ok_or_else(not_configured)?;
    if s.shutdown.is_cancelled() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service is shutting down".into(),
        ));
    }
    if args["workspace"].as_bool() == Some(true) {
        if !s.sandboxes.as_ref().is_some_and(|sb| sb.allowed(user)) {
            return Err(ApiError::bad(
                "Workspaces are not available to this account",
            ));
        }
        launch_workspace(s, user, id)
            .await
            .map_err(|e| ApiError(StatusCode::SERVICE_UNAVAILABLE, e))?;
    }
    let saved: models::ParameterOverrides =
        serde_json::from_value(s.store.saved_settings(user.id, id).await?)
            .map_err(|_| ApiError::internal())?;
    let p = preferences;
    let overrides = models::ParameterOverrides {
        system_prompt: saved
            .system_prompt
            .or_else(|| Some(p.system_prompt.clone()).filter(|t| !t.is_empty())),
        temperature: saved.temperature.or(Some(p.temperature)),
        top_p: saved.top_p.or(Some(p.top_p)),
        max_tokens: saved.max_tokens.or(Some(p.max_tokens).filter(|t| *t > 0)),
        reasoning_effort: saved.reasoning_effort.or(Some(p.reasoning_effort)),
    };
    let launch = Launch {
        content,
        model,
        attachment_ids: vec![],
        overrides,
        variant: None,
    };
    // Nothing reads the live stream; the reply is saved as it is written.
    launch_turn(s.clone(), gateway, user.clone(), None, id, launch).await?;
    Ok(())
}

async fn stop_chat(s: &AppState, token: &TokenUser, args: &Value) -> Result<Value, ApiError> {
    let id = chat_id(args)?;
    let user = &token.user;
    s.store.get(user.id, id).await?;
    let mut tx = s.store.pool.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let stopped = sqlx::query(
        "UPDATE conversations SET generation_cancel_requested=TRUE \
         WHERE id=$1 AND owner_id=$2 AND active_generation IS NOT NULL",
    )
    .bind(id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    tx.commit().await?;
    if stopped {
        audit(s, token, "mcp_chat_stopped", id).await?;
    }
    Ok(json!({"id":id,"stopped":stopped}))
}

async fn audit(s: &AppState, token: &TokenUser, action: &str, chat: Uuid) -> Result<(), ApiError> {
    let mut tx = s.store.pool.begin().await?;
    record_audit(
        &mut tx,
        Some(token.user.id),
        action,
        "conversation",
        Some(&chat.to_string()),
        json!({"apiToken":token.id}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
