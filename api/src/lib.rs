pub mod agents;
pub mod attachments;
pub mod auth;
pub mod autopilot;
pub mod browser;
pub mod code;
pub mod config;
pub mod connections;
pub mod error;
pub mod files;
pub mod gateway;
pub mod groups;
pub mod images;
pub mod knowledge;
pub mod lifecycle;
pub mod mcp;
pub mod metrics;
pub mod models;
pub mod operations;
pub mod projects;
pub mod questions;
pub mod research;
pub mod retention;
pub mod sandbox;
pub mod shares;
pub mod steer;
pub mod store;
pub mod suggestions;

use auth::{Auth, Identity};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json, Router,
};
use error::ApiError;
use futures_util::StreamExt;
use gateway::{Decoder, Gateway};
use metrics::Metrics;
use serde::Deserialize;
use serde_json::{json, Value};
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
use tower_http::{compression::CompressionLayer, services::ServeDir};
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
    pub followups_enabled: bool,
    pub sandboxes: Option<sandbox::Sandboxes>,
    pub browsers: Option<browser::Browsers>,
    pub connections: Option<connections::Connections>,
    pub knowledge: Option<knowledge::Knowledge>,
}
impl AppState {
    pub fn new(auth: Auth, pool: PgPool, gateway: Option<Gateway>, max_streams: usize) -> Self {
        Self {
            auth,
            store: Store::new(pool),
            gateway,
            metrics: Metrics::new(),
            slots: Arc::new(Semaphore::new(max_streams)),
            shutdown: CancellationToken::new(),
            admin_gateway_url: None,
            followups_enabled: true,
            sandboxes: None,
            browsers: None,
            connections: None,
            knowledge: None,
        }
    }
}
pub fn router(state: AppState, static_dir: PathBuf) -> Router {
    let auth = state.auth.router();
    let catalog = models::router(state.clone());
    let operations = operations::router(state.clone());
    let retention = retention::router(state.clone());
    let attachments = attachments::router(state.clone());
    let images = images::router(state.clone());
    let projects = projects::router(state.clone());
    let groups = groups::router(state.clone());
    let shares = shares::router(state.clone());
    let code = code::router(state.clone());
    let research = research::router(state.clone());
    let workspaces = sandbox::router(state.clone());
    let browsing = browser::router(state.clone());
    let connections = connections::router(state.clone());
    let steering = steer::router(state.clone());
    let knowledge = knowledge::router(state.clone());
    let mcp = mcp::router(state.clone());
    let metrics = state.metrics.clone();
    let sandboxes = state.sandboxes.clone();
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
        .route("/api/conversations/{id}/fork", post(fork_turn))
        .route("/api/conversations/{id}/branches", get(branches))
        .route("/api/conversations/bulk", post(bulk))
        .route("/api/conversations/export", post(export))
        .route(
            "/api/conversations/import",
            post(import).layer(DefaultBodyLimit::max(10 * 1024 * 1024)),
        )
        .route("/api/conversations/{id}/messages", post(send))
        .route("/api/conversations/{id}/stop", post(lifecycle::stop))
        .route(
            "/api/conversations/{id}/messages/{message}/select",
            post(select_version),
        )
        .route(
            "/api/{*rest}",
            get(|| async { ApiError::not_found() }).post(|| async { ApiError::not_found() }),
        )
        .with_state(state)
        .merge(auth)
        .merge(catalog)
        .merge(operations)
        .merge(retention)
        .merge(attachments)
        .merge(images)
        .merge(projects)
        .merge(groups)
        .merge(shares)
        .merge(code)
        .merge(research)
        .merge(workspaces)
        .merge(browsing)
        .merge(connections)
        .merge(steering)
        .merge(knowledge)
        .merge(mcp)
        .fallback_service(static_files(static_dir))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(metrics, metrics::record))
        // Outermost: preview hosts are answered before any Chat route or header policy.
        .layer(middleware::from_fn_with_state(
            sandboxes,
            sandbox::preview::dispatch,
        ))
}
static CSP: std::sync::OnceLock<HeaderValue> = std::sync::OnceLock::new();
/// Chat may frame its workspace previews, which live on their own origins.
fn content_security_policy(sandboxes: Option<&sandbox::Sandboxes>) -> HeaderValue {
    let frames = sandboxes
        .map(|sb| {
            format!(
                " frame-src {}://*.{};",
                sb.config().preview_scheme,
                sb.config().preview_domain
            )
        })
        .unwrap_or_default();
    HeaderValue::from_str(&format!("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self';{frames} base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'"))
        .expect("static policy")
}
/// The frontend loads unhashed ES modules by relative URL, so browsers keep a copy
/// but revalidate it (ServeDir answers If-Modified-Since with 304) on every load;
/// other statuses, such as a 404 for an unknown path, keep the no-store default.
/// Compression applies to this service only, away from the streaming API routes.
fn static_files(static_dir: PathBuf) -> Router {
    Router::new()
        .fallback_service(ServeDir::new(static_dir).append_index_html_on_directories(true))
        .layer(middleware::map_response(|mut response: Response| async {
            let status = response.status();
            if status.is_success() || status == StatusCode::NOT_MODIFIED {
                response
                    .headers_mut()
                    .entry(header::CACHE_CONTROL)
                    .or_insert(HeaderValue::from_static("no-cache"));
            }
            response
        }))
        .layer(CompressionLayer::new())
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
async fn metrics_endpoint(State(s): State<AppState>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        s.metrics.render(),
    )
}
async fn capabilities(State(s): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let mut value = json!({"application":"obleth-chat","mode":"connected","liveExecution":s.sandboxes.is_some(),"integrations":{"gateway":{"configured":s.gateway.is_some()},"oidc":{"configured":s.auth.oidc_enabled()},"rag":{"configured":s.knowledge.is_some()},"kubernetes":{"configured":s.sandboxes.is_some()},"slurm":{"configured":false}},"features":{"localAuth":true,"durableConversations":true,"streaming":s.gateway.is_some(),"metrics":true,"modelAccess":true,"history":true,"savedPreferences":true,"workspaces":false,"connections":s.connections.is_some(),"clusters":s.connections.as_ref().is_some_and(|c| c.clusters_enabled()),"autoMode":s.connections.as_ref().is_some_and(|c| c.auto_mode_allowed()),"knowledge":s.knowledge.is_some() && s.gateway.is_some()}});
    let user = s.auth.require(&headers, false).await.ok();
    if user.as_ref().is_some_and(|u| u.is_admin()) {
        value["adminGatewayUrl"] = json!(s.admin_gateway_url);
    }
    if let (Some(sb), Some(user)) = (&s.sandboxes, &user) {
        value["features"]["workspaces"] = json!(sb.allowed(user));
        value["features"]["code"] = json!(sb.allowed(user));
    }
    value["features"]["browser"] = json!(match (&s.browsers, &user) {
        (Some(b), Some(user)) => b.allowed(user),
        _ => false,
    });
    Json(value)
}
async fn jwks(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(s.gateway.as_ref().ok_or_else(not_configured)?.jwks()))
}
pub(crate) fn not_configured() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Gateway integration is not configured".into(),
    )
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
            json!({"conversation":s.store.create_with_effort(u.id,&body.title,&body.model,body.reasoning_effort).await?}),
        ),
    ))
}
async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let u = s.auth.require(&headers, false).await?;
    Ok(Json(json!({"conversation":s.store.get(u.id,id).await?})))
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
async fn import(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<store::ChatArchive>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let u = s.auth.require(&headers, true).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"conversations":s.store.import(u.id,body).await?})),
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
async fn send(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(mut body): Json<Send>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let gateway = s.gateway.clone().ok_or_else(not_configured)?;
    if s.shutdown.is_cancelled() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service is shutting down".into(),
        ));
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
    let rx = launch_turn(s, gateway, user, Some(headers), id, launch).await?;
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

/// Starts a turn and its background worker. Without `headers` the turn runs for
/// no browser session, as Auto mode's nudges do.
pub(crate) async fn launch_turn(
    s: AppState,
    gateway: Gateway,
    user: Identity,
    headers: Option<HeaderMap>,
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
    let toolset = match &s.connections {
        Some(c) if resolved.capabilities.supports_function_calling != Some(false) => {
            Some(c.toolset(user.id, id).await).filter(|t| !t.is_empty())
        }
        _ => None,
    };
    let permit = s.slots.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Chat is at capacity. Try again shortly.".into(),
        )
    })?;
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
            s, gateway, user, headers, id, resolved, turn, tx, permit, workspace, toolset,
        )
        .await;
    });
    Ok(rx)
}
const MAX_AGENT_STEPS: usize = 100;
/// A tool turn runs while the model keeps answering; this only bounds runaways.
const TOOL_TURN_SECS: u64 = 3 * 60 * 60;
/// How long the model may send nothing before a tool turn pauses.
const STALL_SECS: u64 = 10 * 60;
/// A chat in Auto mode runs unattended: it has no step or time limit, and a step
/// the model stalled on is retried this many times before the turn pauses.
const UNATTENDED_STALL_RETRIES: usize = 2;
const AGENT_STEP_LIMIT: &str = "The workspace agent stopped after";
const STALLED: &str = "The model sent nothing for";
/// What Chat tells a model that ends a reply right after changing files.
const CHECK_NOTE: &str = "[Chat: you changed files and have not run anything since, and your draft reply was not shown. Check the work the way the person would before answering: run its tests or build, or make sure its server is running and request the page or endpoint (for example with curl), and fix what fails. Then write your full final reply. If it cannot be checked here, say why in one line.]";
/// Bytes of tool output kept in the request history; older outputs are elided.
const TOOL_HISTORY_BYTES: usize = 160 * 1024;
/// Bytes of tool-call arguments kept in the request history, such as the files
/// the model wrote; older ones are elided so a long turn's requests stay bounded.
const TOOL_ARGUMENT_BYTES: usize = 160 * 1024;
/// Steps of the current turn kept in the request; a longer turn drops its oldest.
const KEPT_STEPS: usize = 150;
/// Steps in a row that may stop at the output limit before the turn gives up.
const MAX_CUTS: u32 = 3;
/// Steps in a row that may lose their connection to the model and be tried again.
const MAX_DROPS: u32 = 2;

/// Whether a stream's error event reports a server-side failure (a 5xx code),
/// which is worth trying again, rather than a problem with the request.
fn server_failure(error: &Value) -> bool {
    let code = match &error["code"] {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    };
    code.is_some_and(|c| (500..600).contains(&c))
}
/// Bytes a tool step may think before it writes anything. Past it, the step is
/// stopped and tried again at low effort, up to MAX_OVERTHINKS times in a row:
/// some models plan a whole program in their thinking and never start writing it.
const STEP_THINKING_BYTES: usize = 64 * 1024;
const MAX_OVERTHINKS: u32 = 2;
const OVERTHOUGHT: &str = "[Chat: you thought for a long time without writing anything, so that step was stopped. Stop planning everything up front. Take the next small step now, such as writing the first part of a file with write_file, then continue in parts and check as you go.]";
const STEPS_DROPPED: &str = "[Earlier steps of this turn were dropped to save context:";
const ARGUMENTS_OMITTED: &str =
    r#"{"omitted":"Earlier arguments omitted to save context. The tool result still applies."}"#;

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Merges streamed tool-call fragments, which arrive keyed by index.
/// The file a streaming write_file or edit_file call is producing: its path and the
/// last lines typed so far.
fn draft_of(call: &ToolCall) -> Option<Value> {
    let field = match call.name.as_str() {
        "write_file" => "content",
        "edit_file" => "new_text",
        _ => return None,
    };
    let path = partial_string(&call.arguments, "path").unwrap_or_default();
    let text = partial_string(&call.arguments, field)?;
    let lines = text.lines().count();
    let tail: Vec<&str> = text.lines().rev().take(8).collect();
    let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    Some(
        json!({"id":call.id,"tool":call.name,"path":path.chars().take(200).collect::<String>(),"lines":lines,"tail":tail.chars().rev().take(1200).collect::<String>().chars().rev().collect::<String>()}),
    )
}

/// The value of `"key": "…"` in JSON that may still be arriving, unescaped as far as
/// it goes.
fn partial_string(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => {}
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

pub(crate) fn accumulate_tool_calls(
    calls: &mut Vec<ToolCall>,
    deltas: &[Value],
) -> Result<(), String> {
    for (position, delta) in deltas.iter().enumerate() {
        let index = delta
            .get("index")
            .and_then(Value::as_u64)
            .map_or(position, |i| i as usize);
        if index >= 16 {
            return Err("The model requested too many tools at once".into());
        }
        while calls.len() <= index {
            calls.push(ToolCall {
                id: format!("call_{}", calls.len()),
                name: String::new(),
                arguments: String::new(),
            });
        }
        let call = &mut calls[index];
        if let Some(id) = delta
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128)
        {
            call.id = id.to_owned();
        }
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str) {
            call.name.push_str(name);
        }
        if let Some(arguments) = delta.pointer("/function/arguments").and_then(Value::as_str) {
            call.arguments.push_str(arguments);
        }
        if call.name.len() > 64 || call.arguments.len() > 2 * 1024 * 1024 {
            return Err("The model sent an oversized tool call".into());
        }
    }
    Ok(())
}

/// Tool calls a model wrote as text because its server passed them through
/// unparsed: Llama's `<function=name>{…}</function>`, Hermes and Qwen's
/// `<tool_call>{…}</tool_call>`, Mistral's `[TOOL_CALLS][…]` and Llama 3's
/// `<|python_tag|>{…}`. They count only when every one names an offered tool;
/// returns the text without them, and the calls.
pub(crate) fn text_tool_calls(text: &str, offered: &[String]) -> Option<(String, Vec<ToolCall>)> {
    const MARKERS: [&str; 4] = [
        "<function=",
        "<tool_call>",
        "[TOOL_CALLS]",
        "<|python_tag|>",
    ];
    fn json_at(s: &str) -> Option<(Value, usize)> {
        let lead = s.len() - s.trim_start().len();
        let mut values = serde_json::Deserializer::from_str(&s[lead..]).into_iter::<Value>();
        if let Some(Ok(value)) = values.next() {
            return Some((value, lead + values.byte_offset()));
        }
        // Models often put raw newlines and tabs inside strings, as in a file's content.
        let end = balanced(&s[lead..])?;
        let value = serde_json::from_str(&escape_controls(&s[lead..lead + end])).ok()?;
        Some((value, lead + end))
    }
    fn named(value: &Value) -> Option<(String, Value)> {
        let name = value["name"].as_str()?.to_owned();
        let arguments = value
            .get("arguments")
            .or_else(|| value.get("parameters"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        Some((name, arguments))
    }
    let mut found: Vec<(String, Value)> = vec![];
    let mut kept = String::new();
    let mut rest = text;
    while let Some((at, marker)) = MARKERS
        .iter()
        .filter_map(|m| rest.find(m).map(|i| (i, *m)))
        .min_by_key(|(i, _)| *i)
    {
        let after = &rest[at + marker.len()..];
        let parsed = match marker {
            "<function=" => after.find('>').and_then(|close| {
                let name = after[..close].trim().to_owned();
                json_at(&after[close + 1..])
                    .map(|(args, used)| (vec![(name, args)], close + 1 + used, Some("</function>")))
            }),
            "[TOOL_CALLS]" => json_at(after).map(|(value, used)| {
                let items = match value {
                    Value::Array(items) => items,
                    one => vec![one],
                };
                (items.iter().filter_map(named).collect(), used, None)
            }),
            _ => json_at(after).map(|(value, used)| {
                let closing = (marker == "<tool_call>").then_some("</tool_call>");
                (named(&value).into_iter().collect(), used, closing)
            }),
        };
        match parsed.filter(|(calls, _, _)| !calls.is_empty()) {
            Some((calls, used, closing)) => {
                kept.push_str(&rest[..at]);
                let mut tail = &after[used..];
                if let Some(end) = closing.and_then(|c| tail.trim_start().strip_prefix(c)) {
                    tail = end;
                }
                found.extend(calls);
                rest = tail;
            }
            None => {
                kept.push_str(&rest[..at + marker.len()]);
                rest = after;
            }
        }
    }
    kept.push_str(rest);
    if found.is_empty() {
        // Llama 4's pythonic form: [name(key="value", ...), ...]
        let (start, end, calls) = pythonic_calls(text, offered)?;
        kept = format!("{}{}", &text[..start], &text[end..]);
        found = calls;
    }
    if found.iter().any(|(name, _)| !offered.contains(name)) {
        return None;
    }
    let calls = found
        .into_iter()
        .enumerate()
        .map(|(i, (name, arguments))| ToolCall {
            id: format!("text_call_{i}"),
            name,
            arguments: match arguments {
                Value::String(raw) => raw,
                other => other.to_string(),
            },
        })
        .collect();
    Some((kept.trim().to_owned(), calls))
}

/// Where the JSON object or array at the start of `s` ends, counting brackets
/// outside strings; a string may hold raw control characters.
fn balanced(s: &str) -> Option<usize> {
    if !s.starts_with(['{', '[']) {
        return None;
    }
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for (i, c) in s.char_indices() {
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// JSON text with control characters inside strings escaped.
fn escape_controls(raw: &str) -> String {
    let (mut out, mut quoted, mut escaped) = (String::with_capacity(raw.len()), false, false);
    for c in raw.chars() {
        if quoted && !escaped && c.is_control() {
            match c {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                other => out.push_str(&format!("\\u{:04x}", other as u32)),
            }
            continue;
        }
        if quoted {
            escaped = !escaped && c == '\\';
            if !escaped && c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        }
        out.push(c);
    }
    out
}

/// A tool's name and its arguments, as parsed from text.
type NamedCall = (String, Value);

/// The first `[name(key=value, ...), ...]` list in `text` whose calls all name
/// offered tools, with its byte range. Values are Python literals: strings
/// (triple-quoted too), numbers, True, False, None, lists and dicts.
fn pythonic_calls(text: &str, offered: &[String]) -> Option<(usize, usize, Vec<NamedCall>)> {
    struct P<'a> {
        s: &'a str,
        i: usize,
    }
    impl P<'_> {
        fn rest(&self) -> &str {
            &self.s[self.i..]
        }
        fn space(&mut self) {
            self.i = self.s.len() - self.rest().trim_start().len();
        }
        fn eat(&mut self, token: &str) -> bool {
            self.space();
            let ok = self.rest().starts_with(token);
            if ok {
                self.i += token.len();
            }
            ok
        }
        fn ident(&mut self) -> Option<String> {
            self.space();
            let n = self
                .rest()
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(self.rest().len());
            if n == 0 || self.rest().starts_with(|c: char| c.is_ascii_digit()) {
                return None;
            }
            let name = self.rest()[..n].to_owned();
            self.i += n;
            Some(name)
        }
        fn string(&mut self) -> Option<String> {
            self.space();
            let quote = ["\"\"\"", "'''", "\"", "'"]
                .into_iter()
                .find(|q| self.rest().starts_with(q))?;
            self.i += quote.len();
            let body = self.rest();
            let mut out = String::new();
            let mut chars = body.char_indices();
            while let Some((at, c)) = chars.next() {
                if body[at..].starts_with(quote) {
                    self.i += at + quote.len();
                    return Some(out);
                }
                if c == '\\' {
                    match chars.next()?.1 {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '0' => out.push('\0'),
                        other => out.push(other),
                    }
                } else {
                    out.push(c);
                }
            }
            None
        }
        fn value(&mut self) -> Option<Value> {
            self.space();
            if self.rest().starts_with(['"', '\'']) {
                return self.string().map(Value::String);
            }
            for (word, value) in [
                ("True", json!(true)),
                ("False", json!(false)),
                ("None", Value::Null),
            ] {
                if self.rest().starts_with(word) {
                    self.i += word.len();
                    return Some(value);
                }
            }
            if self.eat("[") {
                let mut items = vec![];
                while !self.eat("]") {
                    items.push(self.value()?);
                    if !self.eat(",") && !self.rest().trim_start().starts_with(']') {
                        return None;
                    }
                }
                return Some(Value::Array(items));
            }
            if self.eat("{") {
                let mut map = serde_json::Map::new();
                while !self.eat("}") {
                    let key = self.string()?;
                    if !self.eat(":") {
                        return None;
                    }
                    map.insert(key, self.value()?);
                    if !self.eat(",") && !self.rest().trim_start().starts_with('}') {
                        return None;
                    }
                }
                return Some(Value::Object(map));
            }
            let n = self
                .rest()
                .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
                .unwrap_or(self.rest().len());
            let number = serde_json::from_str::<serde_json::Number>(&self.rest()[..n]).ok()?;
            self.i += n;
            Some(Value::Number(number))
        }
        fn call(&mut self) -> Option<(String, Value)> {
            let name = self.ident()?;
            if !self.eat("(") {
                return None;
            }
            let mut args = serde_json::Map::new();
            while !self.eat(")") {
                let key = self.ident()?;
                if !self.eat("=") {
                    return None;
                }
                args.insert(key, self.value()?);
                if !self.eat(",") && !self.rest().trim_start().starts_with(')') {
                    return None;
                }
            }
            Some((name, Value::Object(args)))
        }
    }
    for (start, _) in text.match_indices('[') {
        let after = text[start + 1..].trim_start();
        if !offered.iter().any(|name| {
            after.starts_with(name.as_str()) && after[name.len()..].trim_start().starts_with('(')
        }) {
            continue;
        }
        let mut p = P {
            s: text,
            i: start + 1,
        };
        let mut calls = vec![];
        let parsed = loop {
            match p.call() {
                Some(call) => calls.push(call),
                None => break false,
            }
            if p.eat("]") {
                break true;
            }
            if !p.eat(",") {
                break false;
            }
        };
        if parsed && calls.iter().all(|(name, _)| offered.contains(name)) {
            return Some((start, p.i, calls));
        }
    }
    None
}

/// Lets a turn run for any number of steps: past KEPT_STEPS, the oldest step and
/// the tool results and notes after it leave the request, and one note says so.
fn drop_old_steps(payload: &mut Value) {
    let Some(messages) = payload["messages"].as_array_mut() else {
        return;
    };
    let step = |m: &Value| m["role"] == "assistant" && m["tool_calls"].is_array();
    let starts: Vec<usize> = (0..messages.len())
        .filter(|&i| step(&messages[i]))
        .collect();
    if starts.len() <= KEPT_STEPS {
        return;
    }
    let (first, keep) = (starts[0], starts[starts.len() - KEPT_STEPS]);
    let note = |m: &Value| {
        m["content"]
            .as_str()
            .is_some_and(|c| c.starts_with(STEPS_DROPPED))
    };
    // An earlier note sits just before the first step; count what it already covers.
    let (from, before) = match first.checked_sub(1).filter(|&i| note(&messages[i])) {
        Some(i) => {
            let n = messages[i]["content"].as_str().unwrap_or("")[STEPS_DROPPED.len()..]
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
            (i, n)
        }
        None => (first, 0),
    };
    let dropped = before + starts.len() - KEPT_STEPS;
    messages.splice(
        from..keep,
        [json!({"role":"user","content":format!("{STEPS_DROPPED} {dropped} of them. Files, runs and research_status show where things stand; check them rather than relying on memory.]")})],
    );
}

/// Keeps agent requests bounded: the oldest tool outputs are replaced by a note
/// once their total passes TOOL_HISTORY_BYTES, and the oldest tool-call arguments
/// once theirs passes TOOL_ARGUMENT_BYTES.
fn trim_tool_history(payload: &mut Value) {
    let Some(messages) = payload["messages"].as_array_mut() else {
        return;
    };
    let mut total: usize = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().map_or(0, str::len))
        .sum();
    for message in messages.iter_mut().filter(|m| m["role"] == "tool") {
        if total <= TOOL_HISTORY_BYTES {
            break;
        }
        let size = message["content"].as_str().map_or(0, str::len);
        if size > 200 {
            total -= size;
            message["content"] = json!(
                "[Earlier output omitted to save context. Run the tool again if you need it.]"
            );
        }
    }
    // The files and commands the model sent: in a long turn, the oldest go first.
    let mut arguments: Vec<&mut Value> = messages
        .iter_mut()
        .filter(|m| m["role"] == "assistant")
        .filter_map(|m| m["tool_calls"].as_array_mut())
        .flat_map(|calls| calls.iter_mut())
        .map(|call| &mut call["function"]["arguments"])
        .collect();
    let mut total: usize = arguments
        .iter()
        .map(|a| a.as_str().map_or(0, str::len))
        .sum();
    for argument in arguments.iter_mut() {
        if total <= TOOL_ARGUMENT_BYTES {
            break;
        }
        let size = argument.as_str().map_or(0, str::len);
        if size > ARGUMENTS_OMITTED.len() {
            total -= size;
            **argument = json!(ARGUMENTS_OMITTED);
        }
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

#[allow(clippy::too_many_arguments)]
async fn run_generation(
    s: AppState,
    gateway: Gateway,
    user: Identity,
    headers: Option<HeaderMap>,
    id: Uuid,
    resolved: models::ResolvedRequest,
    turn: Turn,
    tx: mpsc::Sender<Result<Event, Infallible>>,
    permit: tokio::sync::OwnedSemaphorePermit,
    workspace: Option<sandbox::Sandbox>,
    mut toolset: Option<connections::Toolset>,
) {
    let workspace_turn = workspace.is_some();
    // The chat's project: instructions, and collections and saved items it also searches.
    let project = projects::for_conversation(&s.store.pool, user.id, id).await;
    // Passages from the chat's knowledge collections, found before the model starts.
    let mut knowledge_turn = match &s.knowledge {
        Some(k) => {
            let messages: Vec<(String, String)> = turn
                .messages
                .iter()
                .map(|m| (m.role.clone(), m.content.clone()))
                .collect();
            let tools = resolved.capabilities.supports_function_calling != Some(false);
            let library = resolved.capabilities.supports_function_calling == Some(true);
            k.prepare(
                &gateway,
                &user,
                id,
                &messages,
                tools,
                library,
                project.as_ref(),
            )
            .await
        }
        None => None,
    };
    // Models with tools may ask the person to choose before going on.
    let asks = resolved.capabilities.supports_function_calling == Some(true);
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
    // Auto mode is for work nobody watches, such as research: it runs until done.
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
    let mut finish_reason: Option<String> = None;
    let mut first = true;
    let generation_id = turn.generation_id;
    let mut tx = Some(tx);
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
            if files.iter().all(|f| f.meta.kind == "text") {
                let mut text = m.content.clone();
                for file in files {
                    if let Some(context) = file.text_context() {
                        text.push_str(&context);
                    }
                }
                json!(text)
            } else {
                let mut parts = vec![json!({"type":"text","text":m.content})];
                parts.extend(files.iter().map(attachments::Attachment::content_part));
                json!(parts)
            }
        } else {
            json!(m.content)
        };
        json!({"role":m.role,"content":content})
    }));
    let workspace = workspace.zip(s.sandboxes.clone());
    let mut preamble = vec![];
    if let Some(project) = &project {
        preamble.push(project.prompt());
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
    if let Some(prompt) = knowledge_turn
        .as_ref()
        .map(|k| k.prompt())
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
    if recalled {
        preamble.push(format!("Earlier replies in this conversation may end with a bracketed note that starts with \"{}\". Chat adds it so you remember which tools ran; you did not write it and the person does not see it. Never write such a note yourself: your tools report their own results.", store::RECORD));
    }
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
        tools.push(knowledge::chat::save_definition());
    }
    if asks {
        tools.push(questions::definition());
    }
    if offers_workspace {
        tools.push(questions::workspace_definition());
    }
    // Research can be suggested in a chat that isn't part of a project's research.
    let offers_research = asks
        && research_standing.is_none()
        && s.sandboxes.as_ref().is_some_and(|sb| sb.allowed(&user));
    // Its button keeps the chat's project when that project can take the research.
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
    if research_tools {
        tools.extend(research::tools::definitions());
    }
    if browsing.is_some() {
        tools.extend(browser::tools::definitions());
        if asks {
            tools.push(browser::tools::handoff_definition());
        }
    }
    // People who may use the browser can be asked to turn it on for this chat.
    let offers_browser = asks && browsing.is_none() && browser_allowed;
    if offers_browser {
        tools.push(browser::tools::request_definition());
    }
    if !tools.is_empty() {
        payload["tools"] = Value::Array(tools);
    }
    let mut activity: Vec<Value> = vec![];
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
    let work = async {
        let mut steps = 0;
        let mut checkpoint = tokio::time::interval(Duration::from_millis(750));
        checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut saved_bytes = 0;
        let mut saved_at = Instant::now();
        let mut drafted = Instant::now();
        let mut checkpoints = 0;
        let mut cuts = 0;
        let mut drops = 0;
        let mut overthinks = 0;
        // Files changed since the model last ran something, and whether Chat has
        // already asked it to check its work this turn.
        let mut unchecked = false;
        let mut gated = false;
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
            let mut calls: Vec<ToolCall> = vec![];
            let mut stalls = 0;
            let response = loop {
                match tokio::time::timeout(
                    Duration::from_secs(STALL_SECS),
                    gateway.chat(&user, id, payload.clone()),
                )
                .await
                {
                    Ok(response) => break response.map_err(|e| e.1)?,
                    Err(_) if unattended && stalls < UNATTENDED_STALL_RETRIES => stalls += 1,
                    Err(_) => return Err(format!("{STALLED} {} minutes", STALL_SECS / 60)),
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
                            return Err(format!("{STALLED} {} minutes", STALL_SECS / 60));
                        }
                        let total=content.len()+reasoning_content.len();
                        if lifecycle::checkpoint_due(total,saved_bytes,saved_at.elapsed()) {
                            match lifecycle::checkpoint(&s.store.pool,user.id,id,generation_id,&content,&reasoning_content,request_id.as_deref()).await {
                                Ok(())=>{saved_bytes=total;saved_at=Instant::now();}
                                // A transient database error retries on a later tick; the
                                // control loop bounds how long work runs without the database.
                                Err(e) if e.0==StatusCode::INTERNAL_SERVER_ERROR=>tracing::warn!("could not checkpoint chat generation; retrying"),
                                Err(_)=>return Err("The response could not save progress".to_string()),
                            }
                        }
                        continue;
                    }
                };
                let Ok(chunk) = chunk else { break };
                for event in decoder.push(&chunk).map_err(str::to_string)? {
                    if event == "[DONE]" {
                        done = true;
                        break 'outer;
                    }
                    let v: Value = serde_json::from_str(&event)
                        .map_err(|_| "Gateway returned an invalid stream event".to_string())?;
                    if v.get("error").is_some() {
                        let why = gateway::reason(event.as_bytes());
                        // A server-side failure partway through, such as the upstream
                        // connection breaking, is a drop: the step is tried again.
                        if server_failure(&v["error"]) {
                            tracing::warn!(
                                reason = why.as_deref().unwrap_or(""),
                                "gateway stream broke off"
                            );
                            break 'outer;
                        }
                        tracing::warn!(
                            reason = why.as_deref().unwrap_or(""),
                            "gateway reported an error during generation"
                        );
                        return Err(match why {
                            Some(why) => {
                                format!("Gateway reported an error during generation: {why}")
                            }
                            None => "Gateway reported an error during generation".to_string(),
                        });
                    }
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
                            emit(&mut tx, "model", json!({"model":model}));
                        }
                    }
                    if let Some(u) = v.get("usage").filter(|v| v.is_object()) {
                        let mut clean = usage
                            .take()
                            .and_then(|u: Value| u.as_object().cloned())
                            .unwrap_or_default();
                        for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
                            if let Some(n) = u
                                .get(key)
                                .and_then(Value::as_u64)
                                .filter(|n| *n <= 1_000_000_000)
                            {
                                // Agent steps are separate requests; the turn reports their sum.
                                let previous = if steps > 1 {
                                    clean.get(key).and_then(Value::as_u64).unwrap_or(0)
                                } else {
                                    0
                                };
                                clean.insert(key.into(), json!(previous + n));
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
                                clean.insert(last.into(), json!(n));
                            }
                        }
                        clean.insert("steps".into(), json!(steps.max(1)));
                        usage = Some(Value::Object(clean));
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
                                        .to_string(),
                                );
                            }
                            accumulate_tool_calls(&mut calls, deltas)?;
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
                                return Err(
                                    "The response exceeded the 1 MiB storage limit".to_string()
                                );
                            }
                            if first {
                                s.metrics
                                    .first_token
                                    .observe(started.elapsed().as_secs_f64());
                                first = false;
                            }
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
                    return Err("The model kept thinking without writing anything. Try a lower thinking effort or another model.".to_string());
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
                    return Err("The connection to the model kept dropping".to_string());
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
                    return Err(format!("The model kept stopping at {limit} in the middle of a tool call. Raise the output limit in Parameters, or lower thinking effort."));
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
                break;
            }
            // Turning Auto mode off during a long turn brings the limit back.
            if steps >= MAX_AGENT_STEPS && !(unattended && auto_mode(s.clone()).await) {
                return Err(format!("{AGENT_STEP_LIMIT} {MAX_AGENT_STEPS} steps"));
            }
            let text = content[step_start..].to_owned();
            payload["messages"].as_array_mut().unwrap().push(json!({
                "role":"assistant",
                "content": if text.is_empty() { Value::Null } else { json!(text) },
                "tool_calls": calls.iter().map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}})).collect::<Vec<_>>(),
            }));
            let workspace_call = calls.iter().any(|c| {
                c.name != knowledge::chat::TOOL
                    && c.name != knowledge::chat::SAVE_TOOL
                    && c.name != questions::TOOL
                    && c.name != questions::WORKSPACE_TOOL
                    && c.name != questions::RESEARCH_TOOL
                    && c.name != browser::tools::HANDOFF
                    && c.name != browser::tools::REQUEST
                    && c.name != agents::TOOL
                    && !(browsing.is_some() && browser::tools::is_tool(&c.name))
                    && toolset.as_ref().and_then(|t| t.find(&c.name)).is_none()
            });
            let mut running = None;
            if let Some((sandbox, sandboxes)) = workspace.as_ref().filter(|_| workspace_call) {
                // The first workspace tool of a turn may need to start or wake the pod.
                let sandbox = sandboxes.ensure_running(sandbox).await?;
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
                if call.name == questions::WORKSPACE_TOOL || call.name == questions::RESEARCH_TOOL {
                    let research = call.name == questions::RESEARCH_TOOL;
                    let offered = if research {
                        offers_research
                    } else {
                        offers_workspace
                    };
                    let output = if offered && !asked {
                        asked = true;
                        let entry = if research {
                            questions::research_entry(&call.id, &call.arguments, research_here)
                        } else {
                            questions::workspace_entry(&call.id, &call.arguments)
                        };
                        record(&mut activity, &entry, true);
                        emit(&mut tx, "tool", entry);
                        if research {
                            questions::RESEARCH_SHOWN
                        } else {
                            questions::WORKSPACE_SHOWN
                        }
                        .to_string()
                    } else if asked {
                        "Ask for one thing at a time: your reply already ends with a question."
                            .to_string()
                    } else {
                        format!("There is no tool named {}.", call.name)
                    };
                    payload["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"role":"tool","tool_call_id":call.id,"content":output}));
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
                        "Ask for one thing at a time: your reply already ends with a question."
                            .to_string()
                    } else {
                        format!("There is no tool named {}.", call.name)
                    };
                    payload["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"role":"tool","tool_call_id":call.id,"content":output}));
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
                            payload["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":call.id,"content":"The person declined this, so nothing was clicked or submitted. Do not try it again another way unless they ask."}));
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
                    payload["messages"].as_array_mut().unwrap().push(
                        json!({"role":"tool","tool_call_id":call.id,"content":outcome.output}),
                    );
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
                        Ok(_) => format!("There is no tool named {}.", call.name),
                        Err(problem) => problem,
                    };
                    payload["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"role":"tool","tool_call_id":call.id,"content":output}));
                    continue;
                }
                if call.name == knowledge::chat::TOOL {
                    let (Some(k), Some(found)) = (&s.knowledge, knowledge_turn.as_mut()) else {
                        payload["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":call.id,"content":format!("There is no tool named {}.", call.name)}));
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
                    payload["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"role":"tool","tool_call_id":call.id,"content":output}));
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
                        _ => format!("There is no tool named {}.", call.name),
                    };
                    payload["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"role":"tool","tool_call_id":call.id,"content":output}));
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
                        entry["status"] = json!("awaiting");
                        entry["detail"] = json!(connections::arguments_preview(&call.arguments));
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
                            payload["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":call.id,"content":"The person declined this tool call, so it did not run. Do not try it again unless they ask."}));
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
                    emit(&mut tx, "tool", entry.clone());
                    let began = Instant::now();
                    let outcome = set.call(index, &call.arguments).await;
                    entry["status"] = json!(if outcome.ok { "completed" } else { "error" });
                    entry["detail"] = json!(outcome.detail);
                    entry["durationMs"] = json!(began.elapsed().as_millis() as u64);
                    record(&mut activity, &entry, false);
                    emit(&mut tx, "tool", entry);
                    payload["messages"].as_array_mut().unwrap().push(
                        json!({"role":"tool","tool_call_id":call.id,"content":outcome.output}),
                    );
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
                    payload["messages"].as_array_mut().unwrap().push(
                        json!({"role":"tool","tool_call_id":call.id,"content":outcome.output}),
                    );
                    continue;
                }
                let Some((sandbox, sandboxes)) = &running else {
                    payload["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":call.id,"content":format!("There is no tool named {}.", call.name)}));
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
                } else if matches!(
                    call.name.as_str(),
                    "run_command" | "run_in_terminal" | "start_server" | "experiment_run"
                ) {
                    unchecked = false;
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
                payload["messages"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"role":"tool","tool_call_id":call.id,"content":outcome.output}));
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
            drop_old_steps(&mut payload);
            trim_tool_history(&mut payload);
            if let Err(e) =
                lifecycle::save_activity(&s.store.pool, user.id, id, generation_id, &activity).await
            {
                if e.0 != StatusCode::INTERNAL_SERVER_ERROR {
                    return Err("The response could not save progress".to_string());
                }
            }
            // The reply ends at the questions; the answers start the next turn.
            if asked {
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
            Err(
                "The model returned an empty response. Retry or choose a different model."
                    .to_string(),
            )
        } else {
            Ok(())
        }
    };
    let control = async {
        let mut ticks = 0u64;
        let mut renewed = Instant::now();
        let mut recheck_session = false;
        loop {
            let renew = ticks == 0 || renewed.elapsed() >= lifecycle::LEASE_RENEW_INTERVAL;
            let state = if renew {
                lifecycle::heartbeat(&s.store.pool, user.id, id, generation_id).await
            } else {
                lifecycle::check(&s.store.pool, user.id, id, generation_id).await
            };
            match state {
                Ok(Some(reason)) => break reason.to_owned(),
                Ok(None) if renew => renewed = Instant::now(),
                Ok(None) => {}
                // Ride out a database blip while the lease is still comfortably valid.
                Err(_) if renewed.elapsed() < lifecycle::LEASE_GRACE => {
                    tracing::warn!("could not check chat generation lease; retrying")
                }
                Err(_) => break "The response lost its database connection".into(),
            }
            // A turn in Auto mode outlives the browser session that started it, so
            // research keeps going overnight; Stop or disabling the account ends it.
            let due = ticks.is_multiple_of(15) || recheck_session;
            let attended = due && (!unattended || !auto_mode(s.clone()).await);
            if let Some(headers) = headers.as_ref().filter(|_| attended) {
                match s.auth.require(headers, false).await {
                    Ok(_) => recheck_session = false,
                    Err(e) if e.0 == StatusCode::INTERNAL_SERVER_ERROR => recheck_session = true,
                    Err(_) => break "Sign-in session ended".into(),
                }
            }
            ticks += 1;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };
    let limit = (!unattended).then_some(if tool_turn { TOOL_TURN_SECS } else { 540 });
    let bounded = async {
        match limit {
            Some(limit) => tokio::time::timeout(Duration::from_secs(limit), work)
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "Generation reached the {}-minute time limit",
                        limit / 60
                    ))
                }),
            None => work.await,
        }
    };
    let mut result = tokio::select! {reason=control=>Err(reason),result=bounded=>result,_=s.shutdown.cancelled()=>Err("Service shutting down".into())};
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
        connections::finish(&s.store.pool, generation_id).await;
    }
    // A tool turn that stalls or reaches a runaway limit pauses: what it did is kept
    // and replayed, and "continue" resumes it.
    if tool_turn {
        if let Err(reason) = &result {
            let why = if reason.starts_with(AGENT_STEP_LIMIT) {
                Some(format!(
                    "Paused after {MAX_AGENT_STEPS} {}steps",
                    if workspace_turn { "workspace " } else { "" }
                ))
            } else if reason.starts_with("Generation reached the") {
                Some(format!(
                    "Paused after {} hours of work",
                    TOOL_TURN_SECS / 3600
                ))
            } else if reason.starts_with(STALLED) {
                Some(format!(
                    "Paused because the model sent nothing for {} minutes",
                    STALL_SECS / 60
                ))
            } else {
                None
            };
            if let Some(why) = why {
                let note = format!(
                    "{}_{why}. Say \"continue\" to pick up where it left off._",
                    if content.trim().is_empty() {
                        ""
                    } else {
                        "\n\n"
                    },
                );
                let note = note
                    .trim_start_matches(|c| content.ends_with("\n\n") && c == '\n')
                    .to_owned();
                content.push_str(&note);
                emit(&mut tx, "delta", json!({"content":note}));
                result = Ok(());
            }
        }
    }
    let mut status = match &result {
        Ok(()) => "completed",
        Err(e)
            if e == "Response stopped"
                || e == "Response is no longer active"
                || e == "Service shutting down"
                || e == "Sign-in session ended" =>
        {
            "interrupted"
        }
        _ => "error",
    };
    // Drop any replay note the model copied into its own reply.
    let cleaned = store::strip_records(&content);
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
                result.as_ref().err().map(String::as_str),
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
        if let Err(error) = s.store.set_served_model(turn.assistant_id, model).await {
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
    let asked = activity
        .iter()
        .any(|e| ["question", "workspace", "research"].contains(&e["kind"].as_str().unwrap_or("")));
    let followups = if status == "completed" && !content.is_empty() && !asked && s.followups_enabled
    {
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
    };
    if persisted.is_err() {
        emit(
            &mut tx,
            "error",
            json!({"message":"The response could not be saved. Reload before sending another message."}),
        );
    } else if let Err(message) = result {
        emit(&mut tx, "error", json!({"message":message}));
    }
    emit(&mut tx, "done", {
        let mut done = json!({"status":status,"usage":usage,"requestId":request_id,"finishReason":finish_reason,"followUpsStatus":if followups {"pending"}else{"idle"}});
        if let Some(model) = &served {
            done["settings"] = json!({"model":model,"resolvedModel":model,"auto":true,"systemPrompt":parameters.system_prompt,"temperature":parameters.temperature,"topP":parameters.top_p,"maxTokens":parameters.max_tokens,"reasoningEffort":turn.reasoning_effort});
        }
        if let Some(text) = rewritten {
            done["content"] = json!(text);
        }
        done
    });
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    #[test]
    fn only_server_side_stream_errors_are_tried_again() {
        assert!(server_failure(
            &json!({"message":"upstream stream ended","code":502})
        ));
        assert!(server_failure(
            &json!({"message":"overloaded","code":"503"})
        ));
        assert!(!server_failure(
            &json!({"message":"bad tool schema","code":400})
        ));
        assert!(!server_failure(&json!({"message":"no code"})));
        assert!(!server_failure(&json!("boom")));
    }

    #[test]
    fn tool_calls_written_as_text_run_when_they_name_offered_tools() {
        let offered = vec!["delegate".to_string(), "write_file".to_string()];
        let (text, calls) = text_tool_calls(
            "Let me split this.\n<function=delegate>{\"tasks\": [{\"task\": \"a\"}]}</function>",
            &offered,
        )
        .unwrap();
        assert_eq!(text, "Let me split this.");
        assert_eq!(calls[0].name, "delegate");
        assert_eq!(calls[0].arguments, r#"{"tasks":[{"task":"a"}]}"#);
        let (text, calls) = text_tool_calls(
            "<tool_call>\n{\"name\": \"write_file\", \"arguments\": {\"path\": \"a.txt\"}}\n</tool_call>",
            &offered,
        )
        .unwrap();
        assert_eq!((text.as_str(), calls.len()), ("", 1));
        let (_, calls) = text_tool_calls(
            r#"[TOOL_CALLS][{"name":"write_file","arguments":{"path":"a"}},{"name":"delegate","arguments":"{}"}]"#,
            &offered,
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].arguments, "{}");
        // An unknown tool, prose that mentions the syntax, or broken JSON stays text.
        assert!(text_tool_calls(r#"<function=rm_rf>{}</function>"#, &offered).is_none());
        assert!(text_tool_calls("Models write <function=name> tags.", &offered).is_none());
        assert!(text_tool_calls("<tool_call>{not json</tool_call>", &offered).is_none());
        // Raw newlines inside a JSON string, as models write file contents.
        let (_, calls) = text_tool_calls(
            "<function=write_file>{\"path\": \"a.py\", \"content\": \"import re\n\tx = 1\n\"}</function>",
            &offered,
        )
        .unwrap();
        assert_eq!(
            calls[0].arguments,
            r#"{"content":"import re\n\tx = 1\n","path":"a.py"}"#
        );
        // Llama 4's pythonic form, with a triple-quoted string and a nested list.
        let (text, calls) = text_tool_calls(
            "First the page.\n\n[write_file(path=\"index.html\", content='''<p class=\"x\">it's</p>\n'''), delegate(tasks=[{\"task\": \"a\", \"n\": 2}], wait=True)]",
            &offered,
        )
        .unwrap();
        assert_eq!(text, "First the page.");
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].arguments,
            r#"{"content":"<p class=\"x\">it's</p>\n","path":"index.html"}"#
        );
        assert_eq!(
            calls[1].arguments,
            r#"{"tasks":[{"n":2,"task":"a"}],"wait":true}"#
        );
        assert!(text_tool_calls("Use [write_file(path=x)] carefully", &offered).is_none());
        assert!(text_tool_calls("See [rm(path=\"/\")]", &offered).is_none());
    }

    #[test]
    fn a_long_turn_keeps_its_requests_bounded() {
        let file = "x".repeat(40 * 1024);
        let step = |n: usize| {
            [
                json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("c{n}"),"type":"function","function":{"name":"write_file","arguments":json!({"path":format!("f{n}"),"content":file}).to_string()}}]}),
                json!({"role":"tool","tool_call_id":format!("c{n}"),"content":"y".repeat(40 * 1024)}),
            ]
        };
        let mut payload = json!({"messages":(0..1000).flat_map(step).collect::<Vec<_>>()});
        drop_old_steps(&mut payload);
        trim_tool_history(&mut payload);
        let messages = payload["messages"].as_array().unwrap();
        let bytes: usize = messages.iter().map(|m| m.to_string().len()).sum();
        assert!(bytes < 400 * 1024, "{bytes} bytes after trimming");
        let first = &messages[1]["tool_calls"][0]["function"]["arguments"];
        assert_eq!(first, ARGUMENTS_OMITTED);
        assert!(serde_json::from_str::<Value>(first.as_str().unwrap()).is_ok());
        let last = &messages[messages.len() - 2]["tool_calls"][0]["function"]["arguments"];
        assert!(
            last.as_str().unwrap().contains("f999"),
            "the newest call is kept"
        );
    }

    #[test]
    fn a_turn_without_end_drops_its_oldest_steps() {
        let step = |n: usize| {
            [
                json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("c{n}"),"type":"function","function":{"name":"run","arguments":"{}"}}]}),
                json!({"role":"tool","tool_call_id":format!("c{n}"),"content":"ok"}),
            ]
        };
        let head = [
            json!({"role":"system","content":"s"}),
            json!({"role":"user","content":"go"}),
        ];
        let mut payload = json!({"messages":head.iter().cloned().chain((0..KEPT_STEPS + 10).flat_map(step)).collect::<Vec<_>>()});
        drop_old_steps(&mut payload);
        for n in KEPT_STEPS + 10..KEPT_STEPS + 15 {
            payload["messages"].as_array_mut().unwrap().extend(step(n));
            drop_old_steps(&mut payload);
        }
        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3 + 2 * KEPT_STEPS);
        assert_eq!(messages[1]["content"], "go");
        assert!(messages[2]["content"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{STEPS_DROPPED} 15 of them")));
        assert_eq!(messages[3]["tool_calls"][0]["id"], "c15");
        assert_eq!(messages[4]["role"], "tool", "a step keeps its results");
    }

    #[test]
    fn a_file_being_typed_shows_its_last_lines() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "write_file".into(),
            arguments:
                r#"{"path":"js/rig.js","content":"const A = 1;\nconst \"B\" = 2;\n// θé and mo"#
                    .into(),
        };
        let draft = draft_of(&call).unwrap();
        assert_eq!(draft["path"], "js/rig.js");
        assert_eq!(draft["lines"], 3);
        assert_eq!(
            draft["tail"],
            "const A = 1;\nconst \"B\" = 2;\n// θé and mo"
        );
        let reading = ToolCall {
            name: "read_file".into(),
            ..call
        };
        assert!(draft_of(&reading).is_none());
        assert_eq!(
            partial_string(r#"{"path": "a b"}"#, "path").as_deref(),
            Some("a b")
        );
        assert_eq!(partial_string(r#"{"content":"x"}"#, "path"), None);
    }
}
