//! Knowledge: collections of documents the model can search and cite.
//!
//! A file is stored in object storage, then a background worker reads its text,
//! splits it into passages, embeds them through the gateway as the file's owner and
//! indexes them in Postgres for vector and full-text search. Every read checks that
//! the person may open the collection: their own, a group they belong to, or one an
//! administrator shared with everyone.

pub mod attached;
pub mod chat;
pub mod embedding;
pub mod extract;
pub mod saved;
pub mod storage;

use crate::attachments::size_label;
use crate::config::env::text;
use crate::connections::wakeups::Wakeups;
use crate::paging::{Page, PageQuery};
use crate::store::{self, Require};
use crate::{auth::Identity, error::ApiError, gateway::Gateway, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::{
    env,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use storage::ObjectStore;

/// Embedding width stored for every passage. Models that return more dimensions are
/// cut to this many and renormalised (Matryoshka-trained models such as Qwen3-Embedding
/// keep their quality when shortened); shorter vectors are padded with zeros, which
/// leaves their cosine distances unchanged.
pub const DIMENSIONS: usize = 1024;
const MAX_DOCUMENTS: i64 = 2000;
const EMBED_BATCH: usize = 16;
/// Batches of a document embedded at once; their passages are stored in order.
const EMBED_CONCURRENCY: usize = 3;
/// Tries of a batch the gateway refused for a reason that passes (busy, a gateway
/// error, a timeout), with a doubling pause from EMBED_BACKOFF between them.
const EMBED_TRIES: u32 = 3;
const EMBED_BACKOFF: Duration = Duration::from_millis(500);
/// The wake-up key for queued work: documents to index and items to embed.
const QUEUE: &str = "knowledge-queue";
/// How often the worker checks that the embedding settings still match the vectors;
/// saving the settings checks at once.
const RECONCILE_EVERY: Duration = Duration::from_secs(60);
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
    /// Wakes the worker when work is queued, on every replica.
    wakeups: Wakeups,
}

impl Knowledge {
    /// Enabled when object storage is configured.
    pub fn from_env(
        pool: PgPool,
        store: Option<ObjectStore>,
        user_mb: u32,
    ) -> anyhow::Result<Option<Self>> {
        let Some(store) = store else {
            return Ok(None);
        };
        let mut knowledge = Self::new(
            pool,
            store,
            extract::Tools::from_env(),
            text("OBLETH_CHAT_EMBEDDING_MODEL").unwrap_or_else(|| "qwen3-embedding-8b".into()),
            i64::from(user_mb) * 1024 * 1024,
        );
        // Unlike every other setting, set but empty means something here: no prefix.
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
            wakeups: Wakeups::default(),
        }
    }

    /// Shares the replica's wake-ups, which reach the other replicas too.
    pub fn with_wakeups(mut self, wakeups: Wakeups) -> Self {
        self.wakeups = wakeups;
        self
    }

    /// Tells the workers that work was queued.
    pub(crate) async fn queued(&self) {
        self.wakeups.send(&self.pool, QUEUE).await;
    }

    /// The tools documents are read with.
    pub fn tools(&self) -> &extract::Tools {
        &self.tools
    }

    /// Prefixes for the default model, as the environment variables give them.
    pub fn with_prompts(mut self, prompts: Prompts) -> Self {
        self.env_prompts = (Some(prompts.query), Some(prompts.document));
        self
    }

    /// Processes queued documents and embeds saved items until shutdown. Idle, it waits
    /// for a wake-up, looking for work anyway every KNOWLEDGE_IDLE.
    pub async fn worker(self, gateway: Gateway, shutdown: CancellationToken) {
        let mut reconciled: Option<Instant> = None;
        loop {
            if reconciled.is_none_or(|at| at.elapsed() >= RECONCILE_EVERY) {
                if let Err(error) = self.reconcile().await {
                    tracing::warn!("could not check the embedding settings: {error:#}");
                }
                reconciled = Some(Instant::now());
            }
            let embedded = self.embed_saved(&gateway).await;
            let worked = embedded
                || match self.claim().await {
                    Ok(Some(job)) => {
                        let id = job.id;
                        // Each file is read in its own task, so one that makes Chat panic
                        // fails on its own instead of stopping indexing for everyone.
                        let (knowledge, gateway) = (self.clone(), gateway.clone());
                        let result =
                            tokio::spawn(async move { knowledge.process(&gateway, job).await })
                                .await
                                .unwrap_or_else(|_| {
                                    Err(anyhow::anyhow!("Chat failed while reading this file"))
                                });
                        if let Err(error) = result {
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
            let wait = if worked {
                Duration::ZERO
            } else {
                crate::worker::KNOWLEDGE_IDLE
            };
            if !crate::worker::pause(&shutdown, wait, Some((&self.wakeups, QUEUE))).await {
                return;
            }
        }
    }

    /// Takes the oldest queued document, or one whose worker stopped mid-way. Documents
    /// attached in a chat go first: someone is waiting on them.
    async fn claim(&self) -> anyhow::Result<Option<Job>> {
        let row = sqlx::query(
            "UPDATE knowledge_documents SET status='processing', stage='reading', error=NULL, claimed_at=NOW(), attempts=attempts+1, updated_at=NOW()
             WHERE id = (SELECT id FROM knowledge_documents
                         WHERE status='queued' OR (status='processing' AND claimed_at < NOW() - INTERVAL '10 minutes')
                         ORDER BY object_key IS NOT NULL, created_at FOR UPDATE SKIP LOCKED LIMIT 1)
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
        let owner = crate::background::identity_of(&self.pool, job.owner)
            .await?
            .ok_or_else(|| anyhow::anyhow!("The owner's account is disabled"))?;
        let embedding = self.embedding().await?;
        let kind = extract::kind(&job.name, &job.mime)
            .ok_or_else(|| anyhow::anyhow!("This file type is not supported"))?;
        let bytes = match &job.key {
            Some(key) => self.store.get(key, job.size as usize + 1024).await?,
            None => self.attachment_bytes(job.collection).await?,
        };
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
        // Splitting is CPU work on the whole text, so it runs off the async workers.
        let passages = tokio::task::spawn_blocking(move || extract::split(&pages)).await?;
        if passages.is_empty() {
            anyhow::bail!("No text was found in this file");
        }
        // A run that stopped part-way resumes after the passages it stored, when the
        // file splits the same way; a change of embedding settings starts again at 0.
        let (done, total): (i32, i32) = sqlx::query_as(
            "SELECT progress_done, progress_total FROM knowledge_documents WHERE id=$1",
        )
        .bind(job.id)
        .fetch_one(&self.pool)
        .await?;
        let from = if total as usize == passages.len() {
            (done.max(0) as usize).min(passages.len())
        } else {
            0
        };
        sqlx::query("DELETE FROM knowledge_passages WHERE document_id=$1 AND ordinal >= $2")
            .bind(job.id)
            .bind(from as i32)
            .execute(&self.pool)
            .await?;
        let started = sqlx::query("UPDATE knowledge_documents SET stage='embedding', progress_done=$5, progress_total=$2, pages=$3, updated_at=NOW() WHERE id=$1 AND status='processing' AND claimed_at=$4")
            .bind(job.id)
            .bind(passages.len() as i32)
            .bind(page_count)
            .bind(job.claimed)
            .bind(from as i32)
            .execute(&self.pool)
            .await?;
        if started.rows_affected() == 0 {
            return Ok(());
        }
        // A few batches are embedded at once; each is stored in order as it is ready.
        let inputs: Vec<(usize, Vec<String>)> = passages[from..]
            .chunks(EMBED_BATCH)
            .enumerate()
            .map(|(batch, chunk)| {
                let input = chunk.iter().map(|p| embedding.document(&p.text)).collect();
                (from + batch * EMBED_BATCH, input)
            })
            .collect();
        let (owner, model) = (&owner, &embedding.model);
        let mut batches =
            futures_util::stream::iter(inputs.into_iter().map(|(start, input)| async move {
                embed_batch(gateway, owner, model, &input)
                    .await
                    .map(|vectors| (start, vectors))
            }))
            .buffered(EMBED_CONCURRENCY);
        while let Some(embedded) = batches.next().await {
            let (start, vectors) = embedded?;
            let chunk = &passages[start..(start + EMBED_BATCH).min(passages.len())];
            let ordinals: Vec<i32> = (start..start + chunk.len()).map(|i| i as i32).collect();
            let pages: Vec<Option<i32>> = chunk.iter().map(|p| p.page).collect();
            let sections: Vec<Option<String>> = chunk
                .iter()
                .map(|p| p.section.as_ref().map(|s| s.chars().take(1000).collect()))
                .collect();
            let texts: Vec<String> = chunk
                .iter()
                .map(|p| p.text.chars().take(7900).collect())
                .collect();
            let embeddings: Vec<String> = vectors.into_iter().map(vector_literal).collect();
            let inserted = sqlx::query(
                "INSERT INTO knowledge_passages (document_id, collection_id, ordinal, page, section, content, embedding)
                 SELECT $1, $2, o, p, s, t, e::halfvec FROM UNNEST($3::int[], $4::int[], $5::text[], $6::text[], $8::text[]) AS u(o, p, t, e, s)
                 WHERE EXISTS (SELECT 1 FROM knowledge_documents WHERE id=$1 AND status='processing' AND claimed_at=$7)",
            )
            .bind(job.id)
            .bind(job.collection)
            .bind(&ordinals)
            .bind(&pages)
            .bind(&texts)
            .bind(&embeddings)
            .bind(job.claimed)
            .bind(&sections)
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
        // The stored passages' length, overlaps included, which a notebook's meter adds up.
        let text_chars: i64 = passages
            .iter()
            .map(|p| p.text.chars().count().min(7900) as i64)
            .sum();
        let finished = sqlx::query("UPDATE knowledge_documents SET status='ready', stage='', passages=$2, text_chars=$4, updated_at=NOW() WHERE id=$1 AND status='processing' AND claimed_at=$3")
            .bind(job.id)
            .bind(passages.len() as i32)
            .bind(job.claimed)
            .bind(text_chars)
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

    /// Where the attached file a chat's own collection holds is kept: its row's
    /// bytes, or its key in the bucket.
    async fn attachment_row(
        &self,
        collection: Uuid,
    ) -> anyhow::Result<(Option<Vec<u8>>, Option<String>)> {
        sqlx::query_as(
            "SELECT a.data, a.object_key FROM knowledge_collections c JOIN attachments a ON a.id = c.attachment_id WHERE c.id = $1",
        )
        .bind(collection)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("The attached file is gone"))
    }

    /// The bytes of the attached file a chat's own collection holds.
    async fn attachment_bytes(&self, collection: Uuid) -> anyhow::Result<Vec<u8>> {
        let (data, key) = self.attachment_row(collection).await?;
        crate::files::Files::new(self.pool.clone(), Some(self.store.clone()))
            .read(data, key, crate::attachments::MAX_FILE_BYTES)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.1))
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
        self.search_in(gateway, user, collections, None, query, limit)
            .await
    }

    /// `search`, limited to `documents` when given (a notebook's ticked sources).
    pub async fn search_in(
        &self,
        gateway: &Gateway,
        user: &Identity,
        collections: &[Uuid],
        documents: Option<&[Uuid]>,
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
        self.passages(&allowed, documents, &vector, &words(&text), limit)
            .await
    }

    /// `search_in` for a turn's question, whose vector memory may already have made.
    pub async fn search_question(
        &self,
        gateway: &Gateway,
        user: &Identity,
        collections: &[Uuid],
        documents: Option<&[Uuid]>,
        question: &chat::Question,
        limit: usize,
    ) -> Result<Vec<Hit>, ApiError> {
        if question.text.is_empty() || collections.is_empty() {
            return Ok(vec![]);
        }
        let allowed = self.visible(user.id, collections).await?;
        if allowed.is_empty() {
            return Ok(vec![]);
        }
        let Some(vector) = question.vector(self, gateway, user).await else {
            return Ok(vec![]);
        };
        self.passages(&allowed, documents, vector, &words(&question.text), limit)
            .await
    }

    /// Passages in `allowed` collections, and in `documents` when given, nearest
    /// `vector` and best for `words`.
    pub(crate) async fn passages(
        &self,
        allowed: &[Uuid],
        documents: Option<&[Uuid]>,
        vector: &str,
        words: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, ApiError> {
        let mut tx = self.pool.begin().await?;
        // Keep scanning the vector index until enough rows pass the collection filter.
        sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            "WITH vec AS (
                SELECT id, ROW_NUMBER() OVER (ORDER BY embedding <=> $2::halfvec) AS r
                FROM (SELECT id, embedding FROM knowledge_passages WHERE collection_id = ANY($1)
                      AND ($5::uuid[] IS NULL OR document_id = ANY($5)) ORDER BY embedding <=> $2::halfvec LIMIT 40) v
             ), kw AS (
                SELECT id, ROW_NUMBER() OVER (ORDER BY rank DESC) AS r FROM (
                    SELECT p.id, ts_rank_cd(p.tsv, q) AS rank
                    FROM knowledge_passages p, to_tsquery('english', $3) q
                    WHERE $3 <> '' AND p.collection_id = ANY($1) AND ($5::uuid[] IS NULL OR p.document_id = ANY($5)) AND p.tsv @@ q
                    ORDER BY rank DESC LIMIT 40) k
             ), fused AS (
                SELECT id, SUM(1.0 / (60 + r)) AS score, BOOL_OR(src = 'v') AS by_meaning, BOOL_OR(src = 'k') AS by_words
                FROM (SELECT id, r, 'v' AS src FROM vec UNION ALL SELECT id, r, 'k' FROM kw) u GROUP BY id
             )
             SELECT p.document_id, p.collection_id, p.page, p.section, p.content, d.name, f.score, f.by_meaning, f.by_words
             FROM fused f JOIN knowledge_passages p ON p.id = f.id JOIN knowledge_documents d ON d.id = p.document_id
             ORDER BY f.score DESC LIMIT $4",
        )
        .bind(allowed)
        .bind(vector)
        .bind(words)
        .bind(limit as i64)
        .bind(documents)
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
                    section: r.get("section"),
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
    /// None for a document attached in a chat, which reads the attachment's bytes.
    key: Option<String>,
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
    /// The headings the passage sits under, outermost first.
    pub section: Option<String>,
    pub content: String,
    pub matched: &'static str,
    /// A saved item rather than a collection passage; `document` is its id.
    pub saved: bool,
}

impl Hit {
    pub fn json(&self, n: usize) -> Value {
        json!({"n":n,"documentId":self.document,"collectionId":self.collection,"name":self.name,"page":self.page,"section":self.section,"excerpt":excerpt(&self.content, 420),"matched":self.matched,"saved":self.saved})
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
pub(crate) fn vector_literal(mut v: Vec<f32>) -> String {
    v.truncate(DIMENSIONS);
    v.resize(DIMENSIONS, 0.0);
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    let parts: Vec<String> = v.iter().map(|x| format!("{:.6}", x / norm)).collect();
    format!("[{}]", parts.join(","))
}

/// An OR query over the question's words, safe to hand to to_tsquery.
pub(crate) fn words(text: &str) -> String {
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

/// One batch of a document's passages embedded as its owner, tried again after a
/// refusal that passes. An owner who must sign in again stops it at once.
async fn embed_batch(
    gateway: &Gateway,
    owner: &Identity,
    model: &str,
    input: &[String],
) -> anyhow::Result<Vec<Vec<f32>>> {
    let mut tries = 1;
    loop {
        match gateway.embeddings(owner, model, input).await {
            Ok(vectors) => return Ok(vectors),
            Err(e) if e.code() == Some(crate::error::code::SIGN_IN_AGAIN) => {
                // Retrying can't help until the owner signs in, so say that.
                anyhow::bail!(
                    "{}: the file's owner must sign in before it can be indexed",
                    crate::auth::grants::SIGN_IN_AGAIN
                )
            }
            Err(e) if tries < EMBED_TRIES && passing(e.0) => {
                tokio::time::sleep(EMBED_BACKOFF * 2u32.pow(tries - 1)).await;
                tries += 1;
            }
            Err(e) => anyhow::bail!("The embedding model is unavailable ({})", e.1),
        }
    }
}

/// A gateway answer worth trying again: busy, a gateway error, or no answer in time.
fn passing(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
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
            // `upload` reads its own body, after sign-in, up to MAX_FILE_BYTES.
            post(upload),
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
    s.knowledge
        .as_ref()
        .ok_or_else(|| ApiError::feature_off("Knowledge is not configured"))
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
         WHERE {VISIBLE} AND c.attachment_id IS NULL GROUP BY c.id, g.name, u.display_name ORDER BY c.owner_id = $1 DESC, c.updated_at DESC"
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
    // Documents attached in chats count against the person's files, not here.
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size),0)::bigint FROM knowledge_documents WHERE owner_id=$1 AND object_key IS NOT NULL",
    )
    .bind(user.id)
    .fetch_one(&k.pool)
    .await?;
    let groups = crate::groups::shareable(&k.pool, &user).await?;
    Ok(Json(
        json!({"collections":collections,"usage":{"used":used,"limit":k.user_bytes},"groups":groups,"maxFileBytes":s.store.limits.max_file_bytes}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
        "everyone" => Err(ApiError::denied(
            "Only administrators can share a collection with everyone",
        )),
        "group" => {
            let group = group.ok_or_else(|| ApiError::bad("Choose a group to share with"))?;
            crate::groups::may_share(pool, user, group, current).await
        }
        _ => Err(ApiError::bad("Unknown sharing choice")),
    }
}

fn clean_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::bad(
            "Give the collection a name of up to 120 characters",
        ));
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
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_collections WHERE owner_id=$1 AND attachment_id IS NULL",
    )
    .bind(user.id)
    .fetch_one(&k.pool)
    .await?;
    if count >= 100 {
        return Err(ApiError::bad(
            "You have 100 collections; delete one to make another",
        ));
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
        Err(ApiError::denied(
            "Only the collection's owner can change it",
        ))
    }
}

/// Documents per page of a collection, unless the request asks for a limit.
const DOCUMENTS_LISTED: i64 = 100;

async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    let (collection, _) = visible_collection(&k.pool, user.id, id).await?;
    let (documents, more) = documents_for(&k.pool, id, page.page(DOCUMENTS_LISTED)).await?;
    Ok(Json(
        json!({"collection":collection,"documents":documents,"more":more}),
    ))
}

/// A page of a collection's documents, newest first.
async fn documents_for(
    pool: &PgPool,
    collection: Uuid,
    page: Page,
) -> Result<(Vec<Value>, bool), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, mime_type, size, status, stage, progress_done, progress_total, pages, passages, error, created_at
         FROM knowledge_documents WHERE collection_id=$1 ORDER BY created_at DESC, id LIMIT $2 OFFSET $3",
    )
    .bind(collection)
    .bind(page.fetch())
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok(page.cut(rows.iter().map(document_json).collect()))
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
    body: axum::body::Body,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let given = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .or_else(|| axum::body::HttpBody::size_hint(&body).exact());
    let limits = s.store.limits;
    let too_large = || {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Files can be at most {}. Choose a smaller file.",
                size_label(limits.max_file_bytes)
            ),
        )
    };
    let usage = |pool: &PgPool, owner: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query("SELECT COALESCE(SUM(size),0)::bigint AS used, COUNT(*) AS count FROM knowledge_documents WHERE owner_id=$1 AND object_key IS NOT NULL")
                .bind(owner)
                .fetch_one(&pool)
                .await
        }
    };
    // Everything that can refuse the file is checked before any of it is read: who
    // sends it, its name and type, and room for it in the person's storage.
    let allowed = async {
        let user = s.auth.require(&headers, true).await?;
        let k = knowledge(&s)?;
        owned(&k.pool, user.id, id).await?;
        let name = crate::attachments::clean_upload_name(&q.name)?;
        let kind = extract::kind(&name, given).ok_or_else(|| {
            ApiError::bad("Chat can read PDF, Word (.docx), PowerPoint (.pptx), Excel (.xlsx), HTML, Markdown, text and code files")
        })?;
        if declared.is_some_and(|size| size > limits.max_file_bytes as u64) {
            return Err(too_large());
        }
        let row = usage(&k.pool, user.id).await?;
        room(
            row.get("used"),
            row.get("count"),
            declared.map_or(1, |size| size as i64),
            k.user_bytes,
        )?;
        Ok::<_, ApiError>((user, k, name, kind))
    };
    let ((user, k, name, kind), body) = crate::error::admitted(body, allowed.await).await?;
    // Room in the memory uploads share, for the size the upload states or the most a
    // file may be, so many at once wait their turn instead of adding up.
    let _held =
        crate::attachments::hold(declared.map_or(limits.max_file_bytes, |size| size as usize))
            .await?;
    let body = crate::error::upload(body, limits.max_file_bytes, too_large().1).await?;
    if body.is_empty() {
        return Err(ApiError::bad("The file is empty"));
    }
    let mime = extract::mime_for(kind);
    let doc = Uuid::new_v4();
    let key = format!("knowledge/{}/{doc}", user.id);
    let size = body.len() as i64;
    // Checked again now the size is known, and once more with the owner locked when
    // recording.
    let row = usage(&k.pool, user.id).await?;
    room(row.get("used"), row.get("count"), size, k.user_bytes)?;
    // The object goes to the bucket under the pending-trash protocol: its row claims
    // it, and one whose row is never written is swept after the grace period.
    let files = &s.store.files;
    let placed = files.place(key.clone(), body, &mime).await?;
    let recorded = async {
        let mut tx = k.pool.begin().await?;
        // The person was checked as the request came in.
        store::lock_owner(&mut tx, user.id, Require::Any).await?;
        let row = sqlx::query("SELECT COALESCE(SUM(size),0)::bigint AS used, COUNT(*) AS count FROM knowledge_documents WHERE owner_id=$1 AND object_key IS NOT NULL")
            .bind(user.id)
            .fetch_one(&mut *tx)
            .await?;
        room(row.get("used"), row.get("count"), size, k.user_bytes)?;
        let row = sqlx::query(
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
        .await?;
        tx.commit().await?;
        Ok::<_, ApiError>(row)
    }
    .await;
    match recorded {
        Ok(row) => {
            k.queued().await;
            Ok((
                StatusCode::CREATED,
                Json(json!({"document":document_json(&row)})),
            ))
        }
        Err(error) => {
            files.discard(&placed).await;
            Err(error)
        }
    }
}

/// Whether one more document of `size` bytes fits in the person's knowledge
/// storage. A full quota or document count is 409: the request was fine, the
/// account is full.
fn room(used: i64, count: i64, size: i64, quota: i64) -> Result<(), ApiError> {
    if used + size > quota {
        return Err(ApiError::conflict(format!(
            "This would go over your {} of knowledge storage",
            size_label(quota as usize)
        )));
    }
    if count >= MAX_DOCUMENTS {
        return Err(ApiError::conflict(format!(
            "You have reached the limit of {MAX_DOCUMENTS} documents"
        )));
    }
    Ok(())
}

/// A person's knowledge storage in use: bytes and documents, as `room` counts them.
async fn used_by<'e>(
    db: impl sqlx::PgExecutor<'e>,
    owner: Uuid,
) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as("SELECT COALESCE(SUM(size),0)::bigint, COUNT(*) FROM knowledge_documents WHERE owner_id=$1 AND object_key IS NOT NULL")
        .bind(owner)
        .fetch_one(db)
        .await
}

impl Knowledge {
    /// Keeps a web page read in its owner's browser as a document in their collection,
    /// stored and indexed as an upload is, with the address it came from.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn store_page(
        &self,
        files: &crate::files::Files,
        owner: Uuid,
        collection: Uuid,
        name: &str,
        kind: extract::Kind,
        bytes: Vec<u8>,
        source_url: &str,
    ) -> Result<Uuid, ApiError> {
        let size = bytes.len() as i64;
        if size == 0 {
            return Err(ApiError::bad("The page is empty"));
        }
        let (used, count) = used_by(&self.pool, owner).await?;
        room(used, count, size, self.user_bytes)?;
        let doc = Uuid::new_v4();
        let mime = extract::mime_for(kind);
        let placed = files
            .place(format!("knowledge/{owner}/{doc}"), bytes, &mime)
            .await?;
        let recorded = async {
            let key = placed.key.as_deref().ok_or_else(|| {
                ApiError::unavailable("Knowledge needs object storage, which is not configured")
            })?;
            let mut tx = self.pool.begin().await?;
            store::lock_owner(&mut tx, owner, Require::Any).await?;
            let (used, count) = used_by(&mut *tx, owner).await?;
            room(used, count, size, self.user_bytes)?;
            let theirs: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM knowledge_collections WHERE id=$1 AND owner_id=$2 AND attachment_id IS NULL)",
            )
            .bind(collection)
            .bind(owner)
            .fetch_one(&mut *tx)
            .await?;
            if !theirs {
                return Err(ApiError::not_found());
            }
            sqlx::query(
                "INSERT INTO knowledge_documents (id, collection_id, owner_id, name, mime_type, size, object_key, source_url, read_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,NOW())",
            )
            .bind(doc)
            .bind(collection)
            .bind(owner)
            .bind(name)
            .bind(&mime)
            .bind(size)
            .bind(key)
            .bind(source_url)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok::<_, ApiError>(())
        }
        .await;
        match recorded {
            Ok(()) => {
                self.queued().await;
                Ok(doc)
            }
            Err(error) => {
                files.discard(&placed).await;
                Err(error)
            }
        }
    }

    /// Replaces a web page's document with the page as it reads now: the same document,
    /// so ticks and links to it stay, indexed again and given a new guide.
    pub(crate) async fn replace_page(
        &self,
        files: &crate::files::Files,
        owner: Uuid,
        document: Uuid,
        name: &str,
        kind: extract::Kind,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError> {
        let size = bytes.len() as i64;
        if size == 0 {
            return Err(ApiError::bad("The page is empty"));
        }
        let mime = extract::mime_for(kind);
        let placed = files
            .place(
                format!("knowledge/{owner}/{}", Uuid::new_v4()),
                bytes,
                &mime,
            )
            .await?;
        let recorded = async {
            let key = placed.key.as_deref().ok_or_else(|| {
                ApiError::unavailable("Knowledge needs object storage, which is not configured")
            })?;
            let mut tx = self.pool.begin().await?;
            store::lock_owner(&mut tx, owner, Require::Any).await?;
            let old: Option<(Option<String>, i64)> = sqlx::query_as(
                "SELECT object_key, size FROM knowledge_documents WHERE id=$1 AND owner_id=$2 AND source_url IS NOT NULL FOR UPDATE",
            )
            .bind(document)
            .bind(owner)
            .fetch_optional(&mut *tx)
            .await?;
            let (old_key, old_size) = old.ok_or_else(ApiError::not_found)?;
            let (used, count) = used_by(&mut *tx, owner).await?;
            let kept = i64::from(old_key.is_some());
            room(
                used - old_size * kept,
                count - kept,
                size,
                self.user_bytes,
            )?;
            sqlx::query(
                "UPDATE knowledge_documents SET name=$2, mime_type=$3, size=$4, object_key=$5, status='queued', stage='',
                     progress_done=0, progress_total=0, pages=NULL, passages=0, text_chars=NULL, error=NULL, attempts=0,
                     claimed_at=NULL, read_at=NOW(), updated_at=NOW()
                 WHERE id=$1",
            )
            .bind(document)
            .bind(name)
            .bind(&mime)
            .bind(size)
            .bind(key)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM knowledge_passages WHERE document_id=$1")
                .bind(document)
                .execute(&mut *tx)
                .await?;
            // The guide described the page as it was; the next pass writes a new one.
            sqlx::query("DELETE FROM notebook_guides WHERE document_id=$1")
                .bind(document)
                .execute(&mut *tx)
                .await?;
            if let Some(old_key) = old_key {
                sqlx::query("INSERT INTO object_trash (object_key) VALUES ($1) ON CONFLICT (object_key) DO UPDATE SET pending = FALSE")
                    .bind(old_key)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            Ok::<_, ApiError>(())
        }
        .await;
        match recorded {
            Ok(()) => {
                self.queued().await;
                Ok(())
            }
            Err(error) => {
                files.discard(&placed).await;
                Err(error)
            }
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
        return Err(ApiError::denied(
            "Only the collection's owner can remove documents",
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
        return Err(ApiError::denied(
            "Only the collection's owner can retry documents",
        ));
    }
    let queued = sqlx::query("UPDATE knowledge_documents SET status='queued', attempts=0, error=NULL, updated_at=NOW() WHERE id=$1 AND status='failed'")
        .bind(id)
        .execute(&k.pool)
        .await?;
    if queued.rows_affected() > 0 {
        k.queued().await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Sends the browser to a five-minute download link for the original file.
/// Documents stored by earlier versions may carry the type their uploader sent
/// (such as text/html) in object storage itself; those are sent from here as
/// downloads, so their safety never depends on the storage honouring the type a
/// download link asks for.
/// Whether Chat sends a stored document itself rather than linking to the bucket. Only
/// a PDF stored under its own type goes to the bucket, where it opens in place (at a
/// cited page). Anything else is sent by Chat with a safe type and under its own name:
/// some S3 services ignore the name and type a link asks for, and the file would then
/// download under its key, with no extension.
fn sent_by_chat(name: &str, mime: &str) -> bool {
    let stored_safely = extract::kind(name, mime).map(extract::mime_for).as_deref() == Some(mime);
    !stored_safely || !crate::files::served_type(mime).1
}

async fn file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    let (row, _) = visible_document(&k.pool, user.id, id).await?;
    let (key, name, mime): (Option<String>, String, String) =
        (row.get("object_key"), row.get("name"), row.get("mime_type"));
    // A document attached in a chat is the attachment itself.
    let files = &s.store.files;
    let most = crate::attachments::MAX_FILE_BYTES;
    let Some(key) = key else {
        let (data, key) = k
            .attachment_row(row.get("collection_id"))
            .await
            .map_err(|error| {
                tracing::warn!("could not read an attached document: {error:#}");
                ApiError::not_found()
            })?;
        return crate::attachments::file_response(&name, &mime, files.body(data, key, most).await?);
    };
    if sent_by_chat(&name, &mime) {
        // Streamed, not held.
        return crate::attachments::file_response(
            &name,
            &mime,
            files.body(None, Some(key), most).await?,
        );
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
#[serde(deny_unknown_fields)]
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
    let gateway = s
        .gateway
        .as_ref()
        .ok_or_else(crate::gateway::not_configured)?;
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
#[serde(deny_unknown_fields)]
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
        return Err(ApiError::bad("A chat can search up to 8 collections"));
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
    use crate::test_support::Database;

    #[test]
    fn only_a_pdf_is_linked_to_the_bucket_and_the_rest_sent_under_its_name() {
        let text = extract::mime_for(extract::Kind::Markdown);
        assert!(sent_by_chat("Watershed management - Wikipedia.md", &text));
        assert!(sent_by_chat("notes.txt", &text));
        assert!(sent_by_chat("page.html", "text/html; charset=utf-8"));
        let docx = extract::mime_for(extract::Kind::Docx);
        assert!(sent_by_chat("report.docx", &docx));
        assert!(!sent_by_chat("paper.pdf", "application/pdf"));
        assert!(
            sent_by_chat("paper.pdf", "text/html"),
            "a PDF stored under another type"
        );
    }

    #[test]
    fn a_full_account_is_a_conflict_not_a_bad_request() {
        let mib = 1024 * 1024;
        assert!(room(0, 0, mib, 10 * mib).is_ok());
        let full = room(9 * mib, 0, 2 * mib, 10 * mib).unwrap_err();
        assert_eq!(full.0, StatusCode::CONFLICT);
        assert!(full.1.contains("10.0 MB"), "{}", full.1);
        let many = room(0, MAX_DOCUMENTS, 1, 10 * mib).unwrap_err();
        assert_eq!(
            (many.0, many.1.contains("2000")),
            (StatusCode::CONFLICT, true)
        );
    }

    #[tokio::test]
    async fn a_collection_lists_its_documents_a_page_at_a_time() {
        let Some(db) = Database::new().await else {
            return;
        };
        let owner = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, username, display_name, role, password_hash) VALUES ($1, 'ana', 'Ana', 'user', 'x')")
            .bind(owner)
            .execute(&db.pool)
            .await
            .unwrap();
        let collection = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO knowledge_collections (id, owner_id, name) VALUES ($1, $2, 'Papers')",
        )
        .bind(collection)
        .bind(owner)
        .execute(&db.pool)
        .await
        .unwrap();
        for n in 0..150 {
            sqlx::query("INSERT INTO knowledge_documents (id, collection_id, owner_id, name, mime_type, size, object_key, created_at) VALUES ($1, $2, $3, $4, 'text/plain', 1, $5, NOW() - make_interval(secs => $6))")
                .bind(Uuid::new_v4())
                .bind(collection)
                .bind(owner)
                .bind(format!("paper-{n}"))
                .bind(format!("knowledge/{owner}/{n}"))
                .bind(n as f64)
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let first = PageQuery::default().page(DOCUMENTS_LISTED);
        let (documents, more) = documents_for(&db.pool, collection, first).await.unwrap();
        assert_eq!(
            (documents.len(), more, documents[0]["name"].as_str()),
            (100, true, Some("paper-0"))
        );
        let rest = PageQuery {
            offset: Some(100),
            limit: None,
        }
        .page(DOCUMENTS_LISTED);
        let (documents, more) = documents_for(&db.pool, collection, rest).await.unwrap();
        assert_eq!(
            (documents.len(), more, documents[49]["name"].as_str()),
            (50, false, Some("paper-149"))
        );
        db.close().await;
    }

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
