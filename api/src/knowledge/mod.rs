//! Knowledge: collections of documents the model can search and cite.
//!
//! A file is stored in object storage, then a background worker reads its text,
//! splits it into passages, embeds them through the gateway as the file's owner and
//! indexes them in Postgres for vector and full-text search. Every read checks that
//! the person may open the collection: their own, a group they belong to, or one an
//! administrator shared with everyone.

pub mod chat;
pub mod embedding;
pub mod extract;
pub mod saved;
pub mod storage;

use crate::{
    auth::{Identity, Role},
    error::ApiError,
    gateway::Gateway,
    AppState,
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::{env, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use storage::ObjectStore;

/// Embedding width stored for every passage. Models that return more dimensions are
/// cut to this many and renormalised (Matryoshka-trained models such as Qwen3-Embedding
/// keep their quality when shortened); shorter vectors are padded with zeros, which
/// leaves their cosine distances unchanged.
pub const DIMENSIONS: usize = 1024;
pub const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_DOCUMENTS: i64 = 2000;
const EMBED_BATCH: usize = 16;
pub use embedding::Prompts;

/// Who may see a collection, as SQL over `c` with the person's id in `$1`.
/// Collections person $1 may read: their own, shared with everyone or with a group they
/// are in, or owned by the owner of a project shared with them (sharing a project shares
/// the owner's own collections in it, never ones someone else shared with the owner).
pub(crate) const VISIBLE: &str = "(c.owner_id = $1 OR c.scope = 'everyone' OR (c.scope = 'group' AND c.group_id IN (SELECT group_id FROM user_group_members WHERE user_id = $1))
    OR c.id IN (SELECT pc.collection_id FROM project_collections pc JOIN projects p ON p.id = pc.project_id
                WHERE p.scope = 'group' AND p.owner_id = c.owner_id AND p.group_id IN (SELECT group_id FROM user_group_members WHERE user_id = $1)))";

#[derive(Clone)]
pub struct Knowledge {
    pool: PgPool,
    store: ObjectStore,
    tools: extract::Tools,
    model: String,
    /// The environment's prefixes for its model; unset ones follow the model's own.
    env_prompts: (Option<String>, Option<String>),
    user_bytes: i64,
}

impl Knowledge {
    /// Enabled when object storage is configured.
    pub fn from_env(pool: PgPool, store: Option<ObjectStore>) -> anyhow::Result<Option<Self>> {
        let Some(store) = store else {
            return Ok(None);
        };
        let var = |key: &str| env::var(key).ok().filter(|v| !v.trim().is_empty());
        let user_mb: i64 = var("OBLETH_CHAT_KNOWLEDGE_USER_MB")
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(2048);
        let mut knowledge = Self::new(
            pool,
            store,
            extract::Tools::from_env(),
            var("OBLETH_CHAT_EMBEDDING_MODEL").unwrap_or_else(|| "qwen3-embedding-8b".into()),
            user_mb * 1024 * 1024,
        );
        // Set but empty means no prefix.
        knowledge.env_prompts = (
            env::var("OBLETH_CHAT_EMBEDDING_QUERY_PREFIX").ok(),
            env::var("OBLETH_CHAT_EMBEDDING_DOCUMENT_PREFIX").ok(),
        );
        Ok(Some(knowledge))
    }

    pub fn new(
        pool: PgPool,
        store: ObjectStore,
        tools: extract::Tools,
        model: String,
        user_bytes: i64,
    ) -> Self {
        Self {
            pool,
            store,
            tools,
            env_prompts: (None, None),
            model,
            user_bytes,
        }
    }

    /// Prefixes for the default model, as the environment variables give them.
    pub fn with_prompts(mut self, prompts: Prompts) -> Self {
        self.env_prompts = (Some(prompts.query), Some(prompts.document));
        self
    }

    /// Processes queued documents and removes deleted files from storage until shutdown.
    pub async fn worker(self, gateway: Gateway, shutdown: CancellationToken) {
        loop {
            self.empty_trash().await;
            if let Err(error) = self.reconcile().await {
                tracing::warn!("could not check the embedding settings: {error:#}");
            }
            let embedded = self.embed_saved(&gateway).await;
            let worked = embedded
                || match self.claim().await {
                    Ok(Some(job)) => {
                        let id = job.id;
                        if let Err(error) = self.process(&gateway, job).await {
                            tracing::warn!(document=%id, "could not index a document: {error:#}");
                            let message = format!("{error:#}");
                            let _ = sqlx::query(
                            "UPDATE knowledge_documents SET status='failed', stage='', error=$2, updated_at=NOW() WHERE id=$1",
                        )
                        .bind(id)
                        .bind(message.chars().take(900).collect::<String>())
                        .execute(&self.pool)
                        .await;
                        }
                        true
                    }
                    Ok(None) => false,
                    Err(error) => {
                        tracing::warn!("knowledge queue unavailable: {error:#}");
                        false
                    }
                };
            if !worked {
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            } else if shutdown.is_cancelled() {
                return;
            }
        }
    }

    async fn empty_trash(&self) {
        crate::files::empty_trash(&self.pool, &self.store).await;
    }

    /// Takes the oldest queued document, or one whose worker stopped mid-way.
    async fn claim(&self) -> anyhow::Result<Option<Job>> {
        let row = sqlx::query(
            "UPDATE knowledge_documents SET status='processing', stage='reading', error=NULL, claimed_at=NOW(), attempts=attempts+1, updated_at=NOW()
             WHERE id = (SELECT id FROM knowledge_documents
                         WHERE status='queued' OR (status='processing' AND claimed_at < NOW() - INTERVAL '10 minutes')
                         ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1)
             RETURNING id, owner_id, collection_id, name, mime_type, object_key, size, attempts, claimed_at",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| Job {
            id: r.get("id"),
            owner: r.get("owner_id"),
            collection: r.get("collection_id"),
            name: r.get("name"),
            mime: r.get("mime_type"),
            key: r.get("object_key"),
            size: r.get("size"),
            attempts: r.get("attempts"),
            claimed: r.get("claimed_at"),
        }))
    }

    async fn process(&self, gateway: &Gateway, mut job: Job) -> anyhow::Result<()> {
        if job.attempts > 3 {
            anyhow::bail!("Processing stopped three times; delete the file and upload it again");
        }
        let owner = identity(&self.pool, job.owner).await?;
        let embedding = self.embedding().await?;
        let kind = extract::kind(&job.name, &job.mime)
            .ok_or_else(|| anyhow::anyhow!("This file type is not supported"))?;
        let bytes = self.store.get(&job.key, job.size as usize + 1024).await?;
        let pages = match extract::extract(&self.tools, kind, &bytes).await? {
            extract::Extracted::NeedsOcr { pages } => {
                sqlx::query("UPDATE knowledge_documents SET status='needs_ocr', stage='', pages=$2, updated_at=NOW() WHERE id=$1")
                    .bind(job.id)
                    .bind(pages)
                    .execute(&self.pool)
                    .await?;
                return Ok(());
            }
            extract::Extracted::Pages(pages) => pages,
        };
        let page_count = pages.iter().filter_map(|p| p.number).max();
        let passages = extract::split(&pages);
        if passages.is_empty() {
            anyhow::bail!("No text was found in this file");
        }
        sqlx::query("DELETE FROM knowledge_passages WHERE document_id=$1")
            .bind(job.id)
            .execute(&self.pool)
            .await?;
        let started = sqlx::query("UPDATE knowledge_documents SET stage='embedding', progress_done=0, progress_total=$2, pages=$3, updated_at=NOW() WHERE id=$1 AND status='processing' AND claimed_at=$4")
            .bind(job.id)
            .bind(passages.len() as i32)
            .bind(page_count)
            .bind(job.claimed)
            .execute(&self.pool)
            .await?;
        if started.rows_affected() == 0 {
            return Ok(());
        }
        for (batch, chunk) in passages.chunks(EMBED_BATCH).enumerate() {
            let input: Vec<String> = chunk.iter().map(|p| embedding.document(&p.text)).collect();
            let vectors = gateway
                .embeddings(&owner, &embedding.model, &input)
                .await
                .map_err(
                    |e| match e.1.starts_with(crate::auth::grants::SIGN_IN_AGAIN) {
                        // Retrying can't help until the owner signs in, so say that.
                        true => anyhow::anyhow!(
                            "{}: the file's owner must sign in before it can be indexed",
                            crate::auth::grants::SIGN_IN_AGAIN
                        ),
                        false => anyhow::anyhow!("The embedding model is unavailable ({})", e.1),
                    },
                )?;
            let start = batch * EMBED_BATCH;
            let ordinals: Vec<i32> = (start..start + chunk.len()).map(|i| i as i32).collect();
            let pages: Vec<Option<i32>> = chunk.iter().map(|p| p.page).collect();
            let texts: Vec<String> = chunk
                .iter()
                .map(|p| p.text.chars().take(7900).collect())
                .collect();
            let embeddings: Vec<String> = vectors.into_iter().map(vector_literal).collect();
            let inserted = sqlx::query(
                "INSERT INTO knowledge_passages (document_id, collection_id, ordinal, page, content, embedding)
                 SELECT $1, $2, o, p, t, e::halfvec FROM UNNEST($3::int[], $4::int[], $5::text[], $6::text[]) AS u(o, p, t, e)
                 WHERE EXISTS (SELECT 1 FROM knowledge_documents WHERE id=$1 AND status='processing' AND claimed_at=$7)",
            )
            .bind(job.id)
            .bind(job.collection)
            .bind(&ordinals)
            .bind(&pages)
            .bind(&texts)
            .bind(&embeddings)
            .bind(job.claimed)
            .execute(&self.pool)
            .await;
            let inserted = match inserted {
                Ok(done) => done.rows_affected(),
                Err(error) => {
                    // The document was deleted while it was being indexed.
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM knowledge_documents WHERE id=$1)",
                    )
                    .bind(job.id)
                    .fetch_one(&self.pool)
                    .await?;
                    if !exists {
                        return Ok(());
                    }
                    return Err(error.into());
                }
            };
            // Queued again or taken over: the newer run indexes it.
            if inserted == 0 {
                return Ok(());
            }
            // Renews the claim, so a long document isn't taken over while it's indexed.
            let Some(renewed) = sqlx::query_scalar("UPDATE knowledge_documents SET progress_done=$2, claimed_at=NOW(), updated_at=NOW() WHERE id=$1 AND claimed_at=$3 RETURNING claimed_at")
                .bind(job.id)
                .bind((start + chunk.len()) as i32)
                .bind(job.claimed)
                .fetch_optional(&self.pool)
                .await?
            else {
                return Ok(());
            };
            job.claimed = renewed;
        }
        let finished = sqlx::query("UPDATE knowledge_documents SET status='ready', stage='', passages=$2, updated_at=NOW() WHERE id=$1 AND status='processing' AND claimed_at=$3")
            .bind(job.id)
            .bind(passages.len() as i32)
            .bind(job.claimed)
            .execute(&self.pool)
            .await?;
        if finished.rows_affected() == 0 {
            return Ok(());
        }
        sqlx::query("UPDATE knowledge_collections SET updated_at=NOW() WHERE id=$1")
            .bind(job.collection)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The collections among `ids` that the person may search.
    pub async fn visible(&self, user: Uuid, ids: &[Uuid]) -> Result<Vec<Uuid>, ApiError> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT c.id FROM knowledge_collections c WHERE c.id = ANY($2) AND {VISIBLE}"
        ))
        .bind(user)
        .bind(ids)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Names of collections, for showing what a chat searched.
    pub async fn names(&self, ids: &[Uuid]) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT name FROM knowledge_collections WHERE id = ANY($1) ORDER BY name",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Passages that best answer the query: nearest by meaning and best by words,
    /// merged with reciprocal-rank fusion. Only collections the person may open count.
    pub async fn search(
        &self,
        gateway: &Gateway,
        user: &Identity,
        collections: &[Uuid],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, ApiError> {
        let query = query.trim();
        if query.is_empty() || collections.is_empty() {
            return Ok(vec![]);
        }
        let allowed = self.visible(user.id, collections).await?;
        if allowed.is_empty() {
            return Ok(vec![]);
        }
        let embedding = self.embedding().await?;
        let text: String = query.chars().take(2000).collect();
        let vector = gateway
            .embeddings(user, &embedding.model, &[embedding.query(&text)])
            .await?
            .pop()
            .map(vector_literal)
            .ok_or_else(ApiError::internal)?;
        let words = words(&text);
        let mut tx = self.pool.begin().await?;
        // Keep scanning the vector index until enough rows pass the collection filter.
        sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            "WITH vec AS (
                SELECT id, ROW_NUMBER() OVER (ORDER BY embedding <=> $2::halfvec) AS r
                FROM (SELECT id, embedding FROM knowledge_passages WHERE collection_id = ANY($1)
                      ORDER BY embedding <=> $2::halfvec LIMIT 40) v
             ), kw AS (
                SELECT id, ROW_NUMBER() OVER (ORDER BY rank DESC) AS r FROM (
                    SELECT p.id, ts_rank_cd(p.tsv, q) AS rank
                    FROM knowledge_passages p, to_tsquery('english', $3) q
                    WHERE $3 <> '' AND p.collection_id = ANY($1) AND p.tsv @@ q
                    ORDER BY rank DESC LIMIT 40) k
             ), fused AS (
                SELECT id, SUM(1.0 / (60 + r)) AS score, BOOL_OR(src = 'v') AS by_meaning, BOOL_OR(src = 'k') AS by_words
                FROM (SELECT id, r, 'v' AS src FROM vec UNION ALL SELECT id, r, 'k' FROM kw) u GROUP BY id
             )
             SELECT p.document_id, p.collection_id, p.page, p.content, d.name, f.score, f.by_meaning, f.by_words
             FROM fused f JOIN knowledge_passages p ON p.id = f.id JOIN knowledge_documents d ON d.id = p.document_id
             ORDER BY f.score DESC LIMIT $4",
        )
        .bind(&allowed)
        .bind(&vector)
        .bind(&words)
        .bind(limit as i64)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let (meaning, by_words): (bool, bool) = (r.get("by_meaning"), r.get("by_words"));
                Hit {
                    document: r.get("document_id"),
                    collection: r.get("collection_id"),
                    name: r.get("name"),
                    page: r.get("page"),
                    content: r.get("content"),
                    matched: match (meaning, by_words) {
                        (true, true) => "meaning + words",
                        (false, true) => "words",
                        _ => "meaning",
                    },
                    saved: false,
                }
            })
            .collect())
    }
}

struct Job {
    id: Uuid,
    owner: Uuid,
    collection: Uuid,
    name: String,
    mime: String,
    key: String,
    size: i64,
    attempts: i32,
    /// Identifies this claim: a document queued again or taken over by another worker
    /// gets a new one, and this run's writes then stop.
    claimed: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub document: Uuid,
    pub collection: Uuid,
    pub name: String,
    pub page: Option<i32>,
    pub content: String,
    pub matched: &'static str,
    /// A saved item rather than a collection passage; `document` is its id.
    pub saved: bool,
}

impl Hit {
    pub fn json(&self, n: usize) -> Value {
        json!({"n":n,"documentId":self.document,"collectionId":self.collection,"name":self.name,"page":self.page,"excerpt":excerpt(&self.content, 420),"matched":self.matched,"saved":self.saved})
    }
}

pub fn excerpt(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.rsplit_once(' ').map(|(a, _)| a).unwrap_or(&cut))
}

/// Cuts a vector to DIMENSIONS, renormalises it and formats it for a halfvec cast.
fn vector_literal(mut v: Vec<f32>) -> String {
    v.truncate(DIMENSIONS);
    v.resize(DIMENSIONS, 0.0);
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    let parts: Vec<String> = v.iter().map(|x| format!("{:.6}", x / norm)).collect();
    format!("[{}]", parts.join(","))
}

/// An OR query over the question's words, safe to hand to to_tsquery.
fn words(text: &str) -> String {
    let mut seen = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 2 && w.chars().all(|c| c.is_alphanumeric()))
    {
        if !seen.contains(&word) && seen.len() < 32 {
            seen.push(word);
        }
    }
    seen.join(" | ")
}

async fn identity(pool: &PgPool, id: Uuid) -> anyhow::Result<Identity> {
    let row =
        sqlx::query("SELECT id, username, display_name, role FROM users WHERE id=$1 AND enabled")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("The owner's account is disabled"))?;
    Ok(Identity {
        id: row.get("id"),
        username: row.get("username"),
        display_name: row.get("display_name"),
        role: if row.get::<String, _>("role") == "admin" {
            Role::Admin
        } else {
            Role::User
        },
        session: None,
    })
}

// ---- HTTP ----

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/knowledge/collections", get(list).post(create))
        .route(
            "/api/knowledge/collections/{id}",
            get(detail).patch(update).delete(remove),
        )
        .route(
            "/api/knowledge/collections/{id}/documents",
            post(upload).layer(DefaultBodyLimit::max(MAX_FILE_BYTES)),
        )
        .route(
            "/api/knowledge/documents/{id}",
            axum::routing::delete(remove_document),
        )
        .route("/api/knowledge/documents/{id}/retry", post(retry))
        .route("/api/knowledge/documents/{id}/file", get(file))
        .route("/api/knowledge/search", post(search))
        .route(
            "/api/conversations/{id}/knowledge",
            get(conversation_collections).put(set_conversation_collections),
        )
        .with_state(state.clone())
        .merge(saved::router(state))
}

fn knowledge(s: &AppState) -> Result<&Knowledge, ApiError> {
    s.knowledge.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Knowledge is not configured".into(),
        )
    })
}

fn bad(message: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    let rows = sqlx::query(&format!(
        "SELECT c.id, c.name, c.description, c.scope, c.group_id, g.name AS group_name, c.owner_id = $1 AS mine,
                u.display_name AS owner_name, c.updated_at,
                COUNT(d.id) AS documents, COUNT(d.id) FILTER (WHERE d.status IN ('queued','processing')) AS processing,
                COUNT(d.id) FILTER (WHERE d.status IN ('failed','needs_ocr')) AS problems,
                COALESCE(SUM(d.passages), 0)::bigint AS passages, COALESCE(SUM(d.size), 0)::bigint AS bytes
         FROM knowledge_collections c JOIN users u ON u.id = c.owner_id
         LEFT JOIN user_groups g ON g.id = c.group_id
         LEFT JOIN knowledge_documents d ON d.collection_id = c.id
         WHERE {VISIBLE} GROUP BY c.id, g.name, u.display_name ORDER BY c.owner_id = $1 DESC, c.updated_at DESC"
    ))
    .bind(user.id)
    .fetch_all(&k.pool)
    .await?;
    let collections: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"description":r.get::<String,_>("description"),
                "scope":r.get::<String,_>("scope"),"groupId":r.get::<Option<Uuid>,_>("group_id"),"groupName":r.get::<Option<String>,_>("group_name"),
                "mine":r.get::<bool,_>("mine"),"ownerName":r.get::<String,_>("owner_name"),"updatedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at"),
                "documents":r.get::<i64,_>("documents"),"processing":r.get::<i64,_>("processing"),"problems":r.get::<i64,_>("problems"),
                "passages":r.get::<i64,_>("passages"),"bytes":r.get::<i64,_>("bytes")})
        })
        .collect();
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size),0)::bigint FROM knowledge_documents WHERE owner_id=$1",
    )
    .bind(user.id)
    .fetch_one(&k.pool)
    .await?;
    let groups = crate::groups::shareable(&k.pool, &user).await?;
    Ok(Json(
        json!({"collections":collections,"usage":{"used":used,"limit":k.user_bytes},"groups":groups,"maxFileBytes":MAX_FILE_BYTES}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionInput {
    name: Option<String>,
    description: Option<String>,
    scope: Option<String>,
    group_id: Option<Uuid>,
}

/// Checks a sharing choice: everyone needs an administrator, a group needs membership
/// (see `groups::may_share`). `current` is the group the collection is shared with now.
async fn check_scope(
    pool: &PgPool,
    user: &Identity,
    scope: &str,
    group: Option<Uuid>,
    current: Option<Uuid>,
) -> Result<(), ApiError> {
    match scope {
        "personal" => Ok(()),
        "everyone" if user.is_admin() => Ok(()),
        "everyone" => Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only administrators can share a collection with everyone".into(),
        )),
        "group" => {
            let group = group.ok_or_else(|| bad("Choose a group to share with"))?;
            crate::groups::may_share(pool, user, group, current).await
        }
        _ => Err(bad("Unknown sharing choice")),
    }
}

fn clean_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(bad("Give the collection a name of up to 120 characters"));
    }
    Ok(name.to_owned())
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CollectionInput>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    let name = clean_name(body.name.as_deref().unwrap_or(""))?;
    let description = body
        .description
        .unwrap_or_default()
        .trim()
        .chars()
        .take(1000)
        .collect::<String>();
    let scope = body.scope.unwrap_or_else(|| "personal".into());
    check_scope(&k.pool, &user, &scope, body.group_id, None).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_collections WHERE owner_id=$1")
            .bind(user.id)
            .fetch_one(&k.pool)
            .await?;
    if count >= 100 {
        return Err(bad("You have 100 collections; delete one to make another"));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO knowledge_collections (id, owner_id, name, description, scope, group_id) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(id)
        .bind(user.id)
        .bind(&name)
        .bind(&description)
        .bind(&scope)
        .bind(if scope == "group" { body.group_id } else { None })
        .execute(&k.pool)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"collection":{"id":id,"name":name,"description":description,"scope":scope}})),
    ))
}

/// The collection if the person may see it, with whether they own it.
async fn visible_collection(
    pool: &PgPool,
    user: Uuid,
    id: Uuid,
) -> Result<(Value, bool), ApiError> {
    let row = sqlx::query(&format!(
        "SELECT c.id, c.name, c.description, c.scope, c.group_id, g.name AS group_name, c.owner_id = $1 AS mine, u.display_name AS owner_name
         FROM knowledge_collections c JOIN users u ON u.id=c.owner_id LEFT JOIN user_groups g ON g.id=c.group_id
         WHERE c.id=$2 AND {VISIBLE}"
    ))
    .bind(user)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let mine: bool = row.get("mine");
    Ok((
        json!({"id":row.get::<Uuid,_>("id"),"name":row.get::<String,_>("name"),"description":row.get::<String,_>("description"),
            "scope":row.get::<String,_>("scope"),"groupId":row.get::<Option<Uuid>,_>("group_id"),"groupName":row.get::<Option<String>,_>("group_name"),
            "mine":mine,"ownerName":row.get::<String,_>("owner_name")}),
        mine,
    ))
}

async fn owned(pool: &PgPool, user: Uuid, id: Uuid) -> Result<(), ApiError> {
    let (_, mine) = visible_collection(pool, user, id).await?;
    if mine {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the collection's owner can change it".into(),
        ))
    }
}

async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    let (collection, _) = visible_collection(&k.pool, user.id, id).await?;
    let documents: Vec<Value> = sqlx::query(
        "SELECT id, name, mime_type, size, status, stage, progress_done, progress_total, pages, passages, error, created_at
         FROM knowledge_documents WHERE collection_id=$1 ORDER BY created_at DESC",
    )
    .bind(id)
    .fetch_all(&k.pool)
    .await?
    .iter()
    .map(document_json)
    .collect();
    Ok(Json(json!({"collection":collection,"documents":documents})))
}

fn document_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"mimeType":r.get::<String,_>("mime_type"),"size":r.get::<i64,_>("size"),
        "status":r.get::<String,_>("status"),"stage":r.get::<String,_>("stage"),"done":r.get::<i32,_>("progress_done"),"total":r.get::<i32,_>("progress_total"),
        "pages":r.get::<Option<i32>,_>("pages"),"passages":r.get::<i32,_>("passages"),"error":r.get::<Option<String>,_>("error"),
        "createdAt":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at")})
}

async fn update(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<CollectionInput>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    owned(&k.pool, user.id, id).await?;
    if let Some(scope) = &body.scope {
        let current: Option<Uuid> =
            sqlx::query_scalar("SELECT group_id FROM knowledge_collections WHERE id=$1")
                .bind(id)
                .fetch_one(&k.pool)
                .await?;
        check_scope(&k.pool, &user, scope, body.group_id, current).await?;
    }
    let name = body.name.as_deref().map(clean_name).transpose()?;
    let description = body
        .description
        .map(|d| d.trim().chars().take(1000).collect::<String>());
    sqlx::query(
        "UPDATE knowledge_collections SET name=COALESCE($2,name), description=COALESCE($3,description),
         scope=COALESCE($4,scope), group_id=CASE WHEN $4 IS NULL THEN group_id WHEN $4='group' THEN $5 ELSE NULL END, updated_at=NOW() WHERE id=$1",
    )
    .bind(id)
    .bind(name)
    .bind(description)
    .bind(body.scope)
    .bind(body.group_id)
    .execute(&k.pool)
    .await?;
    let (collection, _) = visible_collection(&k.pool, user.id, id).await?;
    Ok(Json(json!({"collection":collection})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    owned(&k.pool, user.id, id).await?;
    // Files go to the trash by trigger; the worker removes them from storage.
    sqlx::query("DELETE FROM knowledge_collections WHERE id=$1")
        .bind(id)
        .execute(&k.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
}

async fn upload(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    owned(&k.pool, user.id, id).await?;
    let name: String = q
        .name
        .trim()
        .replace(['/', '\\'], "_")
        .chars()
        .take(255)
        .collect();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(bad("The file needs a name"));
    }
    if body.is_empty() {
        return Err(bad("The file is empty"));
    }
    if body.len() > MAX_FILE_BYTES {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Files must be 50 MB or smaller".into(),
        ));
    }
    let given = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let kind = extract::kind(&name, given).ok_or_else(|| {
        bad("Chat can read PDF, Word (.docx), HTML, Markdown, text and code files")
    })?;
    let mime = extract::mime_for(kind);
    let doc = Uuid::new_v4();
    let key = format!("knowledge/{}/{doc}", user.id);
    let size = body.len() as i64;
    // Check the quota before storing, then again with the owner locked when recording.
    let quota = |used: i64, count: i64| -> Result<(), ApiError> {
        if used + size > k.user_bytes {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "This would go over your {} MB of knowledge storage",
                    k.user_bytes / 1024 / 1024
                ),
            ));
        }
        if count >= MAX_DOCUMENTS {
            return Err(bad("You have reached the limit of 2,000 documents"));
        }
        Ok(())
    };
    let usage = |pool: &PgPool| {
        let pool = pool.clone();
        async move {
            sqlx::query("SELECT COALESCE(SUM(size),0)::bigint AS used, COUNT(*) AS count FROM knowledge_documents WHERE owner_id=$1")
                .bind(user.id)
                .fetch_one(&pool)
                .await
        }
    };
    let row = usage(&k.pool).await?;
    quota(row.get("used"), row.get("count"))?;
    k.store
        .put(&key, body.to_vec(), &mime)
        .await
        .map_err(|error| {
            tracing::warn!("could not store a knowledge file: {error:#}");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "The file could not be stored. Try again.".into(),
            )
        })?;
    let mut tx = k.pool.begin().await?;
    sqlx::query("SELECT 1 FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query("SELECT COALESCE(SUM(size),0)::bigint AS used, COUNT(*) AS count FROM knowledge_documents WHERE owner_id=$1")
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await?;
    if let Err(error) = quota(row.get("used"), row.get("count")) {
        drop(tx);
        let _ = k.store.delete(&key).await;
        return Err(error);
    }
    let inserted = sqlx::query(
        "INSERT INTO knowledge_documents (id, collection_id, owner_id, name, mime_type, size, object_key) VALUES ($1,$2,$3,$4,$5,$6,$7)
         RETURNING id, name, mime_type, size, status, stage, progress_done, progress_total, pages, passages, error, created_at",
    )
    .bind(doc)
    .bind(id)
    .bind(user.id)
    .bind(&name)
    .bind(&mime)
    .bind(size)
    .bind(&key)
    .fetch_one(&mut *tx)
    .await;
    match inserted {
        Ok(row) => {
            tx.commit().await?;
            Ok((
                StatusCode::CREATED,
                Json(json!({"document":document_json(&row)})),
            ))
        }
        Err(error) => {
            drop(tx);
            let _ = k.store.delete(&key).await;
            Err(error.into())
        }
    }
}

/// A document the person may see, with its collection and whether they own it.
async fn visible_document(
    pool: &PgPool,
    user: Uuid,
    id: Uuid,
) -> Result<(sqlx::postgres::PgRow, bool), ApiError> {
    let row = sqlx::query(&format!(
        "SELECT d.id, d.name, d.mime_type, d.object_key, d.status, d.collection_id, c.owner_id = $1 AS mine
         FROM knowledge_documents d JOIN knowledge_collections c ON c.id = d.collection_id WHERE d.id=$2 AND {VISIBLE}"
    ))
    .bind(user)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let mine = row.get("mine");
    Ok((row, mine))
}

async fn remove_document(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    let (_, mine) = visible_document(&k.pool, user.id, id).await?;
    if !mine {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the collection's owner can remove documents".into(),
        ));
    }
    sqlx::query("DELETE FROM knowledge_documents WHERE id=$1")
        .bind(id)
        .execute(&k.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn retry(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    let (_, mine) = visible_document(&k.pool, user.id, id).await?;
    if !mine {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the collection's owner can retry documents".into(),
        ));
    }
    sqlx::query("UPDATE knowledge_documents SET status='queued', attempts=0, error=NULL, updated_at=NOW() WHERE id=$1 AND status='failed'")
        .bind(id)
        .execute(&k.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sends the browser to a five-minute download link for the original file.
/// Documents stored by earlier versions may carry the type their uploader sent
/// (such as text/html) in object storage itself; those are sent from here as
/// downloads, so their safety never depends on the storage honouring the type a
/// download link asks for.
async fn file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    let (row, _) = visible_document(&k.pool, user.id, id).await?;
    let (key, name, mime): (String, String, String) =
        (row.get("object_key"), row.get("name"), row.get("mime_type"));
    let stored_safely = extract::kind(&name, &mime).map(extract::mime_for) == Some(mime.clone());
    if !stored_safely {
        let bytes = k.store.get(&key, MAX_FILE_BYTES).await.map_err(|error| {
            tracing::warn!("could not read a knowledge file: {error:#}");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "The file could not be read. Try again.".into(),
            )
        })?;
        return crate::attachments::file_response(&name, &mime, bytes);
    }
    let url = k.store.presign_get(&key, 300, &name, &mime);
    let mut response = Redirect::to(url.as_str()).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchInput {
    query: String,
    collection_ids: Vec<Uuid>,
    limit: Option<usize>,
}

async fn search(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SearchInput>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    let gateway = s.gateway.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Gateway integration is not configured".into(),
        )
    })?;
    let hits = k
        .search(
            gateway,
            &user,
            &body.collection_ids,
            &body.query,
            body.limit.unwrap_or(6).clamp(1, 20),
        )
        .await?;
    Ok(Json(
        json!({"results":hits.iter().enumerate().map(|(i, h)| h.json(i + 1)).collect::<Vec<_>>()}),
    ))
}

/// The collections a conversation searches, among those the person may still see.
pub async fn for_conversation(
    pool: &PgPool,
    user: Uuid,
    conversation: Uuid,
) -> Result<Vec<Uuid>, ApiError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT c.id FROM conversation_collections cc JOIN knowledge_collections c ON c.id = cc.collection_id
         JOIN conversations v ON v.id = cc.conversation_id
         WHERE cc.conversation_id=$2 AND v.owner_id=$1 AND {VISIBLE} ORDER BY c.name"
    ))
    .bind(user)
    .bind(conversation)
    .fetch_all(pool)
    .await?)
}

async fn conversation_collections(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    Ok(Json(
        json!({"collectionIds":for_conversation(&k.pool, user.id, id).await?}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationCollections {
    collection_ids: Vec<Uuid>,
}

async fn set_conversation_collections(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ConversationCollections>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    set_for_conversation(&k.pool, &user, id, &body.collection_ids).await?;
    Ok(Json(
        json!({"collectionIds":for_conversation(&k.pool, user.id, id).await?}),
    ))
}

/// Replaces a conversation's collections with the visible ones among `ids`.
pub async fn set_for_conversation(
    pool: &PgPool,
    user: &Identity,
    conversation: Uuid,
    ids: &[Uuid],
) -> Result<(), ApiError> {
    if ids.len() > 8 {
        return Err(bad("A chat can search up to 8 collections"));
    }
    let mut tx = pool.begin().await?;
    let owns: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1 AND owner_id=$2)",
    )
    .bind(conversation)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if !owns {
        return Err(ApiError::not_found());
    }
    sqlx::query("DELETE FROM conversation_collections WHERE conversation_id=$1")
        .bind(conversation)
        .execute(&mut *tx)
        .await?;
    sqlx::query(&format!(
        "INSERT INTO conversation_collections (conversation_id, collection_id)
         SELECT $2, c.id FROM knowledge_collections c WHERE c.id = ANY($3) AND {VISIBLE}"
    ))
    .bind(user.id)
    .bind(conversation)
    .bind(ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_vectors_are_padded() {
        let literal = vector_literal(vec![0.0, 3.0, 4.0]);
        let values: Vec<f32> = literal
            .trim_matches(['[', ']'])
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(values.len(), DIMENSIONS);
        assert!((values[1] - 0.6).abs() < 1e-3 && (values[2] - 0.8).abs() < 1e-3);
        assert!(values[3..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn vectors_are_cut_and_normalised() {
        let literal = vector_literal(vec![3.0; 4096]);
        let values: Vec<f32> = literal
            .trim_matches(['[', ']'])
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(values.len(), DIMENSIONS);
        let norm: f32 = values.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }

    #[test]
    fn keyword_queries_are_safe_or_queries() {
        assert_eq!(
            words("Why does entropy rise? (dS = dQ/T) & 'free' expansion!"),
            "why | does | entropy | rise | ds | dq | free | expansion"
        );
        assert_eq!(words("!!! ''"), "");
    }
}
