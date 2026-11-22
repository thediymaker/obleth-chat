//! A project's notebook: the project's documents with a tick each, kept per person, a
//! chat whose knowledge search is limited to the ticked documents, and, when an
//! administrator names a Notebook model, a short guide to each new document and a
//! meter of how much of that model's window the ticked documents take.
//!
//! Every document goes through the knowledge visibility rules: a notebook lists, ticks
//! and searches only documents in the project's collections that the person may open.

pub mod guides;

use crate::audit::{self, Action, Target};
use crate::knowledge::VISIBLE as COLLECTION_VISIBLE;
use crate::projects::VISIBLE as PROJECT_VISIBLE;
use crate::store::ReasoningEffort;
use crate::{auth::Identity, error::ApiError, gateway::Gateway, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// A rough characters-to-tokens estimate: English prose runs about four characters to
/// a token in common tokenizers; code, tables and other languages run fewer, so the
/// meter is a guide, not a promise.
pub const CHARS_PER_TOKEN: u64 = 4;
/// Room kept in the Notebook model's window for its instructions and its answer when
/// it reads whole sources: what is left is what "the notebook can read".
pub const ANSWER_ROOM: u64 = 12_000;
/// Documents a notebook lists per collection, newest first.
const LISTED: i64 = 300;
/// Documents one change of ticks may name, and the most a person can tick in a project.
const MAX_TICKED: i64 = 1000;
/// Names of ticked sources a chat's instructions list.
const NAMED: usize = 20;

/// The Notebook model, when an administrator named one.
#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Settings {
    pub model: Option<String>,
    pub model_set_at: Option<DateTime<Utc>>,
}

pub async fn settings(pool: &PgPool) -> Result<Settings, sqlx::Error> {
    sqlx::query_as("SELECT model, model_set_at FROM notebook_settings WHERE id")
        .fetch_one(pool)
        .await
}

/// Names the Notebook model, or clears it, and records who did. Naming a different
/// model restarts the clock guides count from.
pub async fn save_settings(
    pool: &PgPool,
    admin: Uuid,
    model: Option<String>,
) -> Result<Settings, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let saved: Settings = sqlx::query_as(
        "UPDATE notebook_settings SET model = $1,
             model_set_at = CASE WHEN $1 IS NULL THEN NULL WHEN model IS DISTINCT FROM $1 THEN NOW() ELSE model_set_at END,
             updated_at = NOW()
         WHERE id RETURNING model, model_set_at",
    )
    .bind(model)
    .fetch_one(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(admin),
        Action::NOTEBOOK_UPDATE,
        Target::site(),
        json!({"model": saved.model}),
    )
    .await?;
    tx.commit().await?;
    Ok(saved)
}

/// What limits a notebook chat's turn: the project's collections the person may open,
/// the documents they ticked among them, and whether answers keep to the passages.
#[derive(Clone, Debug, PartialEq)]
pub struct Restriction {
    pub project: Uuid,
    pub collections: Vec<Uuid>,
    pub documents: Vec<Uuid>,
    /// The ticked documents' names, the first NAMED of them.
    pub names: Vec<String>,
    pub only_sources: bool,
}

/// The restriction of a notebook chat, None for any other chat. A chat moved out of
/// its notebook's project, or in a project the person may no longer use, is not a
/// notebook chat any more.
pub async fn restriction(
    pool: &PgPool,
    user: Uuid,
    conversation: Uuid,
) -> Result<Option<Restriction>, sqlx::Error> {
    let row: Option<(Uuid, bool)> = sqlx::query_as(&format!(
        "SELECT nc.project_id, nc.only_sources FROM notebook_chats nc
         JOIN conversations v ON v.id = nc.conversation_id AND v.project_id = nc.project_id
         JOIN projects p ON p.id = nc.project_id
         WHERE nc.conversation_id = $2 AND v.owner_id = $1 AND {PROJECT_VISIBLE}"
    ))
    .bind(user)
    .bind(conversation)
    .fetch_optional(pool)
    .await?;
    let Some((project, only_sources)) = row else {
        return Ok(None);
    };
    let collections = project_collections(pool, user, project).await?;
    let ticked: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT d.id, d.name FROM notebook_sources s JOIN knowledge_documents d ON d.id = s.document_id
         WHERE s.owner_id = $1 AND s.project_id = $2 AND d.collection_id = ANY($3)
         ORDER BY d.name, d.id",
    )
    .bind(user)
    .bind(project)
    .bind(&collections)
    .fetch_all(pool)
    .await?;
    Ok(Some(Restriction {
        project,
        collections,
        names: ticked.iter().take(NAMED).map(|t| t.1.clone()).collect(),
        documents: ticked.into_iter().map(|t| t.0).collect(),
        only_sources,
    }))
}

/// The project's collections person `user` may open, by name.
async fn project_collections(
    pool: &PgPool,
    user: Uuid,
    project: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT c.id FROM project_collections pc JOIN knowledge_collections c ON c.id = pc.collection_id
         WHERE pc.project_id = $2 AND {COLLECTION_VISIBLE} ORDER BY c.name, c.id"
    ))
    .bind(user)
    .bind(project)
    .fetch_all(pool)
    .await
}

/// The project if the person may use it: its name, and whether they own it.
async fn usable(pool: &PgPool, user: Uuid, project: Uuid) -> Result<(String, bool), ApiError> {
    sqlx::query_as(&format!(
        "SELECT p.name, p.owner_id = $1 FROM projects p WHERE p.id = $2 AND {PROJECT_VISIBLE}"
    ))
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)
}

/// The documents among `ids` that the person may tick in the project: in one of its
/// collections, which they may open.
async fn tickable(
    pool: &PgPool,
    user: Uuid,
    project: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, sqlx::Error> {
    let collections = project_collections(pool, user, project).await?;
    sqlx::query_scalar(
        "SELECT id FROM knowledge_documents WHERE id = ANY($1) AND collection_id = ANY($2)",
    )
    .bind(ids)
    .bind(&collections)
    .fetch_all(pool)
    .await
}

/// Ticks or unticks documents for the person. Ones they may not see are left out.
pub async fn tick(
    pool: &PgPool,
    user: Uuid,
    project: Uuid,
    ids: &[Uuid],
    ticked: bool,
) -> Result<(), ApiError> {
    if ids.len() as i64 > MAX_TICKED {
        return Err(ApiError::bad(format!(
            "Tick up to {MAX_TICKED} sources at a time"
        )));
    }
    if !ticked {
        sqlx::query(
            "DELETE FROM notebook_sources WHERE owner_id = $1 AND project_id = $2 AND document_id = ANY($3)",
        )
        .bind(user)
        .bind(project)
        .bind(ids)
        .execute(pool)
        .await?;
        return Ok(());
    }
    let ids = tickable(pool, user, project, ids).await?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO notebook_sources (owner_id, project_id, document_id) SELECT $1, $2, UNNEST($3::uuid[]) ON CONFLICT DO NOTHING",
    )
    .bind(user)
    .bind(project)
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notebook_sources WHERE owner_id = $1 AND project_id = $2",
    )
    .bind(user)
    .bind(project)
    .fetch_one(&mut *tx)
    .await?;
    if count > MAX_TICKED {
        return Err(ApiError::bad(format!(
            "A notebook can have up to {MAX_TICKED} ticked sources"
        )));
    }
    tx.commit().await?;
    Ok(())
}

/// The person's notebook in the project, made on first use with every document then
/// in the project ticked. Returns its chat, if it still has one.
async fn open(pool: &PgPool, user: Uuid, project: Uuid) -> Result<Option<Uuid>, ApiError> {
    let made: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO project_notebooks (id, owner_id, project_id) VALUES ($1, $2, $3)
         ON CONFLICT (owner_id, project_id) DO NOTHING RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?;
    if made.is_some() {
        let collections = project_collections(pool, user, project).await?;
        sqlx::query(
            "INSERT INTO notebook_sources (owner_id, project_id, document_id)
             SELECT $1, $2, d.id FROM knowledge_documents d WHERE d.collection_id = ANY($3)
             ORDER BY d.created_at DESC LIMIT $4 ON CONFLICT DO NOTHING",
        )
        .bind(user)
        .bind(project)
        .bind(&collections)
        .bind(MAX_TICKED)
        .execute(pool)
        .await?;
    }
    // Only a chat that is still theirs and still in the project.
    Ok(sqlx::query_scalar(
        "SELECT n.conversation_id FROM project_notebooks n
         JOIN conversations v ON v.id = n.conversation_id AND v.owner_id = n.owner_id AND v.project_id = n.project_id AND NOT v.archived
         WHERE n.owner_id = $1 AND n.project_id = $2",
    )
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?)
}

/// How much of the Notebook model's window the ticked sources take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meter {
    /// The ticked, indexed sources' stored text, in characters.
    pub chars: u64,
    /// That as tokens, by CHARS_PER_TOKEN.
    pub tokens: u64,
    /// What the notebook can read: the model's window less ANSWER_ROOM.
    pub capacity: u64,
}

impl Meter {
    pub fn new(chars: u64, window: u64) -> Self {
        Self {
            chars,
            tokens: chars.div_ceil(CHARS_PER_TOKEN),
            capacity: window.saturating_sub(ANSWER_ROOM),
        }
    }
    /// Whole percent of the capacity, rounded up so a sliver past full shows as 101.
    pub fn percent(&self) -> u64 {
        if self.capacity == 0 {
            return if self.tokens == 0 { 0 } else { 100 };
        }
        (self.tokens * 100).div_ceil(self.capacity)
    }
    pub fn fits(&self) -> bool {
        self.capacity > 0 && self.tokens <= self.capacity
    }
    pub fn json(&self) -> Value {
        json!({"chars":self.chars,"tokens":self.tokens,"capacity":self.capacity,"percent":self.percent(),"fits":self.fits()})
    }
}

/// The stored text of the person's ticked, indexed sources in the project. Documents
/// indexed before sizes were kept are measured once now.
pub async fn ticked_chars(pool: &PgPool, user: Uuid, project: Uuid) -> Result<u64, sqlx::Error> {
    let collections = project_collections(pool, user, project).await?;
    sqlx::query(
        "UPDATE knowledge_documents d SET text_chars = (
             SELECT COALESCE(SUM(LEAST(CHAR_LENGTH(p.content), 7900)), 0) FROM knowledge_passages p WHERE p.document_id = d.id)
         WHERE d.status = 'ready' AND d.text_chars IS NULL AND d.collection_id = ANY($3)
           AND d.id IN (SELECT document_id FROM notebook_sources WHERE owner_id = $1 AND project_id = $2)",
    )
    .bind(user)
    .bind(project)
    .bind(&collections)
    .execute(pool)
    .await?;
    let chars: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(d.text_chars), 0)::bigint FROM notebook_sources s JOIN knowledge_documents d ON d.id = s.document_id
         WHERE s.owner_id = $1 AND s.project_id = $2 AND d.status = 'ready' AND d.collection_id = ANY($3)",
    )
    .bind(user)
    .bind(project)
    .bind(&collections)
    .fetch_one(pool)
    .await?;
    Ok(chars.max(0) as u64)
}

/// The Notebook model as the person may use it: its id and the window the gateway
/// reports. None when none is named, the gateway does not list it for them, or it
/// reports no window.
pub async fn model_for(
    pool: &PgPool,
    gateway: Option<&Gateway>,
    user: &Identity,
) -> Result<ModelState, ApiError> {
    let Some(model) = settings(pool).await?.model else {
        return Ok(ModelState::Unset);
    };
    let Some(gateway) = gateway else {
        return Ok(ModelState::Unavailable(model));
    };
    let catalog = match crate::models::effective_catalog(pool, gateway, user, false).await {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!("notebook model lookup failed: {}", error.1);
            return Ok(ModelState::Unavailable(model));
        }
    };
    Ok(
        match catalog.iter().find(|m| m.id == model && m.available) {
            Some(found) => ModelState::Ready {
                window: found.metadata.context_window,
                name: found.name.clone(),
                model,
            },
            None => ModelState::Unavailable(model),
        },
    )
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModelState {
    Unset,
    /// Named, but this person cannot use it now.
    Unavailable(String),
    Ready {
        model: String,
        name: String,
        window: Option<u64>,
    },
}

impl ModelState {
    fn json(&self) -> Value {
        match self {
            ModelState::Unset => json!({"set":false,"available":false}),
            ModelState::Unavailable(_) => json!({"set":true,"available":false}),
            ModelState::Ready { name, window, .. } => {
                json!({"set":true,"available":true,"name":name,"contextWindow":window})
            }
        }
    }
    pub fn window(&self) -> Option<u64> {
        match self {
            ModelState::Ready { window, .. } => *window,
            _ => None,
        }
    }
}

// ---- HTTP ----

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/projects/{id}/notebook", get(read))
        .route("/api/projects/{id}/notebook/sources", put(set_sources))
        .route("/api/projects/{id}/notebook/chat", post(new_chat))
        .route(
            "/api/projects/{id}/notebook/collection",
            post(make_collection),
        )
        .route("/api/conversations/{id}/notebook", put(set_chat))
        .route("/api/admin/notebook", get(admin_read).put(admin_save))
        .with_state(state)
}

fn knowledge_on(s: &AppState) -> Result<(), ApiError> {
    if s.knowledge.is_none() {
        return Err(ApiError::feature_off("Knowledge is not configured"));
    }
    Ok(())
}

/// The notebook as the person sees it: sources by collection with their ticks and
/// guides, the chat, the Notebook model and the meter.
pub async fn view(s: &AppState, user: &Identity, project: Uuid) -> Result<Value, ApiError> {
    let pool = &s.store.pool;
    let (_, mine) = usable(pool, user.id, project).await?;
    let chat = open(pool, user.id, project).await?;
    let only_sources: bool = match chat {
        Some(chat) => {
            sqlx::query_scalar("SELECT only_sources FROM notebook_chats WHERE conversation_id = $1")
                .bind(chat)
                .fetch_optional(pool)
                .await?
                .unwrap_or(true)
        }
        None => true,
    };
    let collections: Vec<(Uuid, String, bool)> = sqlx::query_as(&format!(
        "SELECT c.id, c.name, c.owner_id = $1 FROM project_collections pc JOIN knowledge_collections c ON c.id = pc.collection_id
         WHERE pc.project_id = $2 AND {COLLECTION_VISIBLE} ORDER BY c.name, c.id"
    ))
    .bind(user.id)
    .bind(project)
    .fetch_all(pool)
    .await?;
    let ids: Vec<Uuid> = collections.iter().map(|c| c.0).collect();
    let rows = sqlx::query(
        "SELECT * FROM (
            SELECT d.id, d.collection_id, d.name, d.mime_type, d.size, d.status, d.stage, d.progress_done, d.progress_total,
                   d.pages, d.passages, d.error, d.created_at,
                   s.document_id IS NOT NULL AS ticked, g.status AS guide_status, g.summary, g.questions,
                   ROW_NUMBER() OVER (PARTITION BY d.collection_id ORDER BY d.created_at DESC, d.id) AS n
            FROM knowledge_documents d
            LEFT JOIN notebook_sources s ON s.document_id = d.id AND s.owner_id = $1 AND s.project_id = $2
            LEFT JOIN notebook_guides g ON g.document_id = d.id
            WHERE d.collection_id = ANY($3)) listed
         WHERE n <= $4 ORDER BY created_at DESC, id",
    )
    .bind(user.id)
    .bind(project)
    .bind(&ids)
    .bind(LISTED)
    .fetch_all(pool)
    .await?;
    let counts: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT collection_id, COUNT(*)::bigint FROM knowledge_documents WHERE collection_id = ANY($1) GROUP BY collection_id",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let ticked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notebook_sources s JOIN knowledge_documents d ON d.id = s.document_id
         WHERE s.owner_id = $1 AND s.project_id = $2 AND d.collection_id = ANY($3)",
    )
    .bind(user.id)
    .bind(project)
    .bind(&ids)
    .fetch_one(pool)
    .await?;
    let groups: Vec<Value> = collections
        .iter()
        .map(|(id, name, own)| {
            let documents: Vec<Value> = rows
                .iter()
                .filter(|r| r.get::<Uuid, _>("collection_id") == *id)
                .map(document_json)
                .collect();
            let total = counts.iter().find(|c| c.0 == *id).map_or(0, |c| c.1);
            json!({"id":id,"name":name,"mine":own,"documents":documents,"total":total})
        })
        .collect();
    let total: i64 = counts.iter().map(|c| c.1).sum();
    let model = model_for(pool, s.gateway.as_ref(), user).await?;
    let meter = match model.window() {
        Some(window) => {
            Some(Meter::new(ticked_chars(pool, user.id, project).await?, window).json())
        }
        None => None,
    };
    // Where Add puts files: a collection in the project the person owns; the owner of
    // the project can have one made when there is none.
    let add_to: Vec<Uuid> = collections.iter().filter(|c| c.2).map(|c| c.0).collect();
    Ok(json!({
        "chatId": chat,
        "onlySources": only_sources,
        "collections": groups,
        "ticked": ticked,
        "total": total,
        "model": model.json(),
        "meter": meter,
        "addTo": add_to,
        "canMakeCollection": mine && add_to.is_empty(),
    }))
}

fn document_json(r: &sqlx::postgres::PgRow) -> Value {
    let guide = r
        .get::<Option<String>, _>("guide_status")
        .map(|status| match status.as_str() {
            "ready" => json!({"status":status,"summary":r.get::<String,_>("summary"),"questions":r.get::<Vec<String>,_>("questions")}),
            _ => json!({"status":status}),
        });
    json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"mimeType":r.get::<String,_>("mime_type"),"size":r.get::<i64,_>("size"),
        "status":r.get::<String,_>("status"),"stage":r.get::<String,_>("stage"),"done":r.get::<i32,_>("progress_done"),"total":r.get::<i32,_>("progress_total"),
        "pages":r.get::<Option<i32>,_>("pages"),"passages":r.get::<i32,_>("passages"),"error":r.get::<Option<String>,_>("error"),
        "createdAt":r.get::<DateTime<Utc>,_>("created_at"),"ticked":r.get::<bool,_>("ticked"),"guide":guide})
}

async fn read(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    knowledge_on(&s)?;
    Ok(Json(view(&s, &user, id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct Sources {
    document_ids: Vec<Uuid>,
    ticked: bool,
}

async fn set_sources(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Sources>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    knowledge_on(&s)?;
    usable(&s.store.pool, user.id, id).await?;
    open(&s.store.pool, user.id, id).await?;
    tick(&s.store.pool, user.id, id, &body.document_ids, body.ticked).await?;
    Ok(Json(view(&s, &user, id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct NewChat {
    model: String,
    #[serde(default)]
    reasoning_effort: ReasoningEffort,
}

/// Starts a chat in the notebook, which it then shows. It keeps the previous chat's
/// "Only from the sources" choice.
async fn new_chat(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<NewChat>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    knowledge_on(&s)?;
    let pool = &s.store.pool;
    let (name, _) = usable(pool, user.id, id).await?;
    let previous = open(pool, user.id, id).await?;
    let only_sources: bool = match previous {
        Some(chat) => {
            sqlx::query_scalar("SELECT only_sources FROM notebook_chats WHERE conversation_id = $1")
                .bind(chat)
                .fetch_optional(pool)
                .await?
                .unwrap_or(true)
        }
        None => true,
    };
    let title: String = format!("{name} notebook").chars().take(120).collect();
    let chat = s
        .store
        .create_chat(user.id, &title, &body.model, body.reasoning_effort, false)
        .await?;
    let chat = chat.id;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE conversations SET project_id = $3 WHERE id = $1 AND owner_id = $2")
        .bind(chat)
        .bind(user.id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO notebook_chats (conversation_id, project_id, only_sources) VALUES ($1, $2, $3)",
    )
    .bind(chat)
    .bind(id)
    .bind(only_sources)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE project_notebooks SET conversation_id = $3, updated_at = NOW() WHERE owner_id = $1 AND project_id = $2",
    )
    .bind(user.id)
    .bind(id)
    .bind(chat)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"chatId":chat,"onlySources":only_sources})),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct ChatOptions {
    only_sources: bool,
}

/// Turns "Only from the sources" on or off for a notebook chat, from its next message.
async fn set_chat(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ChatOptions>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let done = sqlx::query(
        "UPDATE notebook_chats SET only_sources = $3 WHERE conversation_id = $1
           AND conversation_id IN (SELECT id FROM conversations WHERE id = $1 AND owner_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .bind(body.only_sources)
    .execute(&s.store.pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(Json(json!({"onlySources":body.only_sources})))
}

/// Makes a collection for the project's sources and attaches it, for the project's
/// owner when none of the project's collections is theirs. Returns its id.
pub async fn make_project_collection(
    pool: &PgPool,
    user: Uuid,
    project: Uuid,
) -> Result<Uuid, ApiError> {
    let (name, mine) = usable(pool, user, project).await?;
    if !mine {
        return Err(ApiError::denied(
            "Only the project's owner can add a collection to it",
        ));
    }
    let mut tx = pool.begin().await?;
    crate::store::lock_owner(&mut tx, user, crate::store::Require::Any).await?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT c.id FROM project_collections pc JOIN knowledge_collections c ON c.id = pc.collection_id
         WHERE pc.project_id = $1 AND c.owner_id = $2 ORDER BY c.created_at LIMIT 1",
    )
    .bind(project)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing) = existing {
        return Ok(existing);
    }
    let (owned, attached): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM knowledge_collections WHERE owner_id = $1 AND attachment_id IS NULL),
                (SELECT COUNT(*) FROM project_collections WHERE project_id = $2)",
    )
    .bind(user)
    .bind(project)
    .fetch_one(&mut *tx)
    .await?;
    if owned >= 100 {
        return Err(ApiError::bad(
            "You have 100 collections; delete one to make another",
        ));
    }
    if attached >= 8 {
        return Err(ApiError::bad("A project can search up to 8 collections"));
    }
    let id = Uuid::new_v4();
    let name: String = name.chars().take(120).collect();
    sqlx::query("INSERT INTO knowledge_collections (id, owner_id, name, description) VALUES ($1, $2, $3, 'Sources added in the project’s notebook')")
        .bind(id)
        .bind(user)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO project_collections (project_id, collection_id) VALUES ($1, $2)")
        .bind(project)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE projects SET updated_at = NOW() WHERE id = $1")
        .bind(project)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(id)
}

async fn make_collection(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    knowledge_on(&s)?;
    let collection = make_project_collection(&s.store.pool, user.id, id).await?;
    Ok(Json(json!({"collectionId":collection})))
}

fn admin_json(settings: &Settings, s: &AppState) -> Value {
    json!({"model":settings.model,"gateway":s.gateway.is_some(),"knowledge":s.knowledge.is_some()})
}

async fn admin_read(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    Ok(Json(admin_json(&settings(&s.store.pool).await?, &s)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminPatch {
    model: Option<String>,
}

async fn admin_save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AdminPatch>,
) -> Result<Json<Value>, ApiError> {
    let admin = s.auth.require_admin(&headers, true).await?;
    let model = crate::gateway::model_name(body.model)?;
    if model.is_some() && s.gateway.is_none() {
        return Err(ApiError::bad(
            "The notebook asks models through the gateway; connect one first",
        ));
    }
    let saved = save_settings(&s.store.pool, admin.id, model).await?;
    Ok(Json(admin_json(&saved, &s)))
}

/// The worker: guides for new documents in projects' collections, on one timer.
pub async fn worker(s: AppState, shutdown: tokio_util::sync::CancellationToken) {
    let pass = || async {
        if let Err(error) = guides::pass(&s.store.pool, s.gateway.as_ref()).await {
            tracing::warn!("notebook guides: {error}");
        }
    };
    crate::worker::every(
        crate::worker::NOTEBOOK,
        crate::worker::Start::AfterPeriod,
        shutdown,
        pass,
    )
    .await
}

#[cfg(test)]
pub(crate) mod tests;
