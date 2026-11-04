//! Apps: a project turned into an API. The project's owner lays out steps (engine.rs),
//! tries them on the draft, and publishes: the published version answers
//! `POST /api/apps/{id}/run` for anyone holding one of the app's keys, as the person who
//! made that key. The id is a fixed eight-character code, so apps never collide and a
//! rename never breaks a caller. The published version also answers on a chat page,
//! `/a/{id}/{slug}`, as the person chatting. For now only administrators make, run,
//! call and chat with apps.
mod code;
pub mod engine;

use crate::{
    auth::{Identity, Role},
    error::ApiError,
    AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use engine::Definition;
use rand::{rngs::OsRng, Rng, RngCore};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::{
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Letters for app codes: no 0/o or 1/l, so a code read aloud is typed right.
const CODE_LETTERS: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
const KEY_MARKER: &str = "oba_";
const MAX_KEYS: i64 = 10;
/// A run that takes longer is stopped and recorded as failed.
const RUN_TIMEOUT: Duration = Duration::from_secs(300);
/// Runs are kept this long, with their inputs and trace, to see what went wrong.
const KEEP_RUN_DAYS: i32 = 7;
/// Calls running at once on this replica, across every app.
static CALLS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(16));

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/projects/{id}/app", get(project_app).post(create))
        .route("/api/apps/{id}", put(save).delete(remove))
        .route("/api/apps/{id}/test", post(test))
        .route("/api/apps/{id}/publish", post(publish))
        .route("/api/apps/{id}/unpublish", post(unpublish))
        .route("/api/apps/{id}/runs", get(runs))
        .route("/api/apps/{id}/runs/{run}", get(run_detail))
        .route("/api/apps/{id}/keys", get(keys).post(create_key))
        .route("/api/apps/{id}/keys/{key}", delete(revoke_key))
        .route("/api/apps/{id}/run", post(call))
        .route("/api/apps/{id}/page", get(page))
        .route("/api/apps/{id}/chat", post(chat))
        .with_state(state)
}

#[derive(FromRow)]
struct App {
    id: String,
    project_id: Uuid,
    name: String,
    slug: String,
    description: String,
    draft: Value,
    live: Option<Value>,
    version: i32,
    published_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}
const APP_COLUMNS: &str =
    "id, project_id, name, slug, description, draft, live, version, published_at, updated_at";

impl App {
    fn view(&self) -> Value {
        json!({
            "id": self.id, "projectId": self.project_id, "name": self.name, "slug": self.slug,
            "description": self.description, "definition": self.draft,
            "live": self.live.is_some(), "version": self.version, "publishedAt": self.published_at,
            "changed": self.live.as_ref() != Some(&self.draft), "updatedAt": self.updated_at,
            "endpoint": format!("/api/apps/{}/run", self.id),
        })
    }
}

fn new_code() -> String {
    let mut rng = OsRng;
    (0..8)
        .map(|_| CODE_LETTERS[rng.gen_range(0..CODE_LETTERS.len())] as char)
        .collect()
}
/// A name as an address: lowercase words joined by hyphens.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 60 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "app".into()
    } else {
        out
    }
}
fn digest(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

async fn person(pool: &PgPool, id: Uuid) -> Result<Option<Identity>, sqlx::Error> {
    let row: Option<(Uuid, String, String, Role)> = sqlx::query_as(
        "SELECT id, username, display_name, role FROM users WHERE id = $1 AND enabled",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, username, display_name, role)| Identity {
        id,
        username,
        display_name,
        role,
        // A run is not tied to a browser sign-in.
        session: None,
    }))
}

async fn audit(
    pool: &PgPool,
    actor: Uuid,
    action: &str,
    app: &str,
    metadata: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,target_id,metadata) VALUES($1,$2,$3,'app',$4,$5)")
        .bind(Uuid::new_v4()).bind(actor).bind(action).bind(app).bind(metadata)
        .execute(pool).await?;
    Ok(())
}

/// An app its project's owner may change: an administrator who owns the project.
async fn editable(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    write: bool,
) -> Result<(Identity, App), ApiError> {
    let user = state.auth.require_admin(headers, write).await?;
    let app = sqlx::query_as::<_, App>(&format!("SELECT {APP_COLUMNS} FROM apps WHERE id = $1"))
        .bind(id)
        .fetch_optional(&state.store.pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    crate::projects::owned(&state.store.pool, user.id, app.project_id).await?;
    Ok((user, app))
}

async fn project_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require_admin(&headers, false).await?;
    crate::projects::owned(&state.store.pool, user.id, project).await?;
    let app = sqlx::query_as::<_, App>(&format!(
        "SELECT {APP_COLUMNS} FROM apps WHERE project_id = $1"
    ))
    .bind(project)
    .fetch_optional(&state.store.pool)
    .await?;
    Ok(Json(json!({"app": app.map(|a| a.view())})))
}

/// Whether a chat in this project can be offered an app: None when the project has one
/// already, otherwise whether the app can go in this project because the person owns it.
pub async fn room(pool: &PgPool, user: Uuid, project: Uuid) -> Option<bool> {
    sqlx::query_scalar::<_, Option<bool>>(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM apps WHERE project_id = $2) THEN NULL ELSE owner_id = $1 END
         FROM projects WHERE id = $2",
    )
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten()
}

/// What a new app may start with, such as the steps a chat suggested.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateInput {
    #[serde(default)]
    description: String,
    #[serde(default)]
    definition: Option<Definition>,
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = state.auth.require_admin(&headers, true).await?;
    crate::projects::owned(&state.store.pool, user.id, project).await?;
    let input: CreateInput = if body.is_empty() {
        CreateInput::default()
    } else {
        serde_json::from_slice::<Option<CreateInput>>(&body)
            .map_err(|e| ApiError::bad(format!("Invalid app: {e}")))?
            .unwrap_or_default()
    };
    let name: String = sqlx::query_scalar("SELECT name FROM projects WHERE id = $1")
        .bind(project)
        .fetch_one(&state.store.pool)
        .await?;
    let draft = match input.definition {
        Some(definition) => {
            definition.validate().map_err(ApiError::bad)?;
            serde_json::to_value(definition).map_err(|_| ApiError::internal())?
        }
        None => {
            json!({"inputs": [{"name": "question", "type": "text", "required": true, "description": ""}], "steps": []})
        }
    };
    let description: String = input.description.trim().chars().take(1000).collect();
    for _ in 0..5 {
        let created = sqlx::query_as::<_, App>(&format!(
            "INSERT INTO apps (id, project_id, owner_id, name, slug, description, draft) VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (id) DO NOTHING RETURNING {APP_COLUMNS}"
        ))
        .bind(new_code())
        .bind(project)
        .bind(user.id)
        .bind(&name)
        .bind(slug(&name))
        .bind(&description)
        .bind(&draft)
        .fetch_optional(&state.store.pool)
        .await;
        match created {
            Ok(Some(app)) => return Ok((StatusCode::CREATED, Json(json!({"app": app.view()})))),
            Ok(None) => continue,
            Err(sqlx::Error::Database(e)) if e.constraint() == Some("apps_project_id_key") => {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "This project already has an app".into(),
                ))
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::internal())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveInput {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: String,
    definition: Definition,
}
async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SaveInput>,
) -> Result<Json<Value>, ApiError> {
    let (_, app) = editable(&state, &headers, &id, true).await?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad("Give the app a name of up to 120 characters"));
    }
    if body.description.chars().count() > 1000 {
        return Err(ApiError::bad("Keep the description to 1,000 characters"));
    }
    let slug = slug(body.slug.as_deref().unwrap_or(name));
    body.definition.validate().map_err(ApiError::bad)?;
    let draft = serde_json::to_value(&body.definition).map_err(|_| ApiError::internal())?;
    let saved = sqlx::query_as::<_, App>(&format!(
        "UPDATE apps SET name = $2, slug = $3, description = $4, draft = $5, updated_at = NOW() WHERE id = $1 RETURNING {APP_COLUMNS}"
    ))
    .bind(&app.id)
    .bind(name)
    .bind(&slug)
    .bind(body.description.trim())
    .bind(&draft)
    .fetch_one(&state.store.pool)
    .await?;
    Ok(Json(json!({"app": saved.view()})))
}

async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (user, app) = editable(&state, &headers, &id, true).await?;
    sqlx::query("DELETE FROM apps WHERE id = $1")
        .bind(&app.id)
        .execute(&state.store.pool)
        .await?;
    audit(
        &state.store.pool,
        user.id,
        "app.deleted",
        &app.id,
        json!({"name": app.name}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (user, app) = editable(&state, &headers, &id, true).await?;
    let definition: Definition = serde_json::from_value(app.draft.clone())
        .map_err(|_| ApiError::bad("Save the steps again before publishing"))?;
    definition.validate().map_err(ApiError::bad)?;
    if definition.steps.is_empty() {
        return Err(ApiError::bad("Add a step before publishing"));
    }
    let saved = sqlx::query_as::<_, App>(&format!(
        "UPDATE apps SET live = draft, version = version + 1, published_at = NOW(), updated_at = NOW() WHERE id = $1 RETURNING {APP_COLUMNS}"
    ))
    .bind(&app.id)
    .fetch_one(&state.store.pool)
    .await?;
    audit(
        &state.store.pool,
        user.id,
        "app.published",
        &app.id,
        json!({"name": saved.name, "version": saved.version}),
    )
    .await?;
    Ok(Json(json!({"app": saved.view()})))
}

async fn unpublish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (user, app) = editable(&state, &headers, &id, true).await?;
    let saved = sqlx::query_as::<_, App>(&format!(
        "UPDATE apps SET live = NULL, updated_at = NOW() WHERE id = $1 RETURNING {APP_COLUMNS}"
    ))
    .bind(&app.id)
    .fetch_one(&state.store.pool)
    .await?;
    audit(
        &state.store.pool,
        user.id,
        "app.unpublished",
        &app.id,
        json!({"name": saved.name}),
    )
    .await?;
    Ok(Json(json!({"app": saved.view()})))
}

// ---- Runs ----

struct Outcome {
    id: Uuid,
    status: &'static str,
    output: Value,
    error: Option<String>,
    trace: Value,
    ms: u64,
}
impl Outcome {
    fn view(&self) -> Value {
        json!({"runId": self.id, "status": self.status, "output": self.output, "error": self.error, "trace": self.trace, "ms": self.ms})
    }
}

/// Runs a version of the app and records it. The run carries on to the end even if
/// the caller goes away.
async fn execute(
    state: &AppState,
    app: &App,
    definition: Definition,
    given: &Value,
    user: Identity,
    trigger: &'static str,
    key: Option<Uuid>,
) -> Result<Outcome, ApiError> {
    let inputs = engine::inputs(&definition, given)
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let pool = &state.store.pool;
    let version = if trigger == "test" { 0 } else { app.version };
    let run: Uuid = sqlx::query_scalar(
        "INSERT INTO app_runs (id, app_id, version, trigger, actor_id, key_id, inputs) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(&app.id)
    .bind(version)
    .bind(trigger)
    .bind(user.id)
    .bind(key)
    .bind(&inputs)
    .fetch_one(pool)
    .await?;
    let collections: Vec<Uuid> =
        sqlx::query_scalar("SELECT collection_id FROM project_collections WHERE project_id = $1")
            .bind(app.project_id)
            .fetch_all(pool)
            .await?;
    let task_state = state.clone();
    let started = Instant::now();
    let handle = tokio::spawn(async move {
        let run = engine::Run {
            state: &task_state,
            user,
            collections,
            trace: Mutex::new(vec![]),
        };
        let result = match tokio::time::timeout(RUN_TIMEOUT, run.go(&definition, inputs)).await {
            Ok(result) => result,
            Err(_) => Err(format!(
                "The run took longer than {} minutes",
                RUN_TIMEOUT.as_secs() / 60
            )),
        };
        let trace = run.trace.into_inner().unwrap_or_default();
        (result, trace)
    });
    let (result, trace) = handle
        .await
        .unwrap_or_else(|_| (Err("The run stopped unexpectedly".into()), vec![]));
    let ms = started.elapsed().as_millis() as u64;
    let (status, output, error) = match result {
        Ok(v) => ("completed", v, None),
        Err(e) => ("failed", Value::Null, Some(e)),
    };
    let trace = Value::Array(trace);
    sqlx::query("UPDATE app_runs SET status = $2, output = $3, error = $4, trace = $5, finished_at = NOW() WHERE id = $1")
        .bind(run)
        .bind(status)
        .bind(&output)
        .bind(&error)
        .bind(&trace)
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM app_runs WHERE app_id = $1 AND started_at < NOW() - make_interval(days => $2)",
    )
    .bind(&app.id)
    .bind(KEEP_RUN_DAYS)
    .execute(pool)
    .await?;
    Ok(Outcome {
        id: run,
        status,
        output,
        error,
        trace,
        ms,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestInput {
    #[serde(default)]
    inputs: Value,
}
async fn test(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<TestInput>,
) -> Result<Json<Value>, ApiError> {
    let (user, app) = editable(&state, &headers, &id, true).await?;
    let definition: Definition = serde_json::from_value(app.draft.clone())
        .map_err(|_| ApiError::bad("Save the steps again"))?;
    let _slot = CALLS.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many runs at once; try again shortly".into(),
        )
    })?;
    let outcome = execute(&state, &app, definition, &body.inputs, user, "test", None).await?;
    Ok(Json(outcome.view()))
}

/// The published app, for a holder of one of its keys.
async fn call(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let secret = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| t.starts_with(KEY_MARKER))
        .ok_or_else(ApiError::unauthorized)?;
    let pool = &state.store.pool;
    let key: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "UPDATE app_keys SET last_used_at = NOW() WHERE app_id = $1 AND token_hash = $2 AND revoked_at IS NULL RETURNING id, created_by",
    )
    .bind(&id)
    .bind(digest(secret))
    .fetch_optional(pool)
    .await?;
    let (key, owner) = key.ok_or_else(ApiError::unauthorized)?;
    let user = match owner {
        Some(owner) => person(pool, owner).await?,
        None => None,
    }
    .filter(|u| u.role == Role::Admin)
    .ok_or_else(|| {
        ApiError(
            StatusCode::FORBIDDEN,
            "The person who made this key can no longer run apps".into(),
        )
    })?;
    let app = sqlx::query_as::<_, App>(&format!("SELECT {APP_COLUMNS} FROM apps WHERE id = $1"))
        .bind(&id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let live = app
        .live
        .clone()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "This app isn't published".into()))?;
    let definition: Definition = serde_json::from_value(live).map_err(|_| ApiError::internal())?;
    let _slot = CALLS.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many runs at once; try again shortly".into(),
        )
    })?;
    let given = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let outcome = execute(&state, &app, definition, &given, user, "api", Some(key)).await?;
    let body = json!({"runId": outcome.id, "status": outcome.status, "output": outcome.output, "error": outcome.error});
    let status = if outcome.status == "completed" {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    Ok((status, Json(body)).into_response())
}

async fn runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (_, app) = editable(&state, &headers, &id, false).await?;
    let runs: Vec<Value> = sqlx::query_scalar(&format!(
        "SELECT json_build_object({RUN_FIELDS}) FROM app_runs WHERE app_id = $1 ORDER BY started_at DESC LIMIT 25"
    ))
    .bind(&app.id)
    .fetch_all(&state.store.pool)
    .await?;
    Ok(Json(json!({"runs": runs})))
}
/// A run as the page lists it. One still marked running long after its time ran out
/// died with its replica.
const RUN_FIELDS: &str = "'id', id, 'trigger', trigger, 'version', version, 'error', error, 'startedAt', started_at,
    'status', CASE WHEN status = 'running' AND started_at < NOW() - INTERVAL '6 minutes' THEN 'failed' ELSE status END,
    'ms', (EXTRACT(EPOCH FROM finished_at - started_at) * 1000)::BIGINT";

async fn run_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, run)): Path<(String, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let (_, app) = editable(&state, &headers, &id, false).await?;
    let run: Value = sqlx::query_scalar(&format!(
        "SELECT json_build_object({RUN_FIELDS}, 'inputs', inputs, 'output', output, 'trace', trace) FROM app_runs WHERE app_id = $1 AND id = $2"
    ))
    .bind(&app.id)
    .bind(run)
    .fetch_optional(&state.store.pool)
    .await?
    .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"run": run})))
}

// ---- Chat page ----

/// What a chat message fills: the first text input other than `history`, and
/// `history`, a JSON input that gets the earlier turns, when the app has one.
struct ChatFields<'a> {
    message: Option<&'a str>,
    history: bool,
    /// Other required inputs, which a chat can't fill.
    missing: Vec<&'a str>,
}
fn chat_fields(definition: &Definition) -> ChatFields<'_> {
    let history = |f: &engine::Field| f.name == "history" && f.kind == engine::Kind::Json;
    let message = definition
        .inputs
        .iter()
        .find(|f| f.kind == engine::Kind::Text && f.name != "history")
        .map(|f| f.name.as_str());
    ChatFields {
        message,
        history: definition.inputs.iter().any(history),
        missing: definition
            .inputs
            .iter()
            .filter(|f| f.required && Some(f.name.as_str()) != message && !history(f))
            .map(|f| f.name.as_str())
            .collect(),
    }
}

/// The published app for a signed-in person, as the chat page shows it.
async fn live_app(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    write: bool,
) -> Result<(Identity, App, Definition), ApiError> {
    let user = state.auth.require_admin(headers, write).await?;
    let app = sqlx::query_as::<_, App>(&format!("SELECT {APP_COLUMNS} FROM apps WHERE id = $1"))
        .bind(id)
        .fetch_optional(&state.store.pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let live = app
        .live
        .clone()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "This app isn't published".into()))?;
    let definition: Definition = serde_json::from_value(live).map_err(|_| ApiError::internal())?;
    Ok((user, app, definition))
}

async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (user, app, definition) = live_app(&state, &headers, &id, false).await?;
    let fields = chat_fields(&definition);
    let owner: bool = sqlx::query_scalar("SELECT owner_id = $2 FROM projects WHERE id = $1")
        .bind(app.project_id)
        .bind(user.id)
        .fetch_optional(&state.store.pool)
        .await?
        .flatten()
        .unwrap_or(false);
    Ok(Json(json!({"app": {
        "id": app.id, "name": app.name, "slug": app.slug, "description": app.description,
        "version": app.version, "publishedAt": app.published_at,
        "message": fields.message, "history": fields.history, "missing": fields.missing,
        "projectId": if owner { Some(app.project_id) } else { None },
    }})))
}

const MAX_MESSAGE_CHARS: usize = 20_000;
const MAX_TURNS: usize = 40;
const MAX_HISTORY_BYTES: usize = 200_000;

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    role: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatInput {
    message: String,
    #[serde(default)]
    history: Vec<Turn>,
}
/// One message on the chat page: a run of the published version as the person
/// chatting, with the earlier turns when the app takes them. The page keeps the
/// conversation; Chat keeps only the run.
async fn chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ChatInput>,
) -> Result<Json<Value>, ApiError> {
    let (user, app, definition) = live_app(&state, &headers, &id, true).await?;
    let fields = chat_fields(&definition);
    let message_field = fields
        .message
        .ok_or_else(|| ApiError::bad("This app has no text input for a chat message"))?;
    if !fields.missing.is_empty() {
        return Err(ApiError::bad(format!(
            "This app also needs {}, which a chat can't give it",
            fields.missing.join(", ")
        )));
    }
    let message = body.message.trim();
    if message.is_empty() || message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(ApiError::bad(format!(
            "Write a message of up to {MAX_MESSAGE_CHARS} characters"
        )));
    }
    let mut given = json!({ message_field: message });
    if fields.history {
        if body
            .history
            .iter()
            .any(|t| t.role != "user" && t.role != "assistant")
        {
            return Err(ApiError::bad(
                "Each earlier turn is from the user or the assistant",
            ));
        }
        // The most recent turns, as many as fit.
        let mut turns: Vec<&Turn> = vec![];
        let mut bytes = 0;
        for turn in body.history.iter().rev().take(MAX_TURNS) {
            bytes += turn.content.len();
            if bytes > MAX_HISTORY_BYTES {
                break;
            }
            turns.push(turn);
        }
        turns.reverse();
        given["history"] = serde_json::to_value(turns).map_err(|_| ApiError::internal())?;
    }
    let _slot = CALLS.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many runs at once; try again shortly".into(),
        )
    })?;
    let outcome = execute(&state, &app, definition, &given, user, "chat", None).await?;
    Ok(Json(
        json!({"runId": outcome.id, "status": outcome.status, "output": outcome.output, "error": outcome.error}),
    ))
}

// ---- Keys ----

async fn keys(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (_, app) = editable(&state, &headers, &id, false).await?;
    let keys: Vec<Value> = sqlx::query_scalar(
        "SELECT json_build_object('id', k.id, 'name', k.name, 'createdBy', u.display_name, 'createdAt', k.created_at, 'lastUsedAt', k.last_used_at)
         FROM app_keys k LEFT JOIN users u ON u.id = k.created_by
         WHERE k.app_id = $1 AND k.revoked_at IS NULL ORDER BY k.created_at DESC",
    )
    .bind(&app.id)
    .fetch_all(&state.store.pool)
    .await?;
    Ok(Json(json!({"keys": keys})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyInput {
    name: String,
}
async fn create_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<KeyInput>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (user, app) = editable(&state, &headers, &id, true).await?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad("Name the key in up to 80 characters"));
    }
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_keys WHERE app_id = $1 AND revoked_at IS NULL",
    )
    .bind(&app.id)
    .fetch_one(&state.store.pool)
    .await?;
    if active >= MAX_KEYS {
        return Err(ApiError::bad(format!(
            "An app has at most {MAX_KEYS} keys; revoke one first"
        )));
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let secret = format!("{KEY_MARKER}{}", URL_SAFE_NO_PAD.encode(bytes));
    let key: Uuid = sqlx::query_scalar(
        "INSERT INTO app_keys (id, app_id, name, token_hash, created_by) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(&app.id)
    .bind(name)
    .bind(digest(&secret))
    .bind(user.id)
    .fetch_one(&state.store.pool)
    .await?;
    audit(
        &state.store.pool,
        user.id,
        "app.key_created",
        &app.id,
        json!({"name": name}),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"key": {"id": key, "name": name}, "secret": secret})),
    ))
}

async fn revoke_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let (user, app) = editable(&state, &headers, &id, true).await?;
    let name: Option<String> = sqlx::query_scalar(
        "UPDATE app_keys SET revoked_at = NOW() WHERE app_id = $1 AND id = $2 AND revoked_at IS NULL RETURNING name",
    )
    .bind(&app.id)
    .bind(key)
    .fetch_optional(&state.store.pool)
    .await?;
    let name = name.ok_or_else(ApiError::not_found)?;
    audit(
        &state.store.pool,
        user.id,
        "app.key_revoked",
        &app.id,
        json!({"name": name}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_slugs_are_safe_in_an_address() {
        let code = new_code();
        assert_eq!(code.len(), 8);
        assert!(code.bytes().all(|b| CODE_LETTERS.contains(&b)));
        assert_eq!(slug("Assay Lab: results (v2)"), "assay-lab-results-v2");
        assert_eq!(slug("  ¿Qué?  "), "qu");
        assert_eq!(slug("!!!"), "app");
        assert!(slug(&"word ".repeat(40)).len() <= 60);
    }
}
