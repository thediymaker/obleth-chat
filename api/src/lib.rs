pub mod agents;
pub mod appearance;
pub mod apps;
pub mod attachments;
pub mod audit;
pub mod auth;
pub mod autopilot;
pub mod background;
pub mod banner;
pub mod batches;
pub mod board;
pub mod browser;
pub mod code;
pub mod compare;
pub mod config;
pub mod connections;
pub mod decisions;
pub mod error;
pub mod feedback;
pub mod files;
pub mod gateway;
pub mod groups;
mod http;
pub mod images;
pub mod imports;
pub mod insights;
pub mod knowledge;
pub mod lifecycle;
pub mod mcp;
pub mod memory;
pub mod metrics;
pub mod models;
pub mod notebook;
pub mod operations;
pub mod paging;
pub mod pause;
pub mod project_log;
pub mod projects;
pub mod questions;
pub mod research;
pub mod retention;
pub mod sandbox;
pub mod scratch;
pub mod secrets;
pub mod sent_feedback;
pub mod shares;
pub mod steer;
pub mod store;
pub mod suggestions;
mod turn;
pub mod ui;
pub mod worker;

#[cfg(test)]
mod test_support;
pub mod workspace_usage;

use turn::{
    no_such_tool,
    parse::{accumulate_tool_calls, draft_of, text_tool_calls, unique_call_ids, ToolCall},
    tool_result,
    trim::{drop_old_steps, trim_tool_history},
    ONE_AT_A_TIME,
};

use auth::{Auth, Identity};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{any, get, post},
    Json, Router,
};
use error::ApiError;
use futures_util::StreamExt;
use gateway::{Decoder, Gateway};
use lifecycle::TurnEnd;
use metrics::Metrics;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    convert::Infallible,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use store::{ReasoningEffort, Store, Turn};
use tokio::sync::{mpsc, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub auth: Auth,
    pub store: Store,
    pub gateway: Option<Gateway>,
    pub metrics: Metrics,
    pub slots: Arc<Semaphore>,
    pub shutdown: CancellationToken,
    pub admin_gateway_url: Option<String>,
    /// The web interface server (OBLETH_CHAT_UI_URL); without one Chat serves its API
    /// and the branding.
    pub ui_url: Option<url::Url>,
    pub followups_enabled: bool,
    pub sandboxes: Option<sandbox::Sandboxes>,
    /// Why workspaces are off, for administrators.
    pub workspaces_off: Option<String>,
    pub browsers: Option<browser::Browsers>,
    pub connections: Option<connections::Connections>,
    pub knowledge: Option<knowledge::Knowledge>,
    /// Wakes waits on this replica, and on the others once its listener runs.
    pub wakeups: connections::wakeups::Wakeups,
    pub decisions: decisions::Decisions,
}
impl AppState {
    pub fn new(auth: Auth, pool: PgPool, gateway: Option<Gateway>, max_streams: usize) -> Self {
        Self {
            auth,
            decisions: decisions::Decisions::new(pool.clone(), None),
            store: Store::new(pool),
            gateway,
            metrics: Metrics::new(),
            slots: Arc::new(Semaphore::new(max_streams)),
            shutdown: CancellationToken::new(),
            admin_gateway_url: None,
            ui_url: None,
            followups_enabled: true,
            sandboxes: None,
            workspaces_off: None,
            browsers: None,
            connections: None,
            knowledge: None,
            wakeups: connections::wakeups::Wakeups::default(),
        }
    }
}
pub fn router(state: AppState, branding_dir: PathBuf) -> Router {
    let auth = state.auth.router();
    let catalog = models::router(state.clone());
    let operations = operations::router(state.clone());
    let retention = retention::router(state.clone());
    let banner = banner::router(state.clone());
    let compare = compare::router(state.clone());
    let appearance = appearance::router(state.clone());
    let attachments = attachments::router(state.clone());
    let images = images::router(state.clone());
    let projects = projects::router(state.clone());
    let board = board::router(state.clone());
    let groups = groups::router(state.clone());
    let shares = shares::router(state.clone());
    let code = code::router(state.clone());
    let research = research::router(state.clone());
    let workspaces = sandbox::router(state.clone());
    let browsing = browser::router(state.clone());
    let connections = connections::router(state.clone());
    let steering = steer::router(state.clone());
    let knowledge = knowledge::router(state.clone());
    let embedding = knowledge::embedding::router(state.clone());
    let decisions = decisions::router(state.clone());
    let feedback = feedback::router(state.clone());
    let sent_feedback = sent_feedback::router(state.clone());
    let insights = insights::router(state.clone());
    let workspace_usage = workspace_usage::router(state.clone());
    let apps = apps::router(state.clone());
    let mcp = mcp::router(state.clone());
    let memory = memory::router(state.clone());
    let project_log = project_log::router(state.clone());
    let notebook = notebook::router(state.clone());
    let batches = batches::router(state.clone());
    let scratch = scratch::router(state.clone());
    let metrics = state.metrics.clone();
    let sandboxes = state.sandboxes.clone();
    let ui_url = state.ui_url.clone();
    let _ = CSP.set(content_security_policy(sandboxes.as_ref()));
    Router::new()
        .route(
            "/healthz",
            get(|| async { Json(json!({"status":"ok","mode":"connected"})) }),
        )
        .route("/readyz", get(ready))
        .route("/metrics", get(metrics_endpoint))
        .route("/.well-known/jwks.json", get(jwks))
        .route("/api/capabilities", get(capabilities))
        .route("/api/conversations", get(list).post(create))
        .route(
            "/api/conversations/{id}",
            get(detail).patch(rename).delete(delete),
        )
        .route("/api/conversations/{id}/branch", post(branch))
        .route("/api/conversations/{id}/leave", post(leave))
        .route("/api/conversations/{id}/fork", post(fork_turn))
        .route("/api/conversations/{id}/branches", get(branches))
        .route("/api/conversations/bulk", post(bulk))
        .route("/api/conversations/export", post(export))
        .route("/api/conversations/import", post(import))
        .route("/api/conversations/{id}/messages", post(send))
        .route("/api/conversations/{id}/stop", post(lifecycle::stop))
        .route(
            "/api/conversations/{id}/messages/{message}/select",
            post(select_version),
        )
        .route(
            "/api/conversations/{id}/messages/{message}/versions",
            get(reply_versions),
        )
        .route("/api/{*rest}", any(|| async { ApiError::not_found() }))
        .with_state(state)
        .merge(auth)
        .merge(catalog)
        .merge(operations)
        .merge(retention)
        .merge(banner)
        .merge(compare)
        .merge(appearance)
        .merge(attachments)
        .merge(images)
        .merge(projects)
        .merge(board)
        .merge(groups)
        .merge(shares)
        .merge(code)
        .merge(research)
        .merge(workspaces)
        .merge(browsing)
        .merge(connections)
        .merge(steering)
        .merge(knowledge)
        .merge(embedding)
        .merge(decisions)
        .merge(feedback)
        .merge(sent_feedback)
        .merge(insights)
        .merge(workspace_usage)
        .merge(apps)
        .merge(mcp)
        .merge(memory)
        .merge(project_log)
        .merge(notebook)
        .merge(batches)
        .merge(scratch)
        .fallback_service(web(branding_dir, ui_url, sandboxes.as_ref()))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn(error::envelope))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(metrics, metrics::record))
        // Outermost: preview hosts are answered before any Chat route or header policy.
        .layer(middleware::from_fn_with_state(
            sandboxes,
            sandbox::preview::dispatch,
        ))
}
static CSP: std::sync::OnceLock<HeaderValue> = std::sync::OnceLock::new();
/// Chat may frame its workspace previews, which live on their own origins, and hand
/// a preview its entry ticket.
/// What the web app draws from a reply, so models use it when it helps.
const FORMATS_PROMPT: &str = "Chat shows your reply as Markdown and draws: math in $...$ or $$...$$ (KaTeX, with \\ce{} for chemistry); diagrams and charts in ```mermaid blocks (flowchart, sequence, class, state, ER, gantt, pie, xychart for bar and line charts, sankey, mindmap, timeline); molecules in ```smiles blocks, one SMILES per line, optionally followed by a name; drawings in ```svg blocks; and data in ```csv blocks, shown as a table. Use one when it shows something more clearly than prose.";

fn content_security_policy(sandboxes: Option<&sandbox::Sandboxes>) -> HeaderValue {
    let previews = sandboxes.and_then(|sb| {
        let c = sb.config();
        Some(format!(
            "{}://*.{}",
            c.preview_scheme,
            c.preview_domain.as_ref()?
        ))
    });
    let (connect, frames) = match &previews {
        Some(previews) => (format!(" {previews}"), format!(" frame-src {previews};")),
        None => Default::default(),
    };
    HeaderValue::from_str(&format!("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'{connect};{frames} base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'"))
        .expect("static policy")
}
/// The web interface: the Next.js server that OBLETH_CHAT_UI_URL names, with the
/// site's branding from its own folder. Without one, Chat answers its API and the
/// branding, as when `next dev` runs in front of it.
fn web(
    branding_dir: PathBuf,
    ui_url: Option<url::Url>,
    sandboxes: Option<&sandbox::Sandboxes>,
) -> Router {
    let previews = sandboxes.and_then(|sb| {
        let c = sb.config();
        Some(format!(
            "{}://*.{}",
            c.preview_scheme,
            c.preview_domain.as_ref()?
        ))
    });
    match ui_url.map(|base| ui::Ui::new(base, previews)) {
        Some(Ok(ui)) => return ui.router(branding_dir),
        None => tracing::warn!(
            "OBLETH_CHAT_UI_URL is not set, so Chat serves its API without the web interface"
        ),
        Some(Err(error)) => tracing::error!(%error, "the web interface is off"),
    }
    ui::branding(branding_dir).fallback(|| async {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "The web interface isn't set up. Point OBLETH_CHAT_UI_URL at Chat's interface server.",
        )
    })
}
/// Defaults only: a handler's own header (such as the attachment sandbox CSP)
/// is kept, and any response that did not choose a cache policy is no-store.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (key, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        headers
            .entry(key)
            .or_insert(HeaderValue::from_static(value));
    }
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert_with(|| CSP.get_or_init(|| content_security_policy(None)).clone());
    response
}
async fn ready(State(s): State<AppState>) -> impl IntoResponse {
    match tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&s.store.pool),
    )
    .await
    {
        Ok(Ok(_)) if !s.shutdown.is_cancelled() => (
            StatusCode::OK,
            Json(json!({"status":"ready","mode":"connected"})),
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"not_ready"})),
        ),
    }
}
/// Prometheus metrics. With OBLETH_CHAT_METRICS_TOKEN set, only a scraper sending
/// that token as a bearer credential may read them.
async fn metrics_endpoint(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    if !s.metrics.allows(&headers) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Metrics need the configured bearer token. Without one they are served only to requests that reach Chat directly, not through a proxy",
        ));
    }
    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        s.metrics.render(),
    ))
}
async fn capabilities(State(s): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let mut value = json!({"application":"obleth-chat","mode":"connected","liveExecution":s.sandboxes.is_some(),"integrations":{"gateway":{"configured":s.gateway.is_some()},"oidc":{"configured":s.auth.oidc_enabled()},"rag":{"configured":s.knowledge.is_some()},"kubernetes":{"configured":s.sandboxes.is_some()},"slurm":{"configured":false}},"features":{"localAuth":s.auth.password_sign_in(),"durableConversations":true,"streaming":s.gateway.is_some(),"metrics":true,"modelAccess":true,"history":true,"savedPreferences":true,"workspaces":false,"connections":s.connections.is_some(),"clusters":s.connections.as_ref().is_some_and(|c| c.clusters_enabled()),"devices":s.connections.as_ref().is_some_and(|c| c.devices_enabled()),"jobOutput":s.connections.as_ref().is_some_and(|c| c.clusters_enabled() && c.job_output_available()),"autoMode":s.connections.as_ref().is_some_and(|c| c.auto_mode_allowed()),"knowledge":s.knowledge.is_some() && s.gateway.is_some()}});
    value["features"]["memory"] = json!(memory::available(&s).await);
    value["features"]["batches"] = json!(s.gateway.is_some());
    let user = s.auth.require(&headers, false).await.ok();
    // The project log: on this server, and on for this person.
    let log = project_log::available(&s).await;
    value["features"]["log"] = json!(log);
    value["features"]["myLog"] = json!(match &user {
        Some(user) if log => project_log::choice(&s.store.pool, user.id)
            .await
            .is_ok_and(|c| c.enabled),
        _ => false,
    });
    if user.as_ref().is_some_and(|u| u.is_admin()) {
        value["adminGatewayUrl"] = json!(s.admin_gateway_url);
    }
    if let (Some(sb), Some(user)) = (&s.sandboxes, &user) {
        value["features"]["workspaces"] = json!(sb.allowed(user));
        value["features"]["previews"] = json!(sb.config().preview_domain.is_some());
        value["features"]["code"] = json!(sb.allowed(user));
    }
    value["features"]["browser"] = json!(match (&s.browsers, &user) {
        (Some(b), Some(user)) => b.allowed(user),
        _ => false,
    });
    // How the sidebar shows the Feedback button: off, sidebar or highlighted.
    value["feedback"] = json!(match &user {
        Some(_) => sent_feedback::mode(&s.store.pool)
            .await
            .unwrap_or_else(|_| "sidebar".into()),
        None => "off".into(),
    });
    Json(value)
}
async fn jwks(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        s.gateway
            .as_ref()
            .ok_or_else(gateway::not_configured)?
            .jwks(),
    ))
}
async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<store::HistoryQuery>,
) -> Result<Json<store::HistoryPage>, ApiError> {
    let u = s.auth.require(&headers, false).await?;
    Ok(Json(s.store.history(u.id, query).await?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Create {
    title: String,
    model: String,
    #[serde(default)]
    reasoning_effort: ReasoningEffort,
    #[serde(default)]
    temporary: bool,
}
async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Create>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let u = s.auth.require(&headers, true).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"conversation":s.store.create_chat(u.id,&body.title,&body.model,body.reasoning_effort,body.temporary).await?}),
        ),
    ))
}
/// The chat as its owner reads it. The ETag hashes the whole answer, so a watcher that
/// sends back the tag of its copy learns the chat is unchanged without the transcript.
async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let u = s.auth.require(&headers, false).await?;
    let body = serde_json::to_vec(&json!({"conversation":s.store.get(u.id,id).await?}))
        .map_err(|_| ApiError::internal())?;
    let tag = format!("\"{:x}\"", Sha256::digest(&body));
    let response = if shares::unchanged(&headers, &tag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, "application/json")], body).into_response()
    };
    shares::tagged(response, &tag)
}
async fn rename(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<store::ConversationPatch>,
) -> Result<Json<Value>, ApiError> {
    let u = s.auth.require(&headers, true).await?;
    Ok(Json(
        json!({"conversation":s.store.patch(u.id,id,body).await?}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Fork {
    message_id: Uuid,
    mode: String,
    content: Option<String>,
}
async fn fork_turn(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Fork>,
) -> Result<(StatusCode, Json<store::ForkResult>), ApiError> {
    let u = s.auth.require(&headers, true).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.store
                .fork_turn(
                    u.id,
                    id,
                    body.message_id,
                    &body.mode,
                    body.content.as_deref(),
                )
                .await?,
        ),
    ))
}
async fn branches(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let u = s.auth.require(&headers, false).await?;
    Ok(Json(
        json!({"conversations":s.store.branches(u.id,id).await?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bulk {
    ids: Vec<Uuid>,
    action: String,
}
async fn bulk(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Bulk>,
) -> Result<StatusCode, ApiError> {
    let u = s.auth.require(&headers, true).await?;
    s.store.bulk(u.id, &body.ids, &body.action).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    ids: Vec<Uuid>,
}
async fn export(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Selection>,
) -> Result<Json<store::ChatArchive>, ApiError> {
    let u = s.auth.require(&headers, true).await?;
    Ok(Json(s.store.export(u.id, &body.ids).await?))
}
/// Takes the export file as it is: Chat's own JSON, Open WebUI's JSON, or a ChatGPT
/// or Claude.ai export zip or its conversations.json. The body is read only after
/// sign-in is checked, since it may be up to 200 MB.
async fn import(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<(StatusCode, Json<store::ImportSummary>), ApiError> {
    let (u, body) = error::admitted(body, s.auth.require(&headers, true).await).await?;
    // Waits its turn before reading the body, so waiting uploads hold no memory.
    let _turn = imports::RUNNING
        .acquire()
        .await
        .map_err(|_| ApiError::internal())?;
    let body = error::upload(
        body,
        imports::MAX_IMPORT_BYTES,
        "Choose an export of 200 MB or less",
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(s.store.import_upload(u.id, body).await?),
    ))
}
async fn delete(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let u = s.auth.require(&headers, true).await?;
    s.store.delete(u.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
/// The person left a temporary chat: it goes now, or once its reply has stopped.
async fn leave(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let u = s.auth.require(&headers, true).await?;
    s.store.leave_temporary(u.id, id).await?;
    // A reply still running stops now, not at its next check.
    s.wakeups
        .send(&s.store.pool, &lifecycle::stop_key(id))
        .await;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Branch {
    through_message_id: Uuid,
}
async fn branch(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Branch>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let u = s.auth.require(&headers, true).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"conversation":s.store.branch(u.id,id,body.through_message_id).await?})),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Send {
    #[serde(default)]
    content: String,
    model: String,
    #[serde(default)]
    attachment_ids: Vec<Uuid>,
    system_prompt: Option<String>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    max_tokens: Option<i32>,
    reasoning_effort: Option<ReasoningEffort>,
    regenerate_message_id: Option<Uuid>,
    edit_message_id: Option<Uuid>,
}
async fn select_version(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, message)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    Ok(Json(
        json!({"conversation":s.store.select_version(user.id,id,message).await?}),
    ))
}
/// A reply's versions with their text, for comparing them.
async fn reply_versions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, message)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    Ok(Json(
        json!({"versions":s.store.reply_versions(user.id,id,message).await?}),
    ))
}
async fn send(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(mut body): Json<Send>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let gateway = s.gateway.clone().ok_or_else(gateway::not_configured)?;
    if s.shutdown.is_cancelled() {
        return Err(ApiError::unavailable("Service is shutting down"));
    }
    if body.regenerate_message_id.is_some() && body.edit_message_id.is_some() {
        return Err(ApiError::bad("Choose either edit or regenerate"));
    }
    let variant = body
        .regenerate_message_id
        .map(|message| (message, "regenerate"))
        .or_else(|| body.edit_message_id.map(|message| (message, "edit")));
    // Only settings are needed before model resolution; start_turn_inner performs
    // the turn's single locked transcript load and every admission check.
    let saved = if let Some((message, mode)) = variant {
        let context = s.store.variant_context(user.id, id, message, mode).await?;
        if mode == "regenerate" {
            body.content = context.draft;
            body.attachment_ids = context.attachments.iter().map(|a| a.id).collect();
        }
        context.settings
    } else {
        s.store.saved_settings(user.id, id).await?
    };
    let saved: models::ParameterOverrides =
        serde_json::from_value(saved).map_err(|_| ApiError::internal())?;
    let overrides = models::ParameterOverrides {
        system_prompt: body.system_prompt.or(saved.system_prompt),
        temperature: body.temperature.or(saved.temperature),
        top_p: body.top_p.or(saved.top_p),
        max_tokens: body.max_tokens.or(saved.max_tokens),
        reasoning_effort: body.reasoning_effort.or(saved.reasoning_effort),
    };
    let launch = Launch {
        content: body.content,
        model: body.model,
        attachment_ids: body.attachment_ids,
        overrides,
        variant,
    };
    // The person writing again gives Auto mode's watchdog a fresh count.
    let _ = sqlx::query("UPDATE conversations SET auto_nudges=0,auto_nudged_at=NULL WHERE id=$1 AND owner_id=$2 AND auto_nudges>0")
        .bind(id)
        .bind(user.id)
        .execute(&s.store.pool)
        .await;
    let rx = launch_turn(s, gateway, user, Started::Session(headers), id, launch).await?;
    let mut response = Sse::new(ReceiverStream::new(rx))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response();
    let headers = response.headers_mut();
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    // Sse sets no-cache, which security_headers would keep; a transcript is no-store.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// What a turn starts from: the person's message and settings, or a nudge.
pub(crate) struct Launch {
    pub content: String,
    pub model: String,
    pub attachment_ids: Vec<Uuid>,
    pub overrides: models::ParameterOverrides,
    pub variant: Option<(Uuid, &'static str)>,
}

/// What started a turn, which is checked again while the turn runs: ending the
/// sign-in, or revoking the token, stops what it began.
pub(crate) enum Started {
    /// A person at a browser, by their request's headers.
    Session(HeaderMap),
    /// An API token, by its id.
    Token(Uuid),
    /// Chat itself, as Auto mode's nudges do: it runs for no session.
    Alone,
}

/// Starts a turn and its background worker.
pub(crate) async fn launch_turn(
    s: AppState,
    gateway: Gateway,
    user: Identity,
    starter: Started,
    id: Uuid,
    launch: Launch,
) -> Result<mpsc::Receiver<Result<Event, Infallible>>, ApiError> {
    let Launch {
        content,
        model,
        attachment_ids,
        overrides,
        variant,
    } = launch;
    // A send Chat has no room for is refused before any catalog or tool discovery.
    let permit = s
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::too_many("Chat is at capacity. Try again shortly."))?;
    let resolved =
        models::resolve_request(&s.store.pool, &gateway, &user, &model, overrides).await?;
    let parameters = &resolved.parameters;
    // A conversation with a workspace gives the model its tools. Catalog metadata
    // often under-reports tool support, so the explicit workspace choice decides; a
    // model that cannot call tools answers without them or reports the error.
    let workspace = match &s.sandboxes {
        Some(sb) if sb.allowed(&user) => sb.get(user.id, id).await?,
        _ => None,
    };
    // Connection tools go to every chat whose model may call tools.
    // Without any tool to offer, the model is still told about connections the person
    // has that this chat cannot use yet, so it can say what is missing.
    let (toolset, unready) = match &s.connections {
        Some(c) if resolved.capabilities.supports_function_calling != Some(false) => {
            let set = c.toolset(user.id, id).await.with_actions(resolved.actions);
            if set.is_empty() {
                (None, set.unready_prompt())
            } else {
                (Some(set), None)
            }
        }
        _ => (None, None),
    };
    let turn = if let Some((message, mode)) = variant {
        s.store
            .start_variant_turn(
                user.id,
                id,
                &content,
                &resolved.model,
                &parameters.system_prompt,
                parameters.temperature,
                parameters.max_tokens,
                Some(parameters.reasoning_effort),
                parameters.top_p,
                &model,
                &attachment_ids,
                resolved.capabilities.supports_vision == Some(true),
                message,
                mode,
            )
            .await?
    } else {
        s.store
            .start_turn_attached(
                user.id,
                id,
                &content,
                &resolved.model,
                &parameters.system_prompt,
                parameters.temperature,
                parameters.max_tokens,
                Some(parameters.reasoning_effort),
                parameters.top_p,
                &model,
                &attachment_ids,
                resolved.capabilities.supports_vision == Some(true),
            )
            .await?
    };
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(128);
    tokio::spawn(async move {
        run_generation(
            s, gateway, user, starter, id, resolved, turn, tx, permit, workspace, toolset, unready,
        )
        .await;
    });
    Ok(rx)
}
/// How often a running reply checks that the sign-in or token that started it is live.
const SESSION_CHECK_EVERY: Duration = Duration::from_secs(15);
/// How long the model may send nothing before a tool turn pauses. A tool turn has
/// no step or time limit: it runs while the model keeps answering, until it is
/// done or stopped.
const STALL_SECS: u64 = 10 * 60;
/// How a paused tool turn's note ends. Auto mode resumes a reply that ends with it.
pub(crate) const PAUSED: &str = "Say \"continue\" to pick up where it left off._";
/// A chat in Auto mode runs unattended: a step the model stalled on is retried
/// this many times before the turn pauses.
const UNATTENDED_STALL_RETRIES: usize = 2;
/// What Chat tells a model that ends a reply right after changing files.
const CHECK_NOTE: &str = "[Chat: you changed files and have not run anything since, and your draft reply was not shown. Check the work the way the person would before answering: run its tests or build, or make sure its server is running and request the page or endpoint (for example with curl), and fix what fails. Then write your full final reply. If it cannot be checked here, say why in one line.]";
/// Steps in a row that may stop at the output limit before the turn gives up.
const MAX_CUTS: u32 = 3;
/// Steps in a row that may lose their connection to the model and be tried again,
/// whether the stream broke off or the request found the gateway failing.
const MAX_DROPS: u32 = 2;
/// How long a step waits before sending a request the gateway failed again.
const RETRY_WAIT: Duration = Duration::from_secs(1);

/// Bytes a tool step may think before it writes anything. Past it, the step is
/// stopped and tried again at low effort, up to MAX_OVERTHINKS times in a row:
/// some models plan a whole program in their thinking and never start writing it.
const STEP_THINKING_BYTES: usize = 64 * 1024;
const MAX_OVERTHINKS: u32 = 2;
const OVERTHOUGHT: &str = "[Chat: you thought for a long time without writing anything, so that step was stopped. Stop planning everything up front. Take the next small step now, such as writing the first part of a file with write_file, then continue in parts and check as you go.]";
/// Writes to one file, with nothing run in between, that count as a loop.
const LOOP_WRITES: u32 = 4;
/// Times a model may put back a version of a file it replaced before that
/// counts as a loop.
const LOOP_REVERTS: u32 = 2;

/// Watches a turn's file writes for a model going in circles: writing one file
/// over and over with nothing run in between, or putting back a version of it
/// that it already replaced (two manifests in turn, each "fixing" the other).
#[derive(Default)]
struct LoopWatch {
    /// Writes per file since the model last ran something.
    rewrites: std::collections::HashMap<String, u32>,
    /// Each file's versions this turn, as content fingerprints.
    versions: std::collections::HashMap<String, Vec<u64>>,
    /// Times each file went back to a version it had already left.
    reverts: std::collections::HashMap<String, u32>,
}

impl LoopWatch {
    fn wrote(&mut self, path: &str, content: Option<&str>) {
        *self.rewrites.entry(path.to_owned()).or_default() += 1;
        let Some(text) = content else { return };
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        let hash = h.finish();
        let seen = self.versions.entry(path.to_owned()).or_default();
        if seen.last() != Some(&hash) && seen.contains(&hash) {
            *self.reverts.entry(path.to_owned()).or_default() += 1;
        }
        seen.push(hash);
    }

    /// The model ran something, which is how it checks what it wrote.
    fn ran(&mut self) {
        self.rewrites.clear();
    }

    /// The file the model is stuck on and how many times it wrote it this turn,
    /// once. Counting starts over after a loop is reported.
    fn looping(&mut self) -> Option<(String, u32)> {
        let path = self
            .rewrites
            .iter()
            .filter(|(_, n)| **n >= LOOP_WRITES)
            .chain(self.reverts.iter().filter(|(_, n)| **n >= LOOP_REVERTS))
            .map(|(path, _)| path.clone())
            .min()?;
        let times = self
            .versions
            .get(&path)
            .map_or(LOOP_WRITES as usize, Vec::len) as u32;
        self.rewrites.clear();
        self.reverts.clear();
        Some((path, times))
    }
}
/// What Chat tells a model that keeps rewriting one file. Models that loop
/// this way often think something else is changing the file.
fn loop_note(path: &str, times: u32) -> String {
    format!("[Chat: you have written {path} {times} times this turn without getting anywhere. Nothing else changes your files: what you read back is exactly what you last wrote. Stop rewriting {path}. Decide what it should hold once, then write the next file the work needs, and build or run it to check.]")
}
/// Shows the card a suggestion tool made and tells the model its reply ends there
/// (`shown`). `card` is None when the tool is not offered this turn. A reply ends
/// with one card at most, and a call whose arguments need fixing is told what to fix.
fn show_card(
    card: Option<Result<Value, String>>,
    shown: &str,
    name: &str,
    asked: &mut bool,
    activity: &mut Vec<Value>,
    tx: &mut Option<mpsc::Sender<Result<Event, Infallible>>>,
) -> String {
    match card {
        Some(Ok(entry)) if !*asked => {
            *asked = true;
            record(activity, &entry, true);
            emit(tx, "tool", entry);
            shown.to_string()
        }
        Some(Ok(_)) => ONE_AT_A_TIME.to_string(),
        Some(Err(problem)) => problem,
        None => no_such_tool(name),
    }
}

/// Adds a tool step to the activity log, or updates the step just added.
fn record(activity: &mut Vec<Value>, entry: &Value, new: bool) {
    match activity.last_mut() {
        Some(last) if !new => *last = entry.clone(),
        _ => {
            if activity.len() >= 300 {
                activity.remove(0);
            }
            activity.push(entry.clone());
        }
    }
}

fn emit(tx: &mut Option<mpsc::Sender<Result<Event, Infallible>>>, name: &str, data: Value) {
    // A slow or disconnected browser must never hold up model consumption.
    // Closing this subscriber makes it reconcile from durable checkpoints.
    if tx.as_ref().is_some_and(|sender| {
        sender
            .try_send(Ok(Event::default().event(name).data(data.to_string())))
            .is_err()
    }) {
        *tx = None;
    }
}
/// Turns on a chat's workspace from within a turn, as its Workspace button does: a
/// project chat's starts with the project's code. Returns it running.
pub(crate) async fn launch_workspace(
    s: &AppState,
    user: &Identity,
    chat: Uuid,
) -> Result<(sandbox::Sandbox, sandbox::Sandboxes), String> {
    let sandboxes = s.sandboxes.clone().ok_or("Workspaces are not available")?;
    let sandbox = sandboxes.create(user.id, chat).await.map_err(|e| e.1)?;
    let running = match code::seed(&s.store.pool, &sandboxes, user, chat, &sandbox).await {
        Ok(Some(running)) => running,
        Ok(None) => sandboxes.ensure_running(&sandbox).await?,
        Err(e) => {
            return Err(format!(
                "Could not put the project's code in the workspace: {}",
                e.1
            ))
        }
    };
    Ok((running, sandboxes))
}

/// Copies the chat's attached files into its running workspace. A file that could not
/// be copied is tried again next turn.
async fn copy_uploads(
    s: &AppState,
    uploads: &attachments::workspace::Uploads,
    sandboxes: &sandbox::Sandboxes,
    sandbox: &sandbox::Sandbox,
) {
    let pod = code::workspace::Pod { sandboxes, sandbox };
    if let Err(error) = uploads.copy(&pod, &s.store.pool, &s.store.files).await {
        tracing::warn!("could not copy a chat's files into its workspace: {error}");
    }
}

/// Longest a turn waits for its long documents to finish indexing before it searches
/// what is ready. Indexing starts on upload, so this is rarely needed.
const ATTACHED_WAIT: Duration = Duration::from_secs(120);

/// The collections of long documents in the turn's messages, once their indexing has
/// finished or ATTACHED_WAIT has passed. The turn's lease is renewed meanwhile.
async fn attached_documents(
    s: &AppState,
    k: &knowledge::Knowledge,
    user: &Identity,
    id: Uuid,
    turn: &Turn,
) -> Vec<Uuid> {
    let mut files: Vec<Uuid> = turn
        .attachments
        .values()
        .flatten()
        .filter(|f| f.searched())
        .map(|f| f.meta.id)
        .collect();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return vec![];
    }
    let collections = match k.attached(user.id, id, &files).await {
        Ok(collections) => collections,
        Err(error) => {
            tracing::warn!("could not find a chat's attached documents: {}", error.1);
            return vec![];
        }
    };
    let started = Instant::now();
    while started.elapsed() < ATTACHED_WAIT && k.indexing(&collections).await {
        let lease = lifecycle::heartbeat(&s.store.pool, user.id, id, turn.generation_id).await;
        if !matches!(lease, Ok(None)) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    collections
}

#[allow(clippy::too_many_arguments)]
async fn run_generation(
    s: AppState,
    gateway: Gateway,
    user: Identity,
    starter: Started,
    id: Uuid,
    resolved: models::ResolvedRequest,
    turn: Turn,
    tx: mpsc::Sender<Result<Event, Infallible>>,
    permit: tokio::sync::OwnedSemaphorePermit,
    workspace: Option<sandbox::Sandbox>,
    mut toolset: Option<connections::Toolset>,
    unready: Option<String>,
) {
    let workspace_turn = workspace.is_some();
    // Getting ready below searches, reads the workspace and copies files into it before
    // the turn's own heartbeat (`control`) begins. The lease is kept meanwhile, so a
    // slow start does not let it run out under the reply.
    let preparing = lifecycle::keep_lease(
        s.store.pool.clone(),
        user.id,
        id,
        turn.generation_id,
        lifecycle::LEASE_RENEW_INTERVAL,
    );
    // The chat's project: instructions, and collections and saved items it also searches.
    let project = projects::for_conversation(&s.store.pool, user.id, id).await;
    // Passages from the chat's knowledge collections, and what Chat remembers about the
    // person that fits this message, found together before the model starts. Both
    // search for the same question, which is embedded once.
    let question = knowledge::chat::Question::new(
        &turn
            .messages
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect::<Vec<_>>(),
    );
    let knowledge_search = async {
        let k = s.knowledge.as_ref()?;
        // A notebook chat searches only the documents its owner ticked.
        let notebook = notebook::restriction(&s.store.pool, user.id, id)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!("could not read a notebook chat's sources: {error}");
                None
            });
        let attached = match notebook {
            Some(_) => vec![],
            None => attached_documents(&s, k, &user, id, &turn).await,
        };
        let tools = resolved.capabilities.supports_function_calling != Some(false);
        let library = resolved.capabilities.supports_function_calling == Some(true);
        k.prepare(
            &gateway,
            &user,
            id,
            &attached,
            &question,
            tools,
            library,
            project.as_ref(),
            notebook,
        )
        .await
    };
    let (mut knowledge_turn, memory_turn) = tokio::join!(
        knowledge_search,
        memory::recall(&s, &gateway, &user, id, &question)
    );
    // Models with tools may ask the person to choose before going on.
    let asks = resolved.capabilities.supports_function_calling == Some(true);
    // A chat in a project can read the person's log of their work there.
    let log_tool = asks && project.is_some() && project_log::on_for(&s, user.id).await;
    // A chat with the browser on gets the browser tools and its owner's search engine.
    let browser_allowed = s.browsers.as_ref().is_some_and(|b| b.allowed(&user));
    let browsing = match &s.browsers {
        Some(b)
            if browser_allowed
                && resolved.capabilities.supports_function_calling != Some(false)
                && b.is_on(user.id, id).await =>
        {
            let engine = b
                .get(user.id)
                .await
                .ok()
                .flatten()
                .map_or_else(|| "duckduckgo".to_owned(), |b| b.search_engine);
            Some((b.clone(), engine))
        }
        _ => None,
    };
    // Worker models the main model can hand work to, chosen by administrators. Any
    // chat can have them; files come from the workspace, which Chat turns on if needed.
    let workers = if resolved.capabilities.supports_function_calling != Some(false) {
        models::worker_models(&s.store.pool, &user).await
    } else {
        vec![]
    };
    let tool_turn = workspace_turn
        || asks
        || !workers.is_empty()
        || browsing.is_some()
        || toolset.is_some()
        || knowledge_turn
            .as_ref()
            .is_some_and(|t| t.searchable() || t.library);
    // Auto mode is for work nobody watches, such as research: its tools run without
    // asking, and its turn outlives the browser session that started it.
    let auto_mode = |s: AppState| async move {
        match &s.connections {
            Some(c) => c.auto(id).await,
            None => false,
        }
    };
    let unattended = tool_turn && auto_mode(s.clone()).await;
    s.metrics.streams.inc();
    let started = Instant::now();
    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut usage = None;
    let mut request_id = None;
    // With Auto the gateway picks the model; the stream says which one it was.
    let mut served: Option<String> = None;
    // Why Auto chose it, when the gateway says so with its answer.
    let mut auto_why: Option<Value> = None;
    let mut finish_reason: Option<String> = None;
    let mut first = true;
    let generation_id = turn.generation_id;
    if let Some(set) = toolset.as_mut() {
        set.begin(generation_id);
    }
    let mut tx = Some(tx);
    // Where the chat's attached files are in its workspace, for the model.
    let uploads = match &workspace {
        Some(_) => attachments::workspace::Uploads::of(&s.store.pool, user.id, id)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!("could not list a chat's attached files: {error}");
                Default::default()
            }),
        None => Default::default(),
    };
    let mut messages = vec![];
    let parameters = &resolved.parameters;
    if !parameters.system_prompt.is_empty() {
        messages.push(json!({"role":"system","content":parameters.system_prompt}));
    }
    messages.extend(turn.messages.iter().map(|m| {
        if m.role == "assistant" {
            return json!({"role":"assistant","content":store::replay_content(m)});
        }
        let files = turn.attachments.get(&m.id);
        let content = if let Some(files) = files.filter(|files| !files.is_empty()) {
            if files.iter().all(|f| f.meta.kind != "image") {
                let mut text = m.content.clone();
                for file in files {
                    if let Some(context) = file.text_context_in(uploads.path(file.meta.id)) {
                        text.push_str(&context);
                    }
                }
                json!(text)
            } else {
                let mut parts = vec![json!({"type":"text","text":m.content})];
                parts.extend(
                    files
                        .iter()
                        .map(|file| file.content_part_in(uploads.path(file.meta.id))),
                );
                json!(parts)
            }
        } else {
            json!(m.content)
        };
        json!({"role":m.role,"content":content})
    }));
    let workspace = workspace.zip(s.sandboxes.clone());
    // The chat's files go into a running workspace now, so the model sees them listed;
    // one that sleeps gets them when its first tool wakes it.
    let mut uploads_copied = false;
    if let Some((sandbox, sandboxes)) = &workspace {
        if sandbox.status == "running" && sandbox.pod_ip.is_some() {
            copy_uploads(&s, &uploads, sandboxes, sandbox).await;
            uploads_copied = true;
        }
    }
    // Every turn knows the date, so searches about anything recent name the right year.
    let mut preamble = vec![format!(
        "# Right now\nToday is {}.",
        chrono::Utc::now().format("%A %-d %B %Y")
    )];
    if let Some(memory) = &memory_turn {
        preamble.push(memory.prompt());
    }
    if let Some(project) = &project {
        preamble.push(project.prompt());
    }
    // A chat in a project works its board: models that call tools change it, and
    // every model sees it, with the card this chat works in full.
    let board_tools = asks && project.is_some();
    if let Some(project) = &project {
        if let Some(view) =
            board::tools::prompt(&s.store.pool, user.id, project.id, id, board_tools).await
        {
            preamble.push(view);
        }
    }
    if log_tool {
        preamble.push(project_log::PROMPT.into());
    }
    if let Some((sandbox, sandboxes)) = &workspace {
        let instructions = models::agent_instructions(&s.store.pool, user.id).await;
        let context = sandbox::tools::project_context(sandboxes, sandbox).await;
        preamble.push(sandbox::tools::system_prompt(
            sandbox,
            instructions.as_deref(),
            &context,
        ));
    }
    // A workspace chat in a project can run the project's research.
    let research_standing = match (&workspace, &project) {
        (Some(_), Some(project)) => {
            research::tools::standing(&s.store.pool, user.id, id, project.id).await
        }
        _ => None,
    };
    if let (Some(standing), Some((sandbox, _))) = (&research_standing, &workspace) {
        preamble.push(research::tools::prompt(&s.store.pool, standing, sandbox).await);
    }
    let research_tools = matches!(
        research_standing,
        Some(research::tools::Standing::Absent | research::tools::Standing::Here(_))
    );
    if let Some(toolset) = &toolset {
        preamble.push(toolset.prompt());
    }
    if let Some(unready) = unready {
        preamble.push(unready);
    }
    if !workers.is_empty() {
        preamble.push(agents::PROMPT.into());
    }
    if let Some((_, engine)) = &browsing {
        preamble.push(browser::tools::prompt(engine));
    }
    // People who may have a workspace can be asked to turn one on for this chat.
    let offers_workspace =
        asks && workspace.is_none() && s.sandboxes.as_ref().is_some_and(|sb| sb.allowed(&user));
    if asks {
        preamble.push(questions::ASK_PROMPT.into());
    }
    if offers_workspace {
        preamble.push(questions::WORKSPACE_PROMPT.into());
    }
    // Research can be suggested in a chat that isn't part of a project's research.
    let offers_research = asks
        && research_standing.is_none()
        && s.sandboxes.as_ref().is_some_and(|sb| sb.allowed(&user));
    if offers_research {
        preamble.push(questions::RESEARCH_PROMPT.into());
    }
    // Administrators can be offered an app, unless the chat's project already has one;
    // the card's button uses that project when they own it.
    let app_room = match (&project, asks && user.is_admin()) {
        (_, false) => None,
        (Some(project), true) => apps::room(&s.store.pool, user.id, project.id).await,
        (None, true) => Some(false),
    };
    if app_room.is_some() {
        preamble.push(questions::APP_PROMPT.into());
    }
    // Ongoing work can be moved into a project from a saved chat that isn't in one,
    // once per chat.
    let offers_project = asks
        && project.is_none()
        && !questions::project_suggested(&turn.messages)
        && !crate::store::is_temporary(&s.store.pool, user.id, id)
            .await
            .unwrap_or(true);
    if offers_project {
        preamble.push(questions::PROJECT_PROMPT.into());
    }
    let memory_learns = memory_turn.as_ref().is_some_and(|m| m.learns());
    if let Some(prompt) = knowledge_turn
        .as_ref()
        .map(|k| k.prompt(memory_learns))
        .filter(|p| !p.is_empty())
    {
        preamble.push(prompt);
    }
    // Described only when the history holds one: some models otherwise write a note
    // of their own in place of calling a tool.
    let recalled = messages.iter().any(|m| {
        m["role"] == "assistant"
            && m["content"]
                .as_str()
                .is_some_and(|c| c.contains(store::RECORD))
    });
    preamble.push(FORMATS_PROMPT.into());
    if recalled {
        preamble.push(format!("Earlier replies in this conversation may end with a bracketed note that starts with \"{}\". Chat adds it so you remember which tools ran; you did not write it and the person does not see it. Never write such a note yourself: your tools report their own results.", store::RECORD));
    }
    // Chat's own instructions come first and the person's system prompt last; Auto's
    // classifier reads only the person's part, so tool and format instructions do not
    // make every request look like code.
    let own_system = (!preamble.is_empty()).then_some(parameters.system_prompt.len());
    if !preamble.is_empty() {
        if !parameters.system_prompt.is_empty() {
            preamble.push(parameters.system_prompt.clone());
        }
        let prompt = preamble.join("\n\n");
        if parameters.system_prompt.is_empty() {
            messages.insert(0, json!({"role":"system","content":prompt}));
        } else {
            messages[0] = json!({"role":"system","content":prompt});
        }
    }
    // No output limit unless someone set one: the model's own applies. Workspace
    // turns write whole files through tool arguments, so a set limit has a floor.
    let max_tokens = match parameters.max_tokens {
        0 => None,
        limit if workspace.is_some() => Some(limit.max(16_384)),
        limit => Some(limit),
    };
    let mut payload = json!({"model":resolved.model,"messages":messages,"temperature":parameters.temperature,"top_p":parameters.top_p,"max_tokens":max_tokens,"reasoning_effort":turn.reasoning_effort,"stream":true,"stream_options":{"include_usage":true}});
    if resolved.capabilities.supports_reasoning == Some(false) {
        payload.as_object_mut().unwrap().remove("reasoning_effort");
    }
    if max_tokens.is_none() {
        payload.as_object_mut().unwrap().remove("max_tokens");
    }
    let mut tools = vec![];
    if workspace.is_some() {
        if let Value::Array(definitions) = sandbox::tools::definitions() {
            tools.extend(definitions);
        }
        if project.is_some() {
            tools.push(code::tool_definition());
        }
    }
    let may_launch = s.sandboxes.as_ref().is_some_and(|sb| sb.allowed(&user));
    let worker_files = if workspace.is_some() {
        agents::Files::Workspace
    } else if may_launch {
        agents::Files::Launch {
            project: project.is_some(),
        }
    } else {
        agents::Files::None
    };
    if !workers.is_empty() {
        tools.push(agents::definition(&workers, worker_files));
    }
    if let Some(toolset) = &toolset {
        tools.extend(toolset.definitions());
    }
    if knowledge_turn.as_ref().is_some_and(|t| t.searchable()) {
        tools.push(knowledge::chat::definition());
    }
    if knowledge_turn.as_ref().is_some_and(|t| t.library) {
        tools.push(knowledge::chat::save_definition(memory_learns));
    }
    if board_tools {
        tools.extend(board::tools::definitions());
    }
    if log_tool {
        tools.push(project_log::definition());
    }
    if asks {
        tools.push(questions::definition());
    }
    if offers_workspace {
        tools.push(questions::workspace_definition());
    }
    // The research card's button keeps the chat's project when that project can take the research.
    let research_here = match (&project, offers_research) {
        (Some(project), true) => matches!(
            research::tools::standing(&s.store.pool, user.id, id, project.id).await,
            Some(research::tools::Standing::Absent | research::tools::Standing::Here(_))
        ),
        _ => false,
    };
    if offers_research {
        tools.push(questions::research_definition());
    }
    if app_room.is_some() {
        tools.push(questions::app_definition());
    }
    if offers_project {
        tools.push(questions::project_definition());
    }
    if research_tools {
        tools.extend(research::tools::definitions());
    }
    if browsing.is_some() {
        tools.extend(browser::tools::definitions());
        if asks {
            tools.push(browser::tools::handoff_definition());
        }
    }
    // A chat in a project with the browser on can add the page its tab shows to the
    // project's notebook, when the person asks.
    let notebook_pages = asks && browsing.is_some() && project.is_some() && s.knowledge.is_some();
    if notebook_pages {
        tools.push(notebook::web::definition());
    }
    // People who may use the browser can be asked to turn it on for this chat.
    let offers_browser = asks && browsing.is_none() && browser_allowed;
    if offers_browser {
        tools.push(browser::tools::request_definition());
    }
    // Where devices are offered, the model can ask for one plugged into the
    // person's computer.
    let offers_device = asks && s.connections.as_ref().is_some_and(|c| c.devices_enabled());
    if offers_device {
        tools.push(connections::device::request_definition());
    }
    // Long work can be paused at the person's request, with a note to carry on from.
    let offers_pause = workspace.is_some()
        || research_tools
        || browsing.is_some()
        || toolset.is_some()
        || !workers.is_empty();
    if offers_pause {
        tools.push(pause::definition());
    }
    if !tools.is_empty() {
        payload["tools"] = Value::Array(tools);
    }
    let mut activity: Vec<Value> = vec![];
    // A reply Auto chooses may also be put to the models Auto ranked next (see
    // `compare`). Its first request is kept for that while an administrator has
    // comparing on, with how many requests the reply took: only one is compared.
    let comparable = payload["model"] == "auto" && compare::watching(&s.store.pool).await;
    let mut first_request: Option<Value> = None;
    let mut requests = 0u32;
    let user_message_id = turn.messages.last().map(|m| m.id);
    emit(
        &mut tx,
        "meta",
        json!({"conversationId":id,"messageId":turn.assistant_id,"userMessageId":user_message_id,"generationId":generation_id,"requestId":request_id,"resolvedModel":resolved.model,"settings":parameters}),
    );
    if let Some(knowledge) = knowledge_turn.as_ref().filter(|k| k.has_collections()) {
        let entry = knowledge.entry();
        record(&mut activity, &entry, true);
        emit(&mut tx, "tool", entry);
    }
    if let Some(memory) = memory_turn.as_ref().filter(|m| m.recalled()) {
        let entry = memory.entry();
        record(&mut activity, &entry, true);
        emit(&mut tx, "tool", entry);
    }
    let work = async {
        let mut steps = 0;
        // Every tool call id this turn has used, so no two calls share one.
        let mut call_ids = std::collections::HashSet::new();
        let mut checkpoint = tokio::time::interval(Duration::from_millis(750));
        checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut saved_bytes = 0;
        let mut saved_reasoning = 0;
        let mut saved_at = Instant::now();
        let mut drafted = Instant::now();
        let mut checkpoints = 0;
        let mut cuts = 0;
        let mut drops = 0;
        let mut overthinks = 0;
        let mut log_reads = 0;
        // Files changed since the model last ran something, and whether Chat has
        // already asked it to check its work this turn.
        let mut unchecked = false;
        let mut gated = false;
        // Writes per file since the model last ran something, how often Chat
        // has stopped a loop this turn, and whether the next request asks
        // `auto` for a stronger model.
        let mut watch = LoopWatch::default();
        let mut loops = 0u32;
        // A chat on a model not certified for changes is offered a certified one when
        // it first reaches for a cluster or workspace tool, once per chat.
        let mut model_offered = false;
        let mut escalate: Option<&'static str> = None;
        // The note the model paused with; the reply ends after that round.
        let mut paused: Option<(pause::Note, bool)> = None;
        loop {
            steps += 1;
            finish_reason = None;
            let step_start = content.len();
            // Where this step's thinking starts, in UTF-16 units as the browser counts.
            let thought_from = reasoning_content.encode_utf16().count();
            let reasoning_start = reasoning_content.len();
            let mut overthought = false;
            let round_began = Instant::now();
            if steps > 1 && tool_turn {
                // Notes the person sent while the model worked, read at the step boundary.
                for (note, text) in steer::take(&s.store.pool, generation_id).await {
                    payload["messages"].as_array_mut().unwrap().push(json!({"role":"user","content":format!("[A note from the person while you work: {text}]")}));
                    let entry = json!({"id":format!("note-{note}"),"kind":"note","summary":text,"status":"completed","startedAt":chrono::Utc::now()});
                    record(&mut activity, &entry, true);
                    emit(&mut tx, "tool", entry);
                }
            }
            requests += 1;
            if comparable && requests == 1 {
                first_request = Some(payload.clone());
            }
            let mut calls: Vec<ToolCall> = vec![];
            let mut stalls = 0;
            // When this request was sent, and when it wrote its first and latest token:
            // the reply's first-token wait and how fast it wrote.
            let asked = Instant::now();
            let (mut wrote_first, mut wrote_last): (Option<Instant>, Option<Instant>) =
                (None, None);
            let response = loop {
                match tokio::time::timeout(
                    Duration::from_secs(STALL_SECS),
                    gateway.chat_escalating(&user, id, &payload, escalate, own_system),
                )
                .await
                {
                    Ok(Ok(response)) => break response,
                    // The gateway could not be reached or failed: a drop, sent again as is.
                    Ok(Err(gateway::ChatFailure::Transient(error))) if drops < MAX_DROPS => {
                        drops += 1;
                        tracing::warn!(
                            status = error.0.as_u16(),
                            "gateway failed a step; retrying"
                        );
                        tokio::time::sleep(RETRY_WAIT).await;
                    }
                    Ok(Err(failure)) => return Err(failure.into_error().1.into()),
                    Err(_) if unattended && stalls < UNATTENDED_STALL_RETRIES => stalls += 1,
                    Err(_) => return Err(TurnEnd::Stalled),
                }
            };
            if let Some(found) = response
                .headers()
                .get("x-obleth-request-id")
                .and_then(|v| v.to_str().ok())
                .filter(|s| s.len() <= 200)
            {
                request_id = Some(found.to_owned());
            }
            // Auto's reason comes with the answer that chose the model; later rounds
            // of the turn name that model and say nothing.
            if payload["model"] == "auto" {
                auto_why = gateway::auto_why(response.headers());
            }
            emit(
                &mut tx,
                "meta",
                json!({"conversationId":id,"messageId":turn.assistant_id,"userMessageId":user_message_id,"generationId":generation_id,"requestId":request_id,"resolvedModel":resolved.model,"settings":parameters}),
            );
            let mut stream = response.bytes_stream();
            let mut decoder = Decoder::default();
            let mut done = false;
            let mut heard = Instant::now();
            'outer: loop {
                let chunk = tokio::select! {
                    chunk=stream.next()=>match chunk {Some(chunk)=>{heard=Instant::now();chunk},None=>break},
                    _=checkpoint.tick()=>{
                        if heard.elapsed() > Duration::from_secs(STALL_SECS) {
                            return Err(TurnEnd::Stalled);
                        }
                        let total=content.len()+reasoning_content.len();
                        if lifecycle::checkpoint_due(total,saved_bytes,saved_at.elapsed()) {
                            let reasoning=(reasoning_content.len()!=saved_reasoning).then_some(reasoning_content.as_str());
                            match lifecycle::checkpoint(&s.store.pool,user.id,id,generation_id,&content,reasoning,request_id.as_deref()).await {
                                Ok(())=>{saved_bytes=total;saved_reasoning=reasoning_content.len();saved_at=Instant::now();}
                                // A transient database error retries on a later tick; the
                                // control loop bounds how long work runs without the database.
                                Err(e) if e.0==StatusCode::INTERNAL_SERVER_ERROR=>tracing::warn!("could not checkpoint chat generation; retrying"),
                                Err(_)=>return Err("The response could not save progress".into()),
                            }
                        }
                        continue;
                    }
                };
                let Ok(chunk) = chunk else { break };
                for event in decoder.events(&chunk)? {
                    let v = match event {
                        gateway::Event::Done => {
                            done = true;
                            break 'outer;
                        }
                        // A server-side failure partway through, such as the upstream
                        // connection breaking, is a drop: the step is tried again.
                        gateway::Event::Failed {
                            retry: true,
                            reason,
                        } => {
                            tracing::warn!(
                                reason = reason.as_deref().unwrap_or(""),
                                "gateway stream broke off"
                            );
                            break 'outer;
                        }
                        gateway::Event::Failed {
                            retry: false,
                            reason,
                        } => {
                            tracing::warn!(
                                reason = reason.as_deref().unwrap_or(""),
                                "gateway reported an error during generation"
                            );
                            return Err(match reason {
                                Some(why) => {
                                    format!("Gateway reported an error during generation: {why}")
                                }
                                None => "Gateway reported an error during generation".to_string(),
                            }
                            .into());
                        }
                        gateway::Event::Chunk(v) => v,
                    };
                    if resolved.model == "auto" && served.is_none() {
                        if let Some(model) = v
                            .get("model")
                            .and_then(Value::as_str)
                            .filter(|m| gateway::valid_model(m) && *m != "auto")
                        {
                            // The upstream may name the model its own way; record and
                            // keep the gateway's name, which later tool rounds need.
                            let model =
                                models::gateway_name(model, &resolved.choices).unwrap_or(model);
                            served = Some(model.to_owned());
                            if resolved.choices.iter().any(|c| c == model) {
                                // Keep the rest of this turn's tool rounds on the same model.
                                payload["model"] = json!(model);
                            }
                            emit(&mut tx, "model", json!({"model":model,"why":auto_why}));
                        }
                    }
                    if let Some(u) = gateway::usage_of(&v) {
                        // Agent steps are separate requests; the turn reports their sum.
                        let mut clean = match usage.take() {
                            Some(previous) if steps > 1 => previous,
                            _ => json!({}),
                        };
                        gateway::add_usage(&mut clean, u);
                        // Measured energy and carbon, when the gateway reports them: the sum too.
                        for key in ["energy_wh", "co2_g"] {
                            if let Some(n) = u
                                .get(key)
                                .and_then(Value::as_f64)
                                .filter(|n| n.is_finite() && (0.0..1e9).contains(n))
                            {
                                let previous = if steps > 1 {
                                    clean.get(key).and_then(Value::as_f64).unwrap_or(0.0)
                                } else {
                                    0.0
                                };
                                clean[key] = json!(previous + n);
                            }
                        }
                        // Time spent writing, from each step's first token to its last.
                        if let (Some(from), Some(to)) = (wrote_first, wrote_last) {
                            let ms = to.duration_since(from).as_millis() as u64;
                            let previous = if steps > 1 {
                                clean
                                    .get("generation_ms")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0)
                            } else {
                                0
                            };
                            clean["generation_ms"] = json!(previous + ms);
                            if steps <= 1 {
                                clean["first_token_ms"] =
                                    json!(from.duration_since(asked).as_millis() as u64);
                            }
                        }
                        // The last step alone is what the model holds: the context meter's size.
                        for (key, last) in [
                            ("prompt_tokens", "last_prompt_tokens"),
                            ("completion_tokens", "last_completion_tokens"),
                        ] {
                            if let Some(n) = u
                                .get(key)
                                .and_then(Value::as_u64)
                                .filter(|n| *n <= 1_000_000_000)
                            {
                                clean[last] = json!(n);
                            }
                        }
                        clean["steps"] = json!(steps.max(1));
                        usage = Some(clean);
                    }
                    if let Some(choice) = v
                        .get("choices")
                        .and_then(Value::as_array)
                        .and_then(|a| a.first())
                    {
                        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                            if reason.len() <= 80 {
                                finish_reason = Some(reason.into());
                            }
                        }
                        if let Some(deltas) = choice
                            .pointer("/delta/tool_calls")
                            .and_then(Value::as_array)
                        {
                            if !tool_turn {
                                return Err(
                                    "This model requested tools. Turn on the workspace to let it use them."
                                        .into(),
                                );
                            }
                            accumulate_tool_calls(&mut calls, deltas)?;
                            let now = Instant::now();
                            wrote_first.get_or_insert(now);
                            wrote_last = Some(now);
                            // Show the file the model is typing, a few times a second.
                            if drafted.elapsed() >= Duration::from_millis(300) {
                                if let Some(draft) = calls.last().and_then(draft_of) {
                                    drafted = Instant::now();
                                    emit(&mut tx, "draft", draft);
                                }
                            }
                        }
                        for (channel, delta) in gateway::text_deltas(choice) {
                            // Generated images arrive inline; keep them as files and link them.
                            let stored;
                            let delta = if channel == "delta" && images::has_inline(delta) {
                                stored = images::store_inline(&s.store, user.id, id, delta).await;
                                stored.as_str()
                            } else {
                                delta
                            };
                            if content.len() + reasoning_content.len() + delta.len()
                                > store::MAX_RESPONSE_BYTES
                            {
                                return Err("The response exceeded the 1 MiB storage limit".into());
                            }
                            if first {
                                s.metrics
                                    .first_token
                                    .observe(started.elapsed().as_secs_f64());
                                first = false;
                            }
                            let now = Instant::now();
                            wrote_first.get_or_insert(now);
                            wrote_last = Some(now);
                            if channel == "reasoning" {
                                reasoning_content.push_str(delta);
                                if tool_turn
                                    && reasoning_content.len() - reasoning_start
                                        > STEP_THINKING_BYTES
                                    && content.len() == step_start
                                    && calls.is_empty()
                                {
                                    emit(&mut tx, channel, json!({"content":delta}));
                                    overthought = true;
                                    break 'outer;
                                }
                            } else {
                                content.push_str(delta);
                            }
                            emit(&mut tx, channel, json!({"content":delta}));
                        }
                    }
                }
            }
            let thought_to = reasoning_content.encode_utf16().count();
            if tool_turn && thought_to > thought_from {
                let entry = json!({"id":format!("thought-{steps}"),"kind":"thought","from":thought_from,"to":thought_to,"durationMs":round_began.elapsed().as_millis() as u64,"status":"completed"});
                record(&mut activity, &entry, true);
                emit(&mut tx, "tool", entry);
            }
            if overthought {
                if overthinks >= MAX_OVERTHINKS {
                    return Err("The model kept thinking without writing anything. Try a lower thinking effort or another model.".into());
                }
                overthinks += 1;
                if payload.get("reasoning_effort").is_some() {
                    payload["reasoning_effort"] = json!("low");
                }
                payload["messages"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"role":"user","content":OVERTHOUGHT}));
                let entry = json!({"id":format!("overthought-{steps}"),"tool":"thinking_limit","summary":"Thought too long without writing; Chat asked for a smaller step","detail":format!("The step thought for over {} KB without writing anything, so Chat stopped it and asked again at low effort.", STEP_THINKING_BYTES / 1024),"status":"error","startedAt":chrono::Utc::now()});
                record(&mut activity, &entry, true);
                emit(&mut tx, "tool", entry);
                continue;
            }
            // The stream broke off before the step finished. Nothing in it ran, so the
            // model carries on from what it had written; what the person saw stays.
            if !done {
                if drops >= MAX_DROPS {
                    return Err("The connection to the model kept dropping".into());
                }
                drops += 1;
                let text = content[step_start..].to_owned();
                let names: Vec<&str> = calls
                    .iter()
                    .map(|c| c.name.as_str())
                    .filter(|n| !n.is_empty())
                    .collect();
                let messages = payload["messages"].as_array_mut().unwrap();
                if !text.is_empty() {
                    messages.push(json!({"role":"assistant","content":text}));
                }
                if !names.is_empty() {
                    messages.push(json!({"role":"user","content":format!("[Chat: the connection to the model dropped while you were writing a call to {}, so nothing ran. Carry on without repeating what you already wrote. Write a long file in parts, the first with write_file and each next one with write_file and append set, so no single call takes long to write.]", names.join(", "))}));
                } else if !text.is_empty() {
                    messages.push(json!({"role":"user","content":"[Chat: the connection to the model dropped. Continue exactly where it stopped, without repeating anything.]"}));
                }
                let entry = json!({"id":format!("drop-{steps}"),"tool":"connection","summary":"The connection to the model dropped; Chat carried on","detail":"The step stopped before it finished, so nothing in it ran. Chat asked the model to continue.","status":"error","startedAt":chrono::Utc::now()});
                record(&mut activity, &entry, true);
                emit(&mut tx, "tool", entry);
                continue;
            }
            drops = 0;
            // A step that stopped at the output limit: a half-written tool call never
            // runs, and an answer cut short under a set limit carries on where it stopped.
            if finish_reason.as_deref() == Some("length") {
                let text = content[step_start..].to_owned();
                let cut_call = tool_turn && !calls.is_empty();
                let limit = max_tokens.map_or("the model's output limit".to_string(), |n| {
                    format!("the {n}-token output limit")
                });
                let note = if cut_call {
                    let names: Vec<&str> = calls
                        .iter()
                        .map(|c| c.name.as_str())
                        .filter(|n| !n.is_empty())
                        .collect();
                    Some(format!("[Chat: your last step stopped at {limit} while you were writing a call to {}, so nothing ran. Keep each call well under the limit: write a long file in parts, the first with write_file and each next one with write_file and append set, and think less before writing.]", names.join(", ")))
                } else if max_tokens.is_some()
                    && text.trim().is_empty()
                    && thought_to > thought_from
                {
                    Some("[Chat: you reached the output limit while thinking. Write the answer now.]".to_string())
                } else if max_tokens.is_some() && !text.trim().is_empty() {
                    Some("[Chat: your answer reached the output limit. Continue exactly where it stopped, without repeating anything.]".to_string())
                } else {
                    None
                };
                if let Some(note) = note.filter(|_| cuts < MAX_CUTS) {
                    cuts += 1;
                    let messages = payload["messages"].as_array_mut().unwrap();
                    if !text.is_empty() {
                        messages.push(json!({"role":"assistant","content":text}));
                    }
                    messages.push(json!({"role":"user","content":note}));
                    if cut_call {
                        let entry = json!({"id":format!("cut-{steps}"),"tool":"output_limit","summary":"Stopped at the output limit mid-call; nothing ran","detail":format!("The step reached {limit}. Chat asked the model to write in smaller parts."),"status":"error","startedAt":chrono::Utc::now()});
                        record(&mut activity, &entry, true);
                        emit(&mut tx, "tool", entry);
                    }
                    continue;
                }
                if cut_call {
                    return Err(format!("The model kept stopping at {limit} in the middle of a tool call. Raise the output limit in Parameters, or lower thinking effort.").into());
                }
            } else {
                cuts = 0;
            }
            if calls.is_empty() && tool_turn {
                let offered: Vec<String> = payload["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.pointer("/function/name")?.as_str().map(str::to_owned))
                    .collect();
                if let Some((text, found)) = text_tool_calls(&content[step_start..], &offered) {
                    content.truncate(step_start);
                    content.push_str(&text);
                    calls = found;
                    emit(&mut tx, "content", json!({"content":content}));
                }
            }
            // A reply that changed files and ran nothing since is asked, once, to
            // check the work first. Its draft is held back: the person sees the
            // answer it gives after checking.
            if calls.is_empty() && unchecked && !gated && workspace.is_some() {
                gated = true;
                let text = content[step_start..].to_owned();
                content.truncate(step_start);
                emit(&mut tx, "content", json!({"content":content}));
                let messages = payload["messages"].as_array_mut().unwrap();
                if !text.is_empty() {
                    messages.push(json!({"role":"assistant","content":text}));
                }
                messages.push(json!({"role":"user","content":CHECK_NOTE}));
                let entry = json!({"id":format!("check-{steps}"),"tool":"check","summary":"Chat asked the model to check its work","detail":"Files changed and nothing ran after the last change, so the model was asked to run or test the work before answering.","status":"completed","startedAt":chrono::Utc::now()});
                record(&mut activity, &entry, true);
                emit(&mut tx, "tool", entry);
                continue;
            }
            overthinks = 0;
            if calls.is_empty() || !tool_turn {
                // A research reply's statements about runs are checked against them.
                if let Some(project) = project
                    .as_ref()
                    .filter(|_| research_tools && calls.is_empty())
                {
                    if let Some(research::tools::Standing::Here(research)) =
                        research::tools::standing(&s.store.pool, user.id, id, project.id).await
                    {
                        let ask = decisions::Asker {
                            decisions: &s.decisions,
                            gateway: Some(&gateway),
                            user: &user,
                        };
                        if let Some(entry) =
                            research::report::check(&s.store.pool, Some(ask), &research, &content)
                                .await
                        {
                            record(&mut activity, &entry, true);
                            emit(&mut tx, "tool", entry);
                        }
                    }
                }
                break;
            }
            unique_call_ids(&mut calls, &mut call_ids, steps);
            let text = content[step_start..].to_owned();
            let mut step = json!({
                "role":"assistant",
                "content": if text.is_empty() { Value::Null } else { json!(text) },
                "tool_calls": calls.iter().map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}})).collect::<Vec<_>>(),
            });
            // The step's thinking goes back with it. Thinking models that call tools
            // (Kimi, Qwen3, GLM, DeepSeek) expect their earlier steps' thinking in the
            // history; given empty <think></think> blocks, Kimi starts writing its
            // thinking into the reply instead.
            let thought = &reasoning_content[reasoning_start..];
            if !thought.trim().is_empty() {
                step["reasoning"] = json!(thought);
            }
            payload["messages"].as_array_mut().unwrap().push(step);
            let workspace_call = calls.iter().any(|c| {
                c.name != knowledge::chat::TOOL
                    && c.name != knowledge::chat::SAVE_TOOL
                    && c.name != project_log::TOOL
                    && c.name != questions::TOOL
                    && c.name != questions::WORKSPACE_TOOL
                    && c.name != questions::RESEARCH_TOOL
                    && c.name != questions::APP_TOOL
                    && c.name != questions::PROJECT_TOOL
                    && c.name != browser::tools::HANDOFF
                    && c.name != browser::tools::REQUEST
                    && c.name != connections::device::REQUEST
                    && c.name != agents::TOOL
                    && c.name != notebook::web::TOOL
                    && !board::tools::is_tool(&c.name)
                    && !(browsing.is_some() && browser::tools::is_tool(&c.name))
                    && toolset.as_ref().and_then(|t| t.find(&c.name)).is_none()
            });
            if !model_offered
                && (workspace_call
                    || calls
                        .iter()
                        .any(|c| toolset.as_ref().and_then(|t| t.find(&c.name)).is_some()))
            {
                model_offered = true;
                let current = served.as_deref().unwrap_or(&resolved.model).to_owned();
                if resolved.actions != models::Actions::Certified && resolved.model_suggestions {
                    if let Some(better) =
                        models::suggest(&s.store.pool, &gateway, &user, &current).await
                    {
                        if models::record_suggestion(
                            &s.store.pool,
                            id,
                            user.id,
                            &current,
                            &better.id,
                        )
                        .await
                        {
                            let entry = json!({"id":"model-suggestion","kind":"model","status":"asked","summary":"Suggested a better suited model","model":current,"suggested":better.id,"name":better.name,"score":better.certification.as_ref().and_then(|c| c["score"].as_f64())});
                            record(&mut activity, &entry, true);
                            emit(&mut tx, "tool", entry);
                        }
                    }
                }
            }
            let mut running = None;
            if let Some((sandbox, sandboxes)) = workspace.as_ref().filter(|_| workspace_call) {
                // The first workspace tool of a turn may need to start or wake the pod.
                let sandbox = sandboxes.ensure_running(sandbox).await?;
                if !uploads_copied {
                    copy_uploads(&s, &uploads, sandboxes, &sandbox).await;
                    uploads_copied = true;
                }
                // A woken pod has no server yet; bring the preview back before the tools run.
                if sandbox.preview_port.is_none() {
                    if let Ok(Some(server)) =
                        sandbox::tools::ensure_dev_server(sandboxes, &sandbox).await
                    {
                        emit(&mut tx, "preview", json!({"port":server.port}));
                    }
                }
                running = Some((sandbox, sandboxes));
            }
            let (mut wrote, mut broken, mut asked) = (false, false, false);
            for call in &calls {
                if call.name == pause::TOOL {
                    let output = match offers_pause.then(|| pause::parse(&call.arguments)) {
                        None => no_such_tool(&call.name),
                        Some(Err(problem)) => problem,
                        Some(Ok(_)) if paused.is_some() => pause::SHOWN.to_string(),
                        Some(Ok(note)) => {
                            // The note goes in the workspace, where a later chat finds it.
                            let saved = match &running {
                                Some((sandbox, sandboxes)) => {
                                    let args = json!({"path":pause::FILE,"content":note.file(chrono::Utc::now())}).to_string();
                                    sandbox::tools::run(sandboxes, sandbox, "write_file", &args)
                                        .await
                                        .ok
                                }
                                None => false,
                            };
                            wrote |= saved;
                            let entry = pause::entry(&call.id, saved.then_some(pause::FILE));
                            record(&mut activity, &entry, true);
                            emit(&mut tx, "tool", entry);
                            // Auto mode leaves a paused chat alone until the person writes.
                            let _ = sqlx::query("UPDATE conversations SET auto_nudges=$3 WHERE id=$1 AND owner_id=$2")
                                .bind(id)
                                .bind(user.id)
                                .bind(autopilot::MAX_NUDGES)
                                .execute(&s.store.pool)
                                .await;
                            paused = Some((note, saved));
                            pause::SHOWN.to_string()
                        }
                    };
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == questions::WORKSPACE_TOOL || call.name == questions::RESEARCH_TOOL {
                    let research = call.name == questions::RESEARCH_TOOL;
                    let offered = if research {
                        offers_research
                    } else {
                        offers_workspace
                    };
                    let (card, shown) = if research {
                        let card =
                            questions::research_entry(&call.id, &call.arguments, research_here);
                        (card, questions::RESEARCH_SHOWN)
                    } else {
                        let card = questions::workspace_entry(&call.id, &call.arguments);
                        (card, questions::WORKSPACE_SHOWN)
                    };
                    let card = offered.then_some(card);
                    let output =
                        show_card(card, shown, &call.name, &mut asked, &mut activity, &mut tx);
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == questions::APP_TOOL {
                    let model = served.as_deref().unwrap_or(&resolved.model);
                    let card = app_room
                        .map(|here| questions::app_entry(&call.id, &call.arguments, model, here));
                    let shown = questions::APP_SHOWN;
                    let output =
                        show_card(card, shown, &call.name, &mut asked, &mut activity, &mut tx);
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == questions::PROJECT_TOOL {
                    let card =
                        offers_project.then(|| questions::project_entry(&call.id, &call.arguments));
                    let shown = questions::PROJECT_SHOWN;
                    let output =
                        show_card(card, shown, &call.name, &mut asked, &mut activity, &mut tx);
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == connections::device::REQUEST {
                    let card = offers_device
                        .then(|| Ok(connections::device::card_entry(&call.id, &call.arguments)));
                    let shown = connections::device::REQUEST_SHOWN;
                    let output =
                        show_card(card, shown, &call.name, &mut asked, &mut activity, &mut tx);
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == browser::tools::HANDOFF || call.name == browser::tools::REQUEST {
                    let handoff = call.name == browser::tools::HANDOFF;
                    let offered = if handoff {
                        asks && browsing.is_some()
                    } else {
                        offers_browser
                    };
                    let output = if offered && !asked {
                        asked = true;
                        if let (true, Some((browsers, _))) = (handoff, &browsing) {
                            let _ = browsers.set_controller(user.id, id, "user").await;
                        }
                        let entry =
                            browser::tools::card_entry(&call.id, &call.name, &call.arguments);
                        record(&mut activity, &entry, true);
                        emit(&mut tx, "tool", entry);
                        if handoff {
                            browser::tools::HANDOFF_SHOWN
                        } else {
                            browser::tools::REQUEST_SHOWN
                        }
                        .to_string()
                    } else if asked {
                        ONE_AT_A_TIME.to_string()
                    } else {
                        no_such_tool(&call.name)
                    };
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if let Some((browsers, _)) = browsing
                    .as_ref()
                    .filter(|_| browser::tools::is_tool(&call.name))
                {
                    let args: Value = serde_json::from_str(&call.arguments).unwrap_or(json!({}));
                    let mut entry = json!({"id":call.id,"tool":call.name,"summary":browser::tools::summary(&call.name, &args),"status":"running","startedAt":chrono::Utc::now()});
                    record(&mut activity, &entry, true);
                    emit(&mut tx, "tool", entry.clone());
                    let began = Instant::now();
                    let mut ctx = browser::tools::Ctx {
                        browsers,
                        owner: user.id,
                        conversation: id,
                        approved: false,
                        auto: auto_mode(s.clone()).await,
                    };
                    let mut run = browser::tools::run(&ctx, &call.name, &call.arguments).await;
                    if let Some((question, host)) = run.ask.take() {
                        entry["status"] = json!("awaiting");
                        entry["question"] = json!(question);
                        entry["remember"] = json!("Always on this site");
                        record(&mut activity, &entry, false);
                        emit(&mut tx, "tool", entry.clone());
                        // Saved now so a reloaded page shows the question too.
                        let _ = lifecycle::save_activity(
                            &s.store.pool,
                            user.id,
                            id,
                            generation_id,
                            &activity,
                        )
                        .await;
                        browser::ask(&s.store.pool, user.id, id, generation_id, &call.id, &host)
                            .await
                            .map_err(|_| "The response could not ask for approval".to_string())?;
                        let decision =
                            connections::wait(&s.store.pool, generation_id, &call.id).await;
                        if decision != connections::Decision::Allow {
                            entry["status"] = json!("declined");
                            entry["detail"] =
                                json!(if decision == connections::Decision::Expired {
                                    "No answer within 15 minutes"
                                } else {
                                    "Declined"
                                });
                            record(&mut activity, &entry, false);
                            emit(&mut tx, "tool", entry);
                            tool_result(&mut payload, &call.id, "The person declined this, so nothing was clicked or submitted. Do not try it again another way unless they ask.");
                            let _ = lifecycle::save_activity(
                                &s.store.pool,
                                user.id,
                                id,
                                generation_id,
                                &activity,
                            )
                            .await;
                            continue;
                        }
                        entry["status"] = json!("running");
                        record(&mut activity, &entry, false);
                        emit(&mut tx, "tool", entry.clone());
                        ctx.approved = true;
                        run = browser::tools::run(&ctx, &call.name, &call.arguments).await;
                    }
                    let outcome = run.outcome;
                    entry["status"] = json!(if outcome.ok { "completed" } else { "error" });
                    entry["summary"] = json!(outcome.summary);
                    entry["detail"] = json!(outcome.detail);
                    entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                    if let Some(facts) = outcome.extra.as_object() {
                        for (key, value) in facts {
                            entry[key] = value.clone();
                        }
                    }
                    record(&mut activity, &entry, false);
                    browser::keep_recent_thumbnails(&mut activity);
                    emit(&mut tx, "tool", entry);
                    tool_result(&mut payload, &call.id, outcome.output);
                    continue;
                }
                if call.name == questions::TOOL {
                    let output = match questions::parse(&call.arguments) {
                        Ok(list) if !asked && asks => {
                            asked = true;
                            let entry = questions::entry(&call.id, list);
                            record(&mut activity, &entry, true);
                            emit(&mut tx, "tool", entry);
                            questions::SHOWN.to_string()
                        }
                        Ok(_) if asked => "Ask all your questions in one call.".to_string(),
                        Ok(_) => no_such_tool(&call.name),
                        Err(problem) => problem,
                    };
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == project_log::TOOL {
                    let Some(project) = project.as_ref().filter(|_| log_tool) else {
                        tool_result(&mut payload, &call.id, no_such_tool(&call.name));
                        continue;
                    };
                    if log_reads >= project_log::MAX_TOOL_READS {
                        tool_result(&mut payload, &call.id, format!("That is {} reads of the log, the most for one answer. Answer from what you have read.", project_log::MAX_TOOL_READS));
                        continue;
                    }
                    log_reads += 1;
                    let mut entry = json!({"id":call.id,"tool":call.name,"summary":"Reading the project log","status":"running","startedAt":chrono::Utc::now()});
                    record(&mut activity, &entry, true);
                    emit(&mut tx, "tool", entry.clone());
                    let began = Instant::now();
                    let read = project_log::read_for_tool(
                        &s.store.pool,
                        user.id,
                        project.id,
                        &project.name,
                        &call.arguments,
                    )
                    .await;
                    entry["status"] = json!(if read.ok { "completed" } else { "error" });
                    entry["summary"] = json!(read.summary);
                    entry["detail"] = json!(read.detail);
                    entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                    record(&mut activity, &entry, false);
                    emit(&mut tx, "tool", entry);
                    tool_result(&mut payload, &call.id, read.output);
                    continue;
                }
                if call.name == knowledge::chat::TOOL {
                    let (Some(k), Some(found)) = (&s.knowledge, knowledge_turn.as_mut()) else {
                        tool_result(&mut payload, &call.id, no_such_tool(&call.name));
                        continue;
                    };
                    let args: Value = serde_json::from_str(&call.arguments).unwrap_or(json!({}));
                    let query = knowledge::excerpt(args["query"].as_str().unwrap_or("").trim(), 80);
                    let mut entry = json!({"id":call.id,"tool":call.name,"summary":format!("Searched for “{query}”"),"status":"running","startedAt":chrono::Utc::now()});
                    record(&mut activity, &entry, true);
                    emit(&mut tx, "tool", entry.clone());
                    let began = Instant::now();
                    let (ok, output, detail) =
                        k.call(&gateway, &user, found, &call.arguments).await;
                    entry["status"] = json!(if ok { "completed" } else { "error" });
                    entry["detail"] = json!(detail);
                    entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                    record(&mut activity, &entry, false);
                    emit(&mut tx, "tool", entry);
                    let sources = found.entry();
                    match activity.iter_mut().find(|e| e["id"] == "knowledge") {
                        Some(saved) => *saved = sources.clone(),
                        None => record(&mut activity, &sources, true),
                    }
                    emit(&mut tx, "tool", sources);
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == knowledge::chat::SAVE_TOOL {
                    let output = match (&s.knowledge, knowledge_turn.as_ref().filter(|t| t.library))
                    {
                        (Some(k), Some(found)) => {
                            let (_, output, entry) = k
                                .save_call(
                                    &gateway,
                                    &user,
                                    found,
                                    id,
                                    turn.assistant_id,
                                    &call.id,
                                    &call.arguments,
                                )
                                .await;
                            record(&mut activity, &entry, true);
                            emit(&mut tx, "tool", entry);
                            output
                        }
                        _ => no_such_tool(&call.name),
                    };
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if call.name == notebook::web::TOOL {
                    // Like save_item, this writes only to the person's own collection
                    // in the project, from their own browser's tab, so it runs without
                    // asking.
                    let output = match project.as_ref().filter(|_| notebook_pages) {
                        Some(project) => {
                            let mut entry = json!({"id":call.id,"tool":call.name,"summary":"Adding the page to the notebook","status":"running","startedAt":chrono::Utc::now()});
                            record(&mut activity, &entry, true);
                            emit(&mut tx, "tool", entry.clone());
                            let began = Instant::now();
                            let (ok, summary, output) =
                                notebook::web::run_tool(&s, &user, id, project.id).await;
                            entry["status"] = json!(if ok { "completed" } else { "error" });
                            entry["summary"] = json!(summary);
                            entry["detail"] = json!(output);
                            entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                            entry["projectId"] = json!(project.id);
                            record(&mut activity, &entry, false);
                            emit(&mut tx, "tool", entry);
                            output
                        }
                        None => no_such_tool(&call.name),
                    };
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                if board::tools::is_tool(&call.name) {
                    // Like save_item, these change only the project's own board, so
                    // they run without asking.
                    let output = match project.as_ref().filter(|_| board_tools) {
                        Some(project) => {
                            let args: Value =
                                serde_json::from_str(&call.arguments).unwrap_or(json!({}));
                            let mut entry = json!({"id":call.id,"tool":call.name,"summary":board::tools::summary(&call.name, &args),"status":"running","startedAt":chrono::Utc::now()});
                            record(&mut activity, &entry, true);
                            emit(&mut tx, "tool", entry.clone());
                            let began = Instant::now();
                            let ctx = board::tools::Ctx {
                                pool: &s.store.pool,
                                user: user.id,
                                conversation: id,
                                project: project.id,
                            };
                            let ran = board::tools::run(&ctx, &call.name, &call.arguments).await;
                            entry["status"] = json!(if ran.ok { "completed" } else { "error" });
                            entry["summary"] = json!(ran.summary);
                            entry["detail"] = json!(ran.output);
                            entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                            entry["projectId"] = json!(project.id);
                            if let Some(card) = ran.card {
                                entry["cardId"] = json!(card);
                            }
                            record(&mut activity, &entry, false);
                            emit(&mut tx, "tool", entry);
                            ran.output
                        }
                        None => no_such_tool(&call.name),
                    };
                    tool_result(&mut payload, &call.id, output);
                    continue;
                }
                let connection = toolset.as_ref().and_then(|t| t.find(&call.name));
                if let (Some(index), Some(set)) = (connection, toolset.as_mut()) {
                    let mut entry = json!({"id":call.id,"tool":call.name,"connection":set.connection(index),"summary":set.label(index),"status":"running","startedAt":chrono::Utc::now()});
                    if let (Some(entry), Value::Object(about)) =
                        (entry.as_object_mut(), set.describe(index))
                    {
                        entry.extend(about);
                    }
                    record(&mut activity, &entry, true);
                    let auto = match &s.connections {
                        Some(c) => c.auto(id).await,
                        None => false,
                    };
                    if set.needs_approval(index)
                        && !auto
                        && !connections::granted(&s.store.pool, id, set.grant(index)).await
                    {
                        // What the card shows comes from the code that will carry the
                        // call out; a call it would refuse is refused without asking.
                        let review = match set.review(index, &call.arguments) {
                            Ok(review) => review,
                            Err(error) => {
                                entry["status"] = json!("error");
                                entry["detail"] = json!(error);
                                record(&mut activity, &entry, false);
                                emit(&mut tx, "tool", entry);
                                tool_result(&mut payload, &call.id, error);
                                continue;
                            }
                        };
                        // A change that only adds something may be put to a decision
                        // first; a sure yes to every question runs it without the card.
                        let judged = match set.gate(index, &call.arguments) {
                            connections::gate::Gate::Judge => match s.decisions.settings().await {
                                Ok(settings) if settings.on() && settings.check_actions => Some(
                                    connections::gate::judge(
                                        decisions::Asker {
                                            decisions: &s.decisions,
                                            gateway: Some(&gateway),
                                            user: &user,
                                        },
                                        &settings,
                                        &set.label(index),
                                        &review,
                                        &connections::gate::person_messages(&payload["messages"]),
                                    )
                                    .await,
                                ),
                                _ => None,
                            },
                            connections::gate::Gate::Ask => None,
                        };
                        if let Some(judged) = &judged {
                            entry["decision"] = judged.entry();
                        }
                        if judged.as_ref().is_some_and(|j| j.allowed) {
                            record(&mut activity, &entry, false);
                        } else {
                            entry["status"] = json!("awaiting");
                            entry["detail"] =
                                json!(connections::arguments_preview(&call.arguments));
                            entry["change"] = review;
                            record(&mut activity, &entry, false);
                            emit(&mut tx, "tool", entry.clone());
                            // Saved now so a reloaded page shows the question too.
                            let _ = lifecycle::save_activity(
                                &s.store.pool,
                                user.id,
                                id,
                                generation_id,
                                &activity,
                            )
                            .await;
                            connections::ask(
                                &s.store.pool,
                                user.id,
                                id,
                                generation_id,
                                &call.id,
                                set.grant(index),
                            )
                            .await
                            .map_err(|_| "The response could not ask for approval".to_string())?;
                            let decision =
                                connections::wait(&s.store.pool, generation_id, &call.id).await;
                            entry["status"] = json!("running");
                            // Answered: the full arguments are not kept with the reply.
                            if let Some(entry) = entry.as_object_mut() {
                                entry.remove("change");
                            }
                            if decision != connections::Decision::Allow {
                                entry["status"] = json!("declined");
                                entry["detail"] =
                                    json!(if decision == connections::Decision::Expired {
                                        "No answer within 15 minutes"
                                    } else {
                                        "Declined"
                                    });
                                record(&mut activity, &entry, false);
                                emit(&mut tx, "tool", entry);
                                tool_result(&mut payload, &call.id, "The person declined this tool call, so it did not run. Do not try it again unless they ask.");
                                let _ = lifecycle::save_activity(
                                    &s.store.pool,
                                    user.id,
                                    id,
                                    generation_id,
                                    &activity,
                                )
                                .await;
                                continue;
                            }
                            record(&mut activity, &entry, false);
                            // A reloaded page stops showing the question.
                            let _ = lifecycle::save_activity(
                                &s.store.pool,
                                user.id,
                                id,
                                generation_id,
                                &activity,
                            )
                            .await;
                        }
                    }
                    if set.is_device(index) {
                        // The person's browser carries this out: the step carries
                        // what to do, and is saved so a reloaded page sees it too.
                        entry["request"] = set.request(index, &call.arguments);
                        record(&mut activity, &entry, false);
                        let _ = lifecycle::save_activity(
                            &s.store.pool,
                            user.id,
                            id,
                            generation_id,
                            &activity,
                        )
                        .await;
                    }
                    emit(&mut tx, "tool", entry.clone());
                    let began = Instant::now();
                    let outcome = set.call(index, &call.id, &call.arguments).await;
                    if let Some(entry) = entry.as_object_mut() {
                        entry.remove("request");
                    }
                    entry["status"] = json!(if outcome.ok { "completed" } else { "error" });
                    entry["detail"] = json!(outcome.detail);
                    if let Some(card) = outcome.card {
                        entry["card"] = card;
                    }
                    entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                    record(&mut activity, &entry, false);
                    emit(&mut tx, "tool", entry);
                    tool_result(&mut payload, &call.id, outcome.output);
                    continue;
                }
                if call.name == agents::TOOL && !workers.is_empty() {
                    // Workers run together; the step shows each one's progress as it goes.
                    let args: Value = serde_json::from_str(&call.arguments).unwrap_or(json!({}));
                    let mut entry = json!({"id":call.id,"tool":call.name,"summary":agents::summary(&args),"status":"running","startedAt":chrono::Utc::now()});
                    record(&mut activity, &entry, true);
                    emit(&mut tx, "tool", entry.clone());
                    let began = Instant::now();
                    // Files come from the chat's workspace: woken if it sleeps, turned on
                    // when the tasks need files and the chat has none.
                    let launch = workspace.is_none() && may_launch && agents::wants_files(&args);
                    if launch {
                        entry["detail"] = json!("Turning on the workspace");
                        emit(&mut tx, "tool", entry.clone());
                    }
                    let place = match (&running, &workspace) {
                        (Some((sandbox, sandboxes)), _) => {
                            Ok(Some((sandbox.clone(), (*sandboxes).clone())))
                        }
                        (None, Some((sandbox, sandboxes))) => sandboxes
                            .ensure_running(sandbox)
                            .await
                            .map(|running| Some((running, sandboxes.clone()))),
                        (None, None) if launch => launch_workspace(&s, &user, id).await.map(Some),
                        (None, None) => Ok(None),
                    };
                    let mut note = String::new();
                    let place = match (
                        place,
                        args["repo"]
                            .as_str()
                            .map(str::trim)
                            .filter(|r| !r.is_empty()),
                    ) {
                        (Ok(Some((sandbox, sandboxes))), Some(repo)) => {
                            entry["detail"] = json!("Checking out the repository");
                            emit(&mut tx, "tool", entry.clone());
                            let pod = code::workspace::Pod {
                                sandboxes: &sandboxes,
                                sandbox: &sandbox,
                            };
                            let checked = match agents::repo_url(repo) {
                                Ok((url, name)) => agents::checkout(&pod, &url, &name)
                                    .await
                                    .map(|folder| format!("The repository {url} is checked out at {folder}/ in the workspace. Start there.")),
                                Err(e) => Err(e),
                            };
                            checked.map(|n| {
                                note = n;
                                Some((sandbox, sandboxes))
                            })
                        }
                        (place, _) => place,
                    };
                    let outcome = match place {
                        Err(e) => sandbox::tools::Outcome {
                            ok: false,
                            detail: e.clone(),
                            summary: agents::summary(&args),
                            output: format!("Error: {e}"),
                            preview: None,
                            extra: json!({}),
                        },
                        Ok(place) => {
                            if launch {
                                entry["workspace"] = json!(true);
                            }
                            entry.as_object_mut().unwrap().remove("detail");
                            let pod =
                                place
                                    .as_ref()
                                    .map(|(sandbox, sandboxes)| code::workspace::Pod {
                                        sandboxes,
                                        sandbox,
                                    });
                            let ctx = agents::Ctx {
                                pool: &s.store.pool,
                                gateway: &gateway,
                                user: &user,
                                conversation: id,
                                shell: pod.as_ref(),
                                workers: &workers,
                                note: note.clone(),
                            };
                            let (progress, mut updates) = mpsc::unbounded_channel();
                            let work = agents::run(&ctx, &call.arguments, progress);
                            tokio::pin!(work);
                            let mut tick = 0;
                            let mut outcome = loop {
                                tokio::select! {
                                    outcome = &mut work => break outcome,
                                    Some(agents) = updates.recv() => {
                                        tick += 1;
                                        entry["agents"] = agents;
                                        entry["tick"] = json!(tick);
                                        record(&mut activity, &entry, false);
                                        emit(&mut tx, "tool", entry.clone());
                                    }
                                }
                            };
                            entry["tick"] = json!(tick + 1);
                            if launch {
                                outcome.output.push_str("\n\nChat turned on this chat's workspace for the workers. Its tools are yours from the person's next message.");
                            }
                            if !note.is_empty() {
                                outcome.output = format!("{note}\n\n{}", outcome.output);
                            }
                            outcome
                        }
                    };
                    entry["status"] = json!(if outcome.ok { "completed" } else { "error" });
                    entry["summary"] = json!(outcome.summary);
                    entry["detail"] = json!(outcome.detail);
                    entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                    if let Some(facts) = outcome.extra.as_object() {
                        for (key, value) in facts {
                            entry[key] = value.clone();
                        }
                    }
                    record(&mut activity, &entry, false);
                    emit(&mut tx, "tool", entry);
                    tool_result(&mut payload, &call.id, outcome.output);
                    continue;
                }
                let Some((sandbox, sandboxes)) = &running else {
                    tool_result(&mut payload, &call.id, no_such_tool(&call.name));
                    continue;
                };
                let args: Value = serde_json::from_str(&call.arguments).unwrap_or(json!({}));
                let researching = research_tools && research::tools::is_tool(&call.name);
                let syncing = call.name == "sync_code" && project.is_some();
                let label = if researching {
                    research::tools::summary(&call.name, &args)
                } else if syncing {
                    code::tool_summary(&args)
                } else {
                    sandbox::tools::summary(&call.name, &args)
                };
                let mut entry = json!({"id":call.id,"tool":call.name,"summary":label,"status":"running","startedAt":chrono::Utc::now()});
                record(&mut activity, &entry, true);
                emit(&mut tx, "tool", entry.clone());
                let began = Instant::now();
                let outcome = match project.as_ref().filter(|_| researching || syncing) {
                    Some(project) if syncing => {
                        let pod = code::workspace::Pod { sandboxes, sandbox };
                        code::run_tool(&s.store.pool, &pod, &user, project.id, id, &call.arguments)
                            .await
                    }
                    Some(project) => {
                        let ctx = research::tools::Ctx {
                            pool: &s.store.pool,
                            sb: sandboxes,
                            sandbox,
                            user: user.id,
                            conversation: id,
                            project: project.id,
                            ask: Some(decisions::Asker {
                                decisions: &s.decisions,
                                gateway: Some(&gateway),
                                user: &user,
                            }),
                        };
                        research::tools::run(&ctx, &call.name, &call.arguments).await
                    }
                    None => {
                        sandbox::tools::run(sandboxes, sandbox, &call.name, &call.arguments).await
                    }
                };
                entry["status"] = json!(if outcome.ok { "completed" } else { "error" });
                entry["summary"] = json!(outcome.summary);
                entry["detail"] = json!(outcome.detail);
                entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                if let Some(facts) = outcome.extra.as_object() {
                    for (key, value) in facts {
                        entry[key] = value.clone();
                    }
                }
                if outcome.ok && matches!(call.name.as_str(), "write_file" | "edit_file") {
                    wrote = true;
                    unchecked = true;
                    let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
                    if let Some(path) = args["path"].as_str() {
                        watch.wrote(path, args["content"].as_str());
                    }
                } else if matches!(
                    call.name.as_str(),
                    "run_command" | "run_in_terminal" | "start_server" | "experiment_run"
                ) {
                    unchecked = false;
                    watch.ran();
                }
                if entry["check"] == "error" {
                    broken = true;
                } else if entry["check"] == "ok" {
                    broken = false;
                }
                record(&mut activity, &entry, false);
                emit(&mut tx, "tool", entry);
                if let Some(port) = outcome.preview {
                    emit(&mut tx, "preview", json!({"port":port}));
                }
                tool_result(&mut payload, &call.id, outcome.output);
            }
            // A round that changed files and left the app compiling is a checkpoint.
            if let (true, false, Some((sandbox, sandboxes))) = (wrote, broken, &running) {
                checkpoints += 1;
                let label = format!("Checkpoint {checkpoints}");
                if let Some((sha, stat)) =
                    sandbox::tools::checkpoint(sandboxes, sandbox, &label).await
                {
                    let entry = json!({"id":format!("checkpoint-{sha}"),"kind":"checkpoint","sha":sha,"summary":label,"stat":stat,"status":"completed","startedAt":chrono::Utc::now()});
                    record(&mut activity, &entry, true);
                    emit(&mut tx, "tool", entry);
                }
            }
            // A model that rewrites one file over and over, or keeps putting back
            // a version it replaced, is stuck: tell it so, and on the second loop
            // of an auto turn ask the gateway for a stronger model.
            escalate = None;
            if let Some((path, times)) = watch.looping() {
                loops += 1;
                payload["messages"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"role":"user","content":loop_note(&path, times)}));
                let stronger = loops >= 2 && resolved.model == "auto";
                if stronger {
                    payload["model"] = json!("auto");
                    escalate = Some("tool_loop");
                    served = None;
                }
                let summary = if stronger {
                    "Chat moved this turn to a stronger model"
                } else {
                    "Chat stopped a loop"
                };
                let entry = json!({"id":format!("loop-{steps}"),"tool":"check","summary":summary,"detail":format!("The model kept rewriting {path} without getting anywhere."),"status":"completed","startedAt":chrono::Utc::now()});
                record(&mut activity, &entry, true);
                emit(&mut tx, "tool", entry);
            }
            drop_old_steps(&mut payload);
            trim_tool_history(&mut payload);
            if let Err(e) =
                lifecycle::save_activity(&s.store.pool, user.id, id, generation_id, &activity).await
            {
                if e.0 != StatusCode::INTERNAL_SERVER_ERROR {
                    return Err("The response could not save progress".into());
                }
            }
            // The reply ends at the questions; the answers start the next turn.
            if asked {
                break;
            }
            // A pause ends the reply with where the work stands.
            if let Some((note, saved)) = &paused {
                let text = note.reply(*saved);
                if !content.trim().is_empty() {
                    let trimmed = content.trim_end().len();
                    content.truncate(trimmed);
                    content.push_str("\n\n");
                }
                content.push_str(&text);
                emit(&mut tx, "content", json!({"content":content}));
                break;
            }
            if !content.is_empty() && !content.ends_with("\n\n") {
                content.push_str("\n\n");
                emit(&mut tx, "delta", json!({"content":"\n\n"}));
            }
        }
        let trimmed = content.trim_end().len();
        content.truncate(trimmed);
        if content.is_empty() && reasoning_content.is_empty() && activity.is_empty() {
            Err(TurnEnd::from(
                "The model returned an empty response. Retry or choose a different model.",
            ))
        } else {
            Ok(())
        }
    };
    let control = async {
        let mut ticks = 0u64;
        let mut renewed = Instant::now();
        let mut session_checked: Option<Instant> = None;
        let mut recheck_session = false;
        let stop = lifecycle::stop_key(id);
        loop {
            let renew = ticks == 0 || renewed.elapsed() >= lifecycle::LEASE_RENEW_INTERVAL;
            let state = if renew {
                lifecycle::heartbeat(&s.store.pool, user.id, id, generation_id).await
            } else {
                lifecycle::check(&s.store.pool, user.id, id, generation_id).await
            };
            match state {
                Ok(Some(end)) => break end,
                Ok(None) if renew => renewed = Instant::now(),
                Ok(None) => {}
                // Ride out a database blip while the lease is still comfortably valid.
                Err(_) if renewed.elapsed() < lifecycle::LEASE_GRACE => {
                    tracing::warn!("could not check chat generation lease; retrying")
                }
                Err(_) => break TurnEnd::from("The response lost its database connection"),
            }
            // A turn in Auto mode outlives the browser session that started it, so
            // research keeps going overnight; Stop or disabling the account ends it.
            let due = recheck_session
                || session_checked.is_none_or(|at| at.elapsed() >= SESSION_CHECK_EVERY);
            if due {
                session_checked = Some(Instant::now());
            }
            let live = match &starter {
                Started::Session(headers)
                    if due && (!unattended || !auto_mode(s.clone()).await) =>
                {
                    Some((
                        s.auth.require(headers, false).await.map(|_| ()),
                        TurnEnd::SessionEnded,
                    ))
                }
                // A token is revoked to stop what it can do, so that holds in
                // Auto mode too.
                Started::Token(token) if due => Some((
                    s.auth.token_live(*token).await,
                    TurnEnd::from("The API token that started this was revoked"),
                )),
                _ => None,
            };
            match live {
                Some((Ok(()), _)) => recheck_session = false,
                Some((Err(e), _)) if e.0 == StatusCode::INTERNAL_SERVER_ERROR => {
                    recheck_session = true
                }
                Some((Err(_), ended)) => break ended,
                None => {}
            }
            ticks += 1;
            // Stop wakes this at once; otherwise it wakes to renew the lease.
            s.wakeups.wait(&stop, lifecycle::LEASE_RENEW_INTERVAL).await;
        }
    };
    // A reply that only writes text is bounded; a tool turn runs until it is done,
    // stopped or stalled.
    let limit = (!tool_turn).then_some(540);
    let bounded = async {
        match limit {
            Some(limit) => tokio::time::timeout(Duration::from_secs(limit), work)
                .await
                .unwrap_or_else(|_| {
                    Err(TurnEnd::Failed(format!(
                        "Generation reached the {}-minute time limit",
                        limit / 60
                    )))
                }),
            None => work.await,
        }
    };
    drop(preparing);
    let mut result = tokio::select! {end=control=>Err(end),result=bounded=>result,_=s.shutdown.cancelled()=>Err(TurnEnd::ShuttingDown)};
    // Steps that never finished are shown as stopped, while the lease still allows writes.
    if !activity.is_empty() {
        for entry in &mut activity {
            if entry["status"] == "running" || entry["status"] == "awaiting" {
                entry["status"] = json!("error");
                entry["detail"] = json!("Stopped before this step finished");
            }
        }
        let _ =
            lifecycle::save_activity(&s.store.pool, user.id, id, generation_id, &activity).await;
    }
    if let Some(toolset) = toolset.take() {
        toolset.close();
    }
    // Browser steps ask for approval too, with or without a connection in the chat.
    if tool_turn {
        connections::finish(&s.store.pool, generation_id).await;
    }
    // A tool turn the model stalls on pauses: what it did is kept and replayed, and
    // "continue" resumes it.
    if tool_turn && result == Err(TurnEnd::Stalled) {
        let note = format!(
            "{}_Paused because the model sent nothing for {} minutes. {PAUSED}",
            if content.trim().is_empty() {
                ""
            } else {
                "\n\n"
            },
            STALL_SECS / 60,
        );
        let note = note
            .trim_start_matches(|c| content.ends_with("\n\n") && c == '\n')
            .to_owned();
        content.push_str(&note);
        emit(&mut tx, "delta", json!({"content":note}));
        result = Ok(());
    }
    let mut status = match &result {
        Ok(()) => "completed",
        Err(end) => end.status(),
    };
    let error = result.as_ref().err().map(TurnEnd::message);
    // Drop any replay note the model copied into its own reply.
    let mut cleaned = store::strip_records(&content);
    // What Chat adds itself (a pause note, the gap between steps) is not counted as
    // it goes in, so a reply already at the limit is cut to what can be saved.
    store::fit_response(&mut cleaned, reasoning_content.len());
    let rewritten = (cleaned != content).then(|| cleaned.clone());
    content = cleaned;
    let mut persisted = Err(ApiError::internal());
    for attempt in 0..3 {
        persisted = tokio::time::timeout(
            Duration::from_secs(5),
            s.store.finish_turn_detailed(
                user.id,
                id,
                generation_id,
                &content,
                &reasoning_content,
                status,
                usage.clone(),
                request_id.as_deref(),
                finish_reason.as_deref(),
                error.as_deref(),
            ),
        )
        .await
        .unwrap_or_else(|_| Err(ApiError::internal()));
        if persisted.is_ok() {
            break;
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(150 * (attempt + 1))).await;
        }
    }
    if persisted.is_err() {
        status = "error";
        tracing::error!("could not persist completed chat generation");
    } else if let Some(model) = &served {
        if let Err(error) = s
            .store
            .set_served_model(turn.assistant_id, model, auto_why.as_ref())
            .await
        {
            tracing::warn!("could not record the model Auto chose: {error:?}");
        }
    }
    for (key, kind) in [
        ("prompt_tokens", "prompt"),
        ("completion_tokens", "completion"),
    ] {
        if let Some(n) = usage
            .as_ref()
            .and_then(|u: &Value| u.get(key))
            .and_then(Value::as_u64)
        {
            s.metrics.tokens.with_label_values(&[kind]).inc_by(n);
        }
    }
    if usage
        .as_ref()
        .and_then(|u: &Value| u.get("total_tokens"))
        .is_none()
    {
        s.metrics.missing_usage.inc();
    }
    s.metrics.streams.dec();
    s.metrics.generations.with_label_values(&[status]).inc();
    s.metrics
        .generation_duration
        .observe(started.elapsed().as_secs_f64());
    drop(permit);
    // Suggested follow-ups would compete with the model's own questions.
    let asked = activity.iter().any(|e| {
        ["question", "workspace", "research", "app", "project"]
            .contains(&e["kind"].as_str().unwrap_or(""))
    });
    // Follow-ups, labels and a better title, and memories from what the person said,
    // each when it is on, are scheduled together before `done`.
    let replied = status == "completed" && !content.is_empty();
    let followups = async {
        if replied && !asked && s.followups_enabled {
            tokio::time::timeout(
                Duration::from_secs(2),
                suggestions::schedule(
                    s.clone(),
                    user.clone(),
                    id,
                    turn.assistant_id,
                    resolved.model.clone(),
                ),
            )
            .await
            .unwrap_or(false)
        } else {
            false
        }
    };
    let title = async { replied && insights::after_reply(&s, &user, id).await };
    let memory = async { replied && memory::after_reply(&s, &user, id, turn.assistant_id).await };
    // Second opinions, for a plain answer Auto chose: one request, no tools run.
    let comparing = async {
        match (first_request.take(), &served) {
            (Some(request), Some(model)) if replied && persisted.is_ok() && requests == 1 => {
                compare::after_reply(
                    &s,
                    &gateway,
                    &user,
                    id,
                    turn.assistant_id,
                    model,
                    auto_why.as_ref(),
                    request,
                )
                .await
            }
            _ => false,
        }
    };
    let (followups, title_pending, memory_pending, compare_pending) =
        tokio::join!(followups, title, memory, comparing);
    if persisted.is_err() {
        emit(
            &mut tx,
            "error",
            json!({"message":"The response could not be saved. Reload before sending another message."}),
        );
    } else if let Some(message) = error {
        emit(&mut tx, "error", json!({"message":message}));
    }
    emit(&mut tx, "done", {
        let mut done = json!({"status":status,"usage":usage,"requestId":request_id,"finishReason":finish_reason,"followUpsStatus":if followups {"pending"}else{"idle"}});
        if let Some(model) = &served {
            done["settings"] = json!({"model":model,"resolvedModel":model,"auto":true,"systemPrompt":parameters.system_prompt,"temperature":parameters.temperature,"topP":parameters.top_p,"maxTokens":parameters.max_tokens,"reasoningEffort":turn.reasoning_effort});
            if let Some(why) = &auto_why {
                done["settings"]["autoWhy"] = why.clone();
            }
        }
        if let Some(text) = rewritten {
            done["content"] = json!(text);
        }
        if title_pending {
            done["titlePending"] = json!(true);
        }
        if memory_pending {
            done["memoryPending"] = json!(true);
        }
        if compare_pending {
            done["comparePending"] = json!(true);
        }
        done
    });
}

#[cfg(test)]
mod loop_watch_tests {
    use super::*;

    #[test]
    fn loop_watch_sees_two_versions_taking_turns() {
        let mut w = LoopWatch::default();
        w.wrote("Cargo.toml", Some("[workspace]"));
        w.wrote("Cargo.toml", Some("[package]"));
        assert!(w.looping().is_none());
        // Back to the first version, then the second again: going in circles,
        // even with a look around in between.
        w.wrote("Cargo.toml", Some("[workspace]"));
        w.ran();
        assert!(w.looping().is_none());
        w.wrote("Cargo.toml", Some("[package]"));
        assert_eq!(w.looping(), Some(("Cargo.toml".to_string(), 4)));
        // Reported once; counting starts over.
        assert!(w.looping().is_none());
    }

    #[test]
    fn loop_watch_sees_one_file_rewritten_with_nothing_run() {
        let mut w = LoopWatch::default();
        for i in 0..LOOP_WRITES - 1 {
            w.wrote("src/main.rs", Some(&format!("v{i}")));
        }
        assert!(w.looping().is_none());
        w.wrote("src/main.rs", Some("v9"));
        assert_eq!(w.looping().map(|(p, _)| p), Some("src/main.rs".to_string()));
    }

    #[test]
    fn loop_watch_lets_write_build_fix_cycles_run() {
        let mut w = LoopWatch::default();
        for i in 0..10 {
            w.wrote("src/main.rs", Some(&format!("attempt {i}")));
            w.wrote("Cargo.toml", Some(&format!("deps {i}")));
            w.ran();
            assert!(w.looping().is_none(), "round {i}");
        }
        // Edits carry no whole file to compare; only their count matters.
        for _ in 0..LOOP_WRITES - 1 {
            w.wrote("src/lib.rs", None);
        }
        w.ran();
        assert!(w.looping().is_none());
    }
}
