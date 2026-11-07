//! Memory: what Chat learns about a person across chats. After a reply, a model reads
//! the person's new messages and keeps short statements about them up to date: their
//! role and field, what they work on, the tools they use and how they like answers. Each
//! memory is linked to the topics it is about, which the Memory page draws as a graph.
//!
//! When a person writes, the memories that fit the message (and the ones they pinned)
//! go into the system prompt, and the reply records which ones it was given. People
//! can see, change, pin and delete every memory, add their own, and turn memory off.
//! Temporary chats are never read and never given memories.
use crate::{
    auth::{Identity, Role},
    error::ApiError,
    gateway::{valid_model, Gateway},
    knowledge::Knowledge,
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Memories one person may keep.
pub const MAX_MEMORIES: i64 = 500;
pub const KINDS: [&str; 4] = ["about", "work", "preference", "goal"];
pub const TOPIC_KINDS: [&str; 5] = ["project", "tool", "field", "course", "topic"];
const MAX_CONTENT: usize = 500;
const MAX_TOPIC: usize = 60;
/// Memories a chat gets: every pinned one, and the best matches for the message.
const PINNED: i64 = 10;
const RELEVANT: i64 = 8;
/// Cosine distance past which a memory does not fit the message by meaning.
const FIT_DISTANCE: f64 = 0.62;
/// Cosine distance under which a new memory repeats one already kept.
const SAME_DISTANCE: f64 = 0.08;
/// What one pass reads: each message and all of them together cut to these lengths.
const MESSAGE_CHARS: usize = 2_000;
const READ_CHARS: usize = 6_000;
const REPLY_CHARS: usize = 1_200;
/// Memories and topic names a pass shows the model, so it updates rather than repeats.
const KNOWN: i64 = 30;
const KNOWN_TOPICS: i64 = 40;
/// Changes one pass may make.
const MAX_ADDS: usize = 5;
const MAX_CHANGES: usize = 5;
const CLAIM_SECS: f64 = 300.0;
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);
/// How long finding a chat's memories may hold up the start of a reply.
const RECALL_TIMEOUT: Duration = Duration::from_secs(4);

const LEARN_PROMPT: &str = r#"You keep the memory of a person who uses an AI assistant at a university, so that later chats can be more helpful to them. Read their newest messages and decide what is worth remembering in future chats.

Remember lasting things about the person: their role and field, what they are working on (projects, research, courses, datasets, papers, deadlines), the tools, languages and systems they use, their goals, and how they like answers. Write each memory as one short sentence in the second person, such as "You are a PhD student in hydrology" or "You prefer Python examples with type hints". Be specific: name the project, tool or course.

Never remember: one-off questions or tasks that say nothing lasting about the person; what the assistant said; anything about other people; health, religion, sexuality, politics, ethnicity, immigration status, finances, grades or discipline; anything they call private; passwords, keys, tokens or account numbers. When they ask you to remember something about themselves, remember it. When they ask you to forget something, or a new message shows a memory is out of date, update or forget that memory. Do not repeat a memory that is already known; update it instead when the new message adds to it.

Each memory is about one to three topics: short names (one to four words) of a project, tool, field, course or general topic. Reuse a topic name already in use when one fits.

Reply with JSON only:
{"add": [{"text": "...", "kind": "about|work|preference|goal", "topics": [{"name": "...", "kind": "project|tool|field|course|topic"}]}], "update": [{"id": "m1", "text": "..."}], "forget": ["m2"]}
kind: about = who they are; work = what they work on; preference = how they like answers; goal = what they want to achieve. Use empty lists when nothing changes; most messages change nothing."#;

const TAG_PROMPT: &str = r#"You file a memory an AI assistant keeps about a person. Reply with JSON only: {"kind": "about|work|preference|goal", "topics": [{"name": "...", "kind": "project|tool|field|course|topic"}]}
kind: about = who they are; work = what they work on; preference = how they like answers; goal = what they want to achieve. topics: one to three short names (one to four words) of the project, tool, field, course or general topic the memory is about. Reuse a topic name already in use when one fits."#;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Settings {
    pub enabled: bool,
    pub default_on: bool,
    pub model: Option<String>,
}

pub async fn settings(pool: &PgPool) -> Result<Settings, sqlx::Error> {
    sqlx::query_as("SELECT enabled, default_on, model FROM memory_settings WHERE id")
        .fetch_one(pool)
        .await
}

/// Whether people can use memory on this server.
pub async fn available(s: &AppState) -> bool {
    s.gateway.is_some()
        && settings(&s.store.pool)
            .await
            .is_ok_and(|m| m.enabled && m.model.is_some())
}

/// A person's choices: whether chats use memories, and whether chats are read for new ones.
#[derive(Clone, Copy, Debug)]
pub struct Choices {
    pub use_memory: bool,
    pub learn: bool,
}

pub async fn choices(pool: &PgPool, user: Uuid, default_on: bool) -> Result<Choices, sqlx::Error> {
    let row: Option<(bool, bool)> =
        sqlx::query_as("SELECT use_memory, learn FROM memory_choices WHERE user_id = $1")
            .bind(user)
            .fetch_optional(pool)
            .await?;
    Ok(match row {
        Some((use_memory, learn)) => Choices { use_memory, learn },
        None => Choices {
            use_memory: default_on,
            learn: default_on,
        },
    })
}

// ---------------------------------------------------------------------------------
// Recall: the memories a reply is given.

/// The memories found for one reply.
pub struct Recall {
    items: Vec<(Uuid, String, bool)>,
}

impl Recall {
    /// The system prompt section with the memories.
    pub fn prompt(&self) -> String {
        let mut text = String::from("# What you remember about the person\nChat keeps these memories from the person's earlier chats, and they can see and change them on their Memory page. Use them when they help, for example to fit an answer to their work, their tools or how they like answers. Don't list them, and don't say \"based on my memory\" or the like; just answer well. If one seems out of date, go by what the person says now.");
        for (_, content, _) in &self.items {
            text.push_str(&format!("\n- {content}"));
        }
        text
    }

    /// The activity entry the reply keeps, so the page can show what it was given.
    pub fn entry(&self) -> Value {
        json!({"id":"memory-recalled","kind":"recalled","status":"completed",
            "summary":format!("Remembered {}", count(self.items.len())),
            "items":self.items.iter().map(|(id, content, pinned)| json!({"id":id,"text":content,"pinned":pinned})).collect::<Vec<_>>()})
    }
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 thing".into()
    } else {
        format!("{n} things")
    }
}

/// The memories for a person's message: pinned ones and the best matches. None when
/// memory is off for them, for this server or for the chat, or nothing fits.
pub async fn recall(
    s: &AppState,
    gateway: &Gateway,
    user: &Identity,
    conversation: Uuid,
    messages: &[(String, String)],
) -> Option<Recall> {
    match tokio::time::timeout(
        RECALL_TIMEOUT,
        find(s, gateway, user, conversation, messages),
    )
    .await
    {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => {
            tracing::warn!("memory recall failed: {error}");
            None
        }
        Err(_) => {
            tracing::warn!("memory recall timed out");
            None
        }
    }
}

async fn find(
    s: &AppState,
    gateway: &Gateway,
    user: &Identity,
    conversation: Uuid,
    messages: &[(String, String)],
) -> Result<Option<Recall>, sqlx::Error> {
    let pool = &s.store.pool;
    let settings = settings(pool).await?;
    if !settings.enabled {
        return Ok(None);
    }
    if !choices(pool, user.id, settings.default_on)
        .await?
        .use_memory
    {
        return Ok(None);
    }
    let temporary: bool =
        sqlx::query_scalar("SELECT temporary FROM conversations WHERE id = $1 AND owner_id = $2")
            .bind(conversation)
            .bind(user.id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(true);
    if temporary {
        return Ok(None);
    }
    let pinned: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, content FROM memories WHERE owner_id = $1 AND pinned ORDER BY updated_at DESC LIMIT $2",
    )
    .bind(user.id)
    .bind(PINNED)
    .fetch_all(pool)
    .await?;
    let query = question(messages);
    let mut items: Vec<(Uuid, String, bool)> =
        pinned.into_iter().map(|(id, c)| (id, c, true)).collect();
    if !query.is_empty() {
        let ranked = rank(
            pool,
            s.knowledge.as_ref(),
            gateway,
            user,
            &query,
            RELEVANT,
            Some(FIT_DISTANCE),
        )
        .await?;
        if !ranked.is_empty() {
            let rows: Vec<(Uuid, String)> =
                sqlx::query_as("SELECT id, content FROM memories WHERE id = ANY($1)")
                    .bind(&ranked)
                    .fetch_all(pool)
                    .await?;
            for id in ranked {
                if items.iter().any(|(i, _, _)| *i == id) {
                    continue;
                }
                if let Some((_, content)) = rows.iter().find(|(i, _)| *i == id) {
                    items.push((id, content.clone(), false));
                }
            }
        }
    }
    if items.is_empty() {
        return Ok(None);
    }
    let ids: Vec<Uuid> = items.iter().map(|(id, _, _)| *id).collect();
    sqlx::query("UPDATE memories SET uses = uses + 1, used_at = NOW() WHERE id = ANY($1)")
        .bind(&ids)
        .execute(pool)
        .await?;
    Ok(Some(Recall { items }))
}

/// The person's unpinned memories that best match `query`, by meaning and by words,
/// best first. `fit` drops matches by meaning that are further away than it.
async fn rank(
    pool: &PgPool,
    knowledge: Option<&Knowledge>,
    gateway: &Gateway,
    user: &Identity,
    query: &str,
    limit: i64,
    fit: Option<f64>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let vector = match knowledge {
        Some(k) => embed(k, gateway, user, &[query.to_owned()], true)
            .await
            .and_then(|mut v| v.pop()),
        None => None,
    };
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "WITH mine AS (SELECT id, embedding, tsv FROM memories WHERE owner_id = $1 AND NOT pinned),
         vec AS (
            SELECT id, ROW_NUMBER() OVER (ORDER BY d) AS r FROM (
                SELECT id, embedding <=> $2::halfvec AS d FROM mine
                WHERE $2::text IS NOT NULL AND embedding IS NOT NULL) v
            WHERE d <= $5 ORDER BY d LIMIT 30
         ), kw AS (
            SELECT id, ROW_NUMBER() OVER (ORDER BY rank DESC) AS r FROM (
                SELECT m.id, ts_rank_cd(m.tsv, q) AS rank FROM mine m, to_tsquery('english', $3) q
                WHERE $3 <> '' AND m.tsv @@ q ORDER BY rank DESC LIMIT 30) k
         )
         SELECT id FROM (SELECT id, r FROM vec UNION ALL SELECT id, r FROM kw) u
         GROUP BY id ORDER BY SUM(1.0 / (60 + r)) DESC LIMIT $4",
    )
    .bind(user.id)
    .bind(&vector)
    .bind(crate::knowledge::words(query))
    .bind(limit)
    .bind(fit.unwrap_or(2.0))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Embeds texts as the person, formatted for a halfvec cast; None when the embedding
/// model is not set up or did not answer.
async fn embed(
    k: &Knowledge,
    gateway: &Gateway,
    user: &Identity,
    texts: &[String],
    query: bool,
) -> Option<Vec<String>> {
    let embedding = k.embedding().await.ok()?;
    let input: Vec<String> = texts
        .iter()
        .map(|t| {
            if query {
                embedding.query(t)
            } else {
                embedding.document(t)
            }
        })
        .collect();
    match gateway.embeddings(user, &embedding.model, &input).await {
        Ok(vectors) => Some(
            vectors
                .into_iter()
                .map(crate::knowledge::vector_literal)
                .collect(),
        ),
        Err(error) => {
            tracing::info!("memory embedding deferred: {}", error.1);
            None
        }
    }
}

/// What to match memories against: the latest message, with the one before it when the
/// latest is a short follow-up.
fn question(messages: &[(String, String)]) -> String {
    let mut users = messages
        .iter()
        .rev()
        .filter(|(role, _)| role == "user")
        .map(|(_, c)| c.trim());
    let last = users.next().unwrap_or("");
    let text = if last.chars().count() < 60 {
        match users.next() {
            Some(previous) => format!("{previous}\n{last}"),
            None => last.to_owned(),
        }
    } else {
        last.to_owned()
    };
    text.chars().take(1500).collect()
}

// ---------------------------------------------------------------------------------
// Learning: reading new messages after a reply.

/// When a reply finishes: reads the chat's new messages for memories in the background.
/// Returns at once; true when a pass started, so the page looks again for what it learned.
pub async fn after_reply(s: &AppState, user: &Identity, conversation: Uuid, reply: Uuid) -> bool {
    match tokio::time::timeout(Duration::from_secs(2), start(s, user, conversation, reply)).await {
        Ok(Ok(started)) => started,
        Ok(Err(error)) => {
            tracing::warn!(conversation = %conversation, "memory after a reply: {error}");
            false
        }
        Err(_) => false,
    }
}

async fn start(
    s: &AppState,
    user: &Identity,
    conversation: Uuid,
    reply: Uuid,
) -> Result<bool, sqlx::Error> {
    let Some(gateway) = s.gateway.clone() else {
        return Ok(false);
    };
    let pool = &s.store.pool;
    let settings = settings(pool).await?;
    let (true, Some(model)) = (settings.enabled, settings.model.clone()) else {
        return Ok(false);
    };
    if !choices(pool, user.id, settings.default_on).await?.learn {
        return Ok(false);
    }
    let temporary: bool =
        sqlx::query_scalar("SELECT temporary FROM conversations WHERE id = $1 AND owner_id = $2")
            .bind(conversation)
            .bind(user.id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(true);
    if temporary {
        return Ok(false);
    }
    // The claim, unless another pass holds it or the last one failed moments ago.
    let read_through: Option<i32> = sqlx::query_scalar(
        "INSERT INTO memory_reads (conversation_id, owner_id, claimed_until)
         VALUES ($1, $2, NOW() + make_interval(secs => $3))
         ON CONFLICT (conversation_id) DO UPDATE SET claimed_until = EXCLUDED.claimed_until
         WHERE (memory_reads.claimed_until IS NULL OR memory_reads.claimed_until < NOW())
           AND (memory_reads.failed_at IS NULL
                OR memory_reads.failed_at < NOW() - make_interval(mins => 5 * memory_reads.attempts))
         RETURNING read_through",
    )
    .bind(conversation)
    .bind(user.id)
    .bind(CLAIM_SECS)
    .fetch_optional(pool)
    .await?;
    let Some(read_through) = read_through else {
        return Ok(false);
    };
    let new: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM messages WHERE conversation_id = $1 AND role = 'user' AND ordinal > $2)",
    )
    .bind(conversation)
    .bind(read_through)
    .fetch_one(pool)
    .await?;
    if !new {
        sqlx::query("UPDATE memory_reads SET claimed_until = NULL WHERE conversation_id = $1")
            .bind(conversation)
            .execute(pool)
            .await?;
        return Ok(false);
    }
    let pool = pool.clone();
    let knowledge = s.knowledge.clone();
    let user = user.clone();
    tokio::spawn(async move {
        let pass = Pass {
            pool: &pool,
            knowledge: knowledge.as_ref(),
            gateway: &gateway,
            model: &model,
            user: &user,
        };
        let done = pass.learn(conversation, reply, read_through).await;
        if let Err(error) = settle(&pool, conversation, done).await {
            tracing::warn!(conversation = %conversation, "memory after a reply: {error}");
        }
    });
    Ok(true)
}

enum Failure {
    Database(sqlx::Error),
    /// The gateway or the model gave no usable answer; the chat is read again later.
    Answer(String),
}

impl From<sqlx::Error> for Failure {
    fn from(error: sqlx::Error) -> Self {
        Failure::Database(error)
    }
}

async fn settle(pool: &PgPool, id: Uuid, done: Result<(), Failure>) -> Result<(), sqlx::Error> {
    match done {
        Ok(()) => Ok(()),
        Err(failure) => {
            if let Failure::Answer(why) = &failure {
                tracing::info!(conversation = %id, "memory could not read a chat: {why}");
            }
            sqlx::query(
                "UPDATE memory_reads SET claimed_until = NULL, failed_at = NOW(), attempts = LEAST(attempts + 1, 100)
                 WHERE conversation_id = $1",
            )
            .bind(id)
            .execute(pool)
            .await?;
            match failure {
                Failure::Database(error) => Err(error),
                Failure::Answer(_) => Ok(()),
            }
        }
    }
}

/// One pass over a chat, as its owner.
struct Pass<'a> {
    pool: &'a PgPool,
    knowledge: Option<&'a Knowledge>,
    gateway: &'a Gateway,
    model: &'a str,
    user: &'a Identity,
}

/// A memory the model may change, under the short id it saw.
struct Known {
    short: String,
    id: Uuid,
    content: String,
    pinned: bool,
}

/// The model's answer, read leniently.
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub add: Vec<NewMemory>,
    pub update: Vec<(String, String)>,
    pub forget: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct NewMemory {
    pub text: String,
    pub kind: String,
    pub topics: Vec<(String, String)>,
}

impl Pass<'_> {
    async fn learn(
        &self,
        conversation: Uuid,
        reply: Uuid,
        read_through: i32,
    ) -> Result<(), Failure> {
        let rows: Vec<(i32, String, String)> = sqlx::query_as(
            "SELECT ordinal, role, content FROM messages
             WHERE conversation_id = $1 AND ordinal > $2 AND role IN ('user', 'assistant')
             ORDER BY ordinal",
        )
        .bind(conversation)
        .bind(read_through)
        .fetch_all(self.pool)
        .await?;
        let through = rows.iter().map(|r| r.0).max().unwrap_or(read_through);
        let texts: Vec<String> = rows
            .iter()
            .filter(|r| r.1 == "user")
            .map(|r| r.2.clone())
            .collect();
        let read = latest(&texts);
        let answer: String = rows
            .iter()
            .rev()
            .find(|r| r.1 == "assistant")
            .map(|r| r.2.trim().chars().take(REPLY_CHARS).collect())
            .unwrap_or_default();
        if read.is_empty() {
            self.read_through(conversation, through).await?;
            return Ok(());
        }
        let joined = read.join("\n\n");
        let known = self.known(&joined).await?;
        let topics: Vec<String> = sqlx::query_scalar(
            "SELECT t.name FROM memory_topics t LEFT JOIN memory_links l ON l.topic_id = t.id
             WHERE t.owner_id = $1 GROUP BY t.id ORDER BY COUNT(l.memory_id) DESC, t.name LIMIT $2",
        )
        .bind(self.user.id)
        .bind(KNOWN_TOPICS)
        .fetch_all(self.pool)
        .await?;
        let mut input = String::from("Known memories:\n");
        if known.is_empty() {
            input.push_str("(none yet)\n");
        }
        for k in &known {
            let pin = if k.pinned {
                " (pinned, keep as is)"
            } else {
                ""
            };
            input.push_str(&format!("{}: {}{pin}\n", k.short, k.content));
        }
        input.push_str(&format!(
            "\nTopics in use: {}\n\nThe person's newest messages:\n---\n{}\n---",
            if topics.is_empty() {
                "(none yet)".to_owned()
            } else {
                topics.join(", ")
            },
            read.join("\n---\n")
        ));
        if !answer.is_empty() {
            input.push_str(&format!(
                "\n\nThe assistant's latest reply, for context only (never remember what it says):\n{answer}"
            ));
        }
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": 1200,
            "stream": false,
            "messages": [
                {"role": "system", "content": LEARN_PROMPT},
                {"role": "user", "content": input},
            ],
        });
        let response = self
            .gateway
            .complete(self.user, &body, MODEL_TIMEOUT)
            .await
            .map_err(|e| Failure::Answer(e.1))?;
        let changes = changes(&response).ok_or_else(|| {
            Failure::Answer("the model's answer was not the JSON asked for".into())
        })?;
        let items = self.apply(conversation, &known, changes).await?;
        if !items.is_empty() {
            let entry = json!({"id":"memory-learned","kind":"learned","status":"completed",
                "summary":format!("Memory updated: {}", count(items.len())),"items":items});
            sqlx::query(
                "UPDATE messages SET activity = activity || jsonb_build_array($2::jsonb)
                 WHERE id = $1 AND conversation_id = $3",
            )
            .bind(reply)
            .bind(entry)
            .bind(conversation)
            .execute(self.pool)
            .await?;
        }
        self.read_through(conversation, through).await?;
        Ok(())
    }

    async fn read_through(&self, conversation: Uuid, through: i32) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE memory_reads SET read_through = GREATEST(read_through, $2), claimed_until = NULL,
               attempts = 0, failed_at = NULL WHERE conversation_id = $1",
        )
        .bind(conversation)
        .bind(through)
        .execute(self.pool)
        .await
        .map(|_| ())
    }

    /// The memories the model sees: all of them for a person with few, otherwise the
    /// pinned ones and the best matches for the new messages, under short ids.
    async fn known(&self, text: &str) -> Result<Vec<Known>, sqlx::Error> {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memories WHERE owner_id = $1")
            .bind(self.user.id)
            .fetch_one(self.pool)
            .await?;
        let rows: Vec<(Uuid, String, bool)> = if total <= KNOWN {
            sqlx::query_as(
                "SELECT id, content, pinned FROM memories WHERE owner_id = $1 ORDER BY created_at",
            )
            .bind(self.user.id)
            .fetch_all(self.pool)
            .await?
        } else {
            let mut ids = rank(
                self.pool,
                self.knowledge,
                self.gateway,
                self.user,
                text,
                KNOWN,
                None,
            )
            .await?;
            let pinned: Vec<Uuid> = sqlx::query_scalar(
                "SELECT id FROM memories WHERE owner_id = $1 AND pinned ORDER BY updated_at DESC LIMIT $2",
            )
            .bind(self.user.id)
            .bind(PINNED)
            .fetch_all(self.pool)
            .await?;
            ids.extend(pinned);
            if ids.len() < KNOWN as usize {
                let recent: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM memories WHERE owner_id = $1 AND NOT (id = ANY($2))
                     ORDER BY COALESCE(used_at, created_at) DESC LIMIT $3",
                )
                .bind(self.user.id)
                .bind(&ids)
                .bind(KNOWN - ids.len() as i64)
                .fetch_all(self.pool)
                .await?;
                ids.extend(recent);
            }
            sqlx::query_as(
                "SELECT id, content, pinned FROM memories WHERE id = ANY($1) ORDER BY created_at",
            )
            .bind(&ids)
            .fetch_all(self.pool)
            .await?
        };
        Ok(rows
            .into_iter()
            .enumerate()
            .map(|(i, (id, content, pinned))| Known {
                short: format!("m{}", i + 1),
                id,
                content,
                pinned,
            })
            .collect())
    }

    /// Applies the model's changes; returns what changed, for the reply's card.
    async fn apply(
        &self,
        conversation: Uuid,
        known: &[Known],
        changes: Changes,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let find = |short: &str| known.iter().find(|k| k.short == short.trim() && !k.pinned);
        let mut items = vec![];
        let mut forgotten = vec![];
        for short in changes.forget.iter().take(MAX_CHANGES) {
            if let Some(k) = find(short) {
                let gone = sqlx::query(
                    "DELETE FROM memories WHERE id = $1 AND owner_id = $2 AND NOT pinned",
                )
                .bind(k.id)
                .bind(self.user.id)
                .execute(self.pool)
                .await?;
                if gone.rows_affected() > 0 {
                    forgotten.push(k.id);
                    items.push(json!({"id":k.id,"text":k.content,"change":"forgot"}));
                }
            }
        }
        let mut updates = vec![];
        for (short, text) in changes.update.iter().take(MAX_CHANGES) {
            let Some(text) = clean(text) else { continue };
            if let Some(k) = find(short) {
                if forgotten.contains(&k.id) || same(&k.content, &text) {
                    continue;
                }
                updates.push((k.id, text));
            }
        }
        let vectors = match self.knowledge {
            Some(k) => {
                let texts: Vec<String> = updates.iter().map(|u| u.1.clone()).collect();
                if texts.is_empty() {
                    None
                } else {
                    embed(k, self.gateway, self.user, &texts, false).await
                }
            }
            None => None,
        };
        for (i, (id, text)) in updates.iter().enumerate() {
            let vector = vectors.as_ref().and_then(|v| v.get(i).cloned());
            let done = sqlx::query(
                "UPDATE memories SET content = $3, embedding = $4::halfvec, embedding_attempts = 0, updated_at = NOW()
                 WHERE id = $1 AND owner_id = $2 AND NOT pinned",
            )
            .bind(id)
            .bind(self.user.id)
            .bind(text)
            .bind(vector)
            .execute(self.pool)
            .await?;
            if done.rows_affected() > 0 {
                items.push(json!({"id":id,"text":text,"change":"updated"}));
            }
        }
        let mut adds: Vec<NewMemory> = vec![];
        for add in changes.add.into_iter() {
            let Some(text) = clean(&add.text) else {
                continue;
            };
            if adds.len() >= MAX_ADDS
                || known.iter().any(|k| same(&k.content, &text))
                || adds.iter().any(|a| same(&a.text, &text))
            {
                continue;
            }
            adds.push(NewMemory { text, ..add });
        }
        if adds.is_empty() {
            self.prune_topics().await?;
            return Ok(items);
        }
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memories WHERE owner_id = $1")
            .bind(self.user.id)
            .fetch_one(self.pool)
            .await?;
        let room = (MAX_MEMORIES - total).max(0) as usize;
        adds.truncate(room);
        let vectors = match self.knowledge {
            Some(k) if !adds.is_empty() => {
                let texts: Vec<String> = adds.iter().map(|a| a.text.clone()).collect();
                embed(k, self.gateway, self.user, &texts, false).await
            }
            _ => None,
        };
        let source_message: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM messages WHERE conversation_id = $1 AND role = 'user' ORDER BY ordinal DESC LIMIT 1",
        )
        .bind(conversation)
        .fetch_optional(self.pool)
        .await?;
        for (i, add) in adds.iter().enumerate() {
            let vector = vectors.as_ref().and_then(|v| v.get(i).cloned());
            // A memory that says the same as one kept already, by meaning.
            if let Some(vector) = &vector {
                let repeat: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM memories WHERE owner_id = $1 AND embedding IS NOT NULL
                     AND embedding <=> $2::halfvec < $3 LIMIT 1",
                )
                .bind(self.user.id)
                .bind(vector)
                .bind(SAME_DISTANCE)
                .fetch_optional(self.pool)
                .await?;
                if repeat.is_some() {
                    continue;
                }
            }
            let id = Uuid::new_v4();
            let kind = if KINDS.contains(&add.kind.as_str()) {
                add.kind.as_str()
            } else {
                "about"
            };
            sqlx::query(
                "INSERT INTO memories (id, owner_id, kind, content, origin, source_conversation_id, source_message_id, embedding)
                 VALUES ($1, $2, $3, $4, 'learned', $5, $6, $7::halfvec)",
            )
            .bind(id)
            .bind(self.user.id)
            .bind(kind)
            .bind(&add.text)
            .bind(conversation)
            .bind(source_message)
            .bind(vector)
            .execute(self.pool)
            .await?;
            link(self.pool, self.user.id, id, &add.topics).await?;
            items.push(json!({"id":id,"text":add.text,"change":"added"}));
        }
        self.prune_topics().await?;
        Ok(items)
    }

    async fn prune_topics(&self) -> Result<(), sqlx::Error> {
        prune_topics(self.pool, self.user.id).await
    }
}

/// Topics no memory is about any more.
async fn prune_topics(pool: &PgPool, owner: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM memory_topics t WHERE t.owner_id = $1
         AND NOT EXISTS (SELECT 1 FROM memory_links l WHERE l.topic_id = t.id)",
    )
    .bind(owner)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Links a memory to its topics, making the topics the person does not have yet.
async fn link(
    pool: &PgPool,
    owner: Uuid,
    memory: Uuid,
    topics: &[(String, String)],
) -> Result<(), sqlx::Error> {
    for (name, kind) in topics.iter().take(3) {
        let Some(name) = topic_name(name) else {
            continue;
        };
        let kind = if TOPIC_KINDS.contains(&kind.as_str()) {
            kind.as_str()
        } else {
            "topic"
        };
        sqlx::query(
            "INSERT INTO memory_topics (id, owner_id, name, kind) VALUES ($1, $2, $3, $4)
             ON CONFLICT (owner_id, lower(name)) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(owner)
        .bind(&name)
        .bind(kind)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO memory_links (memory_id, topic_id)
             SELECT $1, id FROM memory_topics WHERE owner_id = $2 AND lower(name) = lower($3)
             ON CONFLICT DO NOTHING",
        )
        .bind(memory)
        .bind(owner)
        .bind(&name)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// One line of at most MAX_CONTENT characters, or None when empty.
fn clean(text: &str) -> Option<String> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(MAX_CONTENT).collect();
    (!text.is_empty()).then_some(text)
}

fn topic_name(name: &str) -> Option<String> {
    let name: String = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c == '#' || c == '"' || c == '.')
        .chars()
        .take(MAX_TOPIC)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Whether two memories say the same, ignoring case and punctuation.
fn same(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    norm(a) == norm(b)
}

/// The person's newest messages as the model reads them: as many of the latest as fit,
/// in order, each one and all together cut short.
fn latest(texts: &[String]) -> Vec<String> {
    let mut left = READ_CHARS;
    let mut out = vec![];
    for text in texts
        .iter()
        .rev()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
    {
        if left == 0 {
            break;
        }
        let part: String = text.chars().take(MESSAGE_CHARS.min(left)).collect();
        left -= part.chars().count();
        out.push(part);
    }
    out.reverse();
    out
}

/// The JSON object in a model's answer, after any thinking.
fn answer_json(response: &Value) -> Option<Value> {
    let text = response["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    let text = match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    };
    let (open, close) = (text.find('{')?, text.rfind('}')?);
    if close < open {
        return None;
    }
    serde_json::from_str(&text[open..=close]).ok()
}

fn topics_of(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .map(|topics| {
            topics
                .iter()
                .filter_map(|t| match t {
                    Value::String(name) => Some((name.clone(), "topic".into())),
                    Value::Object(_) => Some((
                        t["name"].as_str()?.to_owned(),
                        t["kind"].as_str().unwrap_or("topic").to_owned(),
                    )),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The changes in a learning answer; None when it holds no JSON object.
pub fn changes(response: &Value) -> Option<Changes> {
    let value = answer_json(response)?;
    let list = |key: &str| value[key].as_array().cloned().unwrap_or_default();
    Some(Changes {
        add: list("add")
            .iter()
            .filter_map(|a| {
                Some(NewMemory {
                    text: a["text"].as_str()?.to_owned(),
                    kind: a["kind"].as_str().unwrap_or("about").to_owned(),
                    topics: topics_of(&a["topics"]),
                })
            })
            .collect(),
        update: list("update")
            .iter()
            .filter_map(|u| Some((u["id"].as_str()?.to_owned(), u["text"].as_str()?.to_owned())))
            .collect(),
        forget: list("forget")
            .iter()
            .filter_map(|f| match f {
                Value::String(id) => Some(id.clone()),
                Value::Object(_) => f["id"].as_str().map(str::to_owned),
                _ => None,
            })
            .collect(),
    })
}

/// What the model fills in for a memory the person added.
struct Tagging {
    kind: bool,
    topics: bool,
}

/// Files a memory the person added: its kind and topics, from the model.
async fn tag(
    pool: PgPool,
    gateway: Gateway,
    model: String,
    user: Identity,
    memory: Uuid,
    content: String,
    fill: Tagging,
) {
    let topics: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM memory_topics t LEFT JOIN memory_links l ON l.topic_id = t.id
         WHERE t.owner_id = $1 GROUP BY t.id ORDER BY COUNT(l.memory_id) DESC, t.name LIMIT $2",
    )
    .bind(user.id)
    .bind(KNOWN_TOPICS)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();
    let body = json!({
        "model": model,
        "temperature": 0,
        "max_tokens": 300,
        "stream": false,
        "messages": [
            {"role": "system", "content": TAG_PROMPT},
            {"role": "user", "content": format!("Topics in use: {}\n\nMemory: {content}",
                if topics.is_empty() { "(none yet)".to_owned() } else { topics.join(", ") })},
        ],
    });
    let Ok(response) = gateway.complete(&user, &body, MODEL_TIMEOUT).await else {
        return;
    };
    let Some(value) = answer_json(&response) else {
        return;
    };
    if fill.kind {
        if let Some(kind) = value["kind"].as_str().filter(|k| KINDS.contains(k)) {
            let _ = sqlx::query("UPDATE memories SET kind = $3 WHERE id = $1 AND owner_id = $2")
                .bind(memory)
                .bind(user.id)
                .bind(kind)
                .execute(&pool)
                .await;
        }
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM memories WHERE id = $1)")
        .bind(memory)
        .fetch_one(&pool)
        .await
        .unwrap_or(false);
    if exists && fill.topics {
        if let Err(error) = link(&pool, user.id, memory, &topics_of(&value["topics"])).await {
            tracing::warn!("memory topics: {error}");
        }
    }
}

// ---------------------------------------------------------------------------------
// The worker: embeddings for memories saved while the embedding model was busy, and
// after the embedding settings change.

pub async fn worker(s: AppState, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            _ = shutdown.cancelled() => break,
        }
        let (Some(gateway), Some(knowledge)) = (&s.gateway, &s.knowledge) else {
            continue;
        };
        if let Err(error) = embed_missing(&s.store.pool, knowledge, gateway).await {
            tracing::warn!("memory embeddings: {error}");
        }
    }
}

/// Embeds memories that have no embedding, a few at a time, as their owners.
pub async fn embed_missing(
    pool: &PgPool,
    knowledge: &Knowledge,
    gateway: &Gateway,
) -> Result<usize, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, String, i16)> = sqlx::query_as(
        "UPDATE memories SET embedding_attempts = embedding_attempts + 1 WHERE id IN (
            SELECT id FROM memories WHERE embedding IS NULL AND embedding_attempts < 3
            ORDER BY created_at LIMIT 16 FOR UPDATE SKIP LOCKED)
         RETURNING id, owner_id, content, embedding_attempts",
    )
    .fetch_all(pool)
    .await?;
    let mut done = 0;
    for (id, owner, content, attempts) in rows {
        let Some(user) = identity(pool, owner).await? else {
            continue;
        };
        if let Some(vector) = embed(knowledge, gateway, &user, &[content], false)
            .await
            .and_then(|mut v| v.pop())
        {
            // Not if the memory was changed meanwhile.
            sqlx::query("UPDATE memories SET embedding = $2::halfvec WHERE id = $1 AND embedding IS NULL AND embedding_attempts = $3")
                .bind(id)
                .bind(vector)
                .bind(attempts)
                .execute(pool)
                .await?;
            done += 1;
        }
    }
    Ok(done)
}

async fn identity(pool: &PgPool, user: Uuid) -> Result<Option<Identity>, sqlx::Error> {
    let row: Option<(Uuid, String, String, Role)> = sqlx::query_as(
        "SELECT id, username, display_name, role FROM users WHERE id = $1 AND enabled",
    )
    .bind(user)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, username, display_name, role)| Identity {
        id,
        username,
        display_name,
        role,
        session: None,
    }))
}

// ---------------------------------------------------------------------------------
// The API.

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/memory", get(read).post(create).delete(forget_all))
        .route("/api/memory/settings", put(choose))
        .route(
            "/api/memory/topics/{id}",
            patch(rename_topic).delete(remove_topic),
        )
        .route("/api/memory/{id}", patch(update).delete(remove))
        .route("/api/admin/memory", get(admin_read).put(admin_save))
        .with_state(state)
}

fn off() -> ApiError {
    ApiError(StatusCode::CONFLICT, "Memory is off on this server".into())
}

async fn read(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    let settings = settings(pool).await?;
    let available = settings.enabled && settings.model.is_some() && s.gateway.is_some();
    let chosen = choices(pool, user.id, settings.default_on).await?;
    let memories = sqlx::query(
        "SELECT m.id, m.kind, m.content, m.origin, m.pinned, m.uses, m.used_at, m.created_at, m.updated_at,
           m.source_conversation_id, c.title AS source_title,
           COALESCE((SELECT array_agg(l.topic_id) FROM memory_links l WHERE l.memory_id = m.id), '{}') AS topics
         FROM memories m LEFT JOIN conversations c ON c.id = m.source_conversation_id AND c.owner_id = m.owner_id
         WHERE m.owner_id = $1 ORDER BY m.created_at DESC",
    )
    .bind(user.id)
    .fetch_all(pool)
    .await?;
    let topics = sqlx::query(
        "SELECT t.id, t.name, t.kind, COUNT(l.memory_id) AS count FROM memory_topics t
         LEFT JOIN memory_links l ON l.topic_id = t.id WHERE t.owner_id = $1
         GROUP BY t.id ORDER BY count DESC, t.name",
    )
    .bind(user.id)
    .fetch_all(pool)
    .await?;
    Ok(Json(json!({
        "available": available,
        "useMemory": chosen.use_memory,
        "learn": chosen.learn,
        "max": MAX_MEMORIES,
        "memories": memories.iter().map(memory_json).collect::<Vec<_>>(),
        "topics": topics.iter().map(|r| json!({
            "id": r.get::<Uuid, _>("id"),
            "name": r.get::<String, _>("name"),
            "kind": r.get::<String, _>("kind"),
            "count": r.get::<i64, _>("count"),
        })).collect::<Vec<_>>(),
    })))
}

fn memory_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({
        "id": r.get::<Uuid, _>("id"),
        "kind": r.get::<String, _>("kind"),
        "content": r.get::<String, _>("content"),
        "origin": r.get::<String, _>("origin"),
        "pinned": r.get::<bool, _>("pinned"),
        "uses": r.get::<i32, _>("uses"),
        "usedAt": r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("used_at"),
        "createdAt": r.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
        "updatedAt": r.get::<chrono::DateTime<chrono::Utc>, _>("updated_at"),
        "source": r.get::<Option<Uuid>, _>("source_conversation_id").map(|id| json!({
            "conversationId": id,
            "title": r.get::<Option<String>, _>("source_title"),
        })),
        "topics": r.get::<Vec<Uuid>, _>("topics"),
    })
}

async fn one(pool: &PgPool, owner: Uuid, id: Uuid) -> Result<Value, ApiError> {
    let row = sqlx::query(
        "SELECT m.id, m.kind, m.content, m.origin, m.pinned, m.uses, m.used_at, m.created_at, m.updated_at,
           m.source_conversation_id, c.title AS source_title,
           COALESCE((SELECT array_agg(l.topic_id) FROM memory_links l WHERE l.memory_id = m.id), '{}') AS topics
         FROM memories m LEFT JOIN conversations c ON c.id = m.source_conversation_id AND c.owner_id = m.owner_id
         WHERE m.id = $1 AND m.owner_id = $2",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)?;
    Ok(memory_json(&row))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Choose {
    use_memory: bool,
    learn: bool,
}

async fn choose(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Choose>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    sqlx::query(
        "INSERT INTO memory_choices (user_id, use_memory, learn) VALUES ($1, $2, $3)
         ON CONFLICT (user_id) DO UPDATE SET use_memory = $2, learn = $3, updated_at = NOW()",
    )
    .bind(user.id)
    .bind(body.use_memory)
    .bind(body.learn)
    .execute(&s.store.pool)
    .await?;
    Ok(Json(
        json!({"useMemory": body.use_memory, "learn": body.learn}),
    ))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Input {
    content: Option<String>,
    kind: Option<String>,
    pinned: Option<bool>,
    /// Topic names; absent keeps the topics, and on a new memory lets the model choose.
    topics: Option<Vec<String>>,
}

fn kind_of(kind: Option<&str>) -> Result<Option<String>, ApiError> {
    match kind {
        None => Ok(None),
        Some(k) if KINDS.contains(&k) => Ok(Some(k.to_owned())),
        Some(_) => Err(ApiError::bad(
            "Kind must be about, work, preference or goal",
        )),
    }
}

/// Topic names with the kind of an existing topic of that name, or "topic".
async fn named_topics(
    pool: &PgPool,
    owner: Uuid,
    names: &[String],
) -> Result<Vec<(String, String)>, ApiError> {
    if names.len() > 3 {
        return Err(ApiError::bad("A memory is about three topics at most"));
    }
    let mut out = vec![];
    for name in names {
        let Some(name) = topic_name(name) else {
            continue;
        };
        let kind: Option<String> = sqlx::query_scalar(
            "SELECT kind FROM memory_topics WHERE owner_id = $1 AND lower(name) = lower($2)",
        )
        .bind(owner)
        .bind(&name)
        .fetch_optional(pool)
        .await?;
        out.push((name, kind.unwrap_or_else(|| "topic".into())));
    }
    Ok(out)
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    let settings = settings(pool).await?;
    if !settings.enabled {
        return Err(off());
    }
    let content = body
        .content
        .as_deref()
        .and_then(clean)
        .ok_or_else(|| ApiError::bad("Write what Chat should remember"))?;
    let kind = kind_of(body.kind.as_deref())?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memories WHERE owner_id = $1")
        .bind(user.id)
        .fetch_one(pool)
        .await?;
    if total >= MAX_MEMORIES {
        return Err(ApiError::bad(format!(
            "Memory is full ({MAX_MEMORIES}). Delete some memories first."
        )));
    }
    let topics = match &body.topics {
        Some(names) => Some(named_topics(pool, user.id, names).await?),
        None => None,
    };
    let vector = match (&s.knowledge, &s.gateway) {
        (Some(k), Some(g)) => embed(k, g, &user, std::slice::from_ref(&content), false)
            .await
            .and_then(|mut v| v.pop()),
        _ => None,
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO memories (id, owner_id, kind, content, origin, pinned, embedding)
         VALUES ($1, $2, $3, $4, 'added', $5, $6::halfvec)",
    )
    .bind(id)
    .bind(user.id)
    .bind(kind.as_deref().unwrap_or("about"))
    .bind(&content)
    .bind(body.pinned.unwrap_or(false))
    .bind(vector)
    .execute(pool)
    .await?;
    if let Some(topics) = &topics {
        link(pool, user.id, id, topics).await?;
    }
    let fill = Tagging {
        kind: kind.is_none(),
        topics: topics.as_ref().is_none_or(|t| t.is_empty()),
    };
    if fill.kind || fill.topics {
        if let (Some(gateway), Some(model)) = (s.gateway.clone(), settings.model.clone()) {
            tokio::spawn(tag(
                pool.clone(),
                gateway,
                model,
                user.clone(),
                id,
                content,
                fill,
            ));
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({"memory": one(pool, user.id, id).await?})),
    ))
}

async fn update(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Input>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    let kind = kind_of(body.kind.as_deref())?;
    let content = match &body.content {
        Some(text) => Some(clean(text).ok_or_else(|| ApiError::bad("A memory can't be empty"))?),
        None => None,
    };
    let current: Option<String> =
        sqlx::query_scalar("SELECT content FROM memories WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(user.id)
            .fetch_optional(pool)
            .await?;
    let current = current.ok_or_else(ApiError::not_found)?;
    let changed = content.filter(|c| *c != current);
    let vector = match (&changed, &s.knowledge, &s.gateway) {
        (Some(text), Some(k), Some(g)) => embed(k, g, &user, std::slice::from_ref(text), false)
            .await
            .and_then(|mut v| v.pop()),
        _ => None,
    };
    sqlx::query(
        "UPDATE memories SET
           content = COALESCE($3, content),
           kind = COALESCE($4, kind),
           pinned = COALESCE($5, pinned),
           embedding = CASE WHEN $3 IS NULL THEN embedding ELSE $6::halfvec END,
           embedding_attempts = CASE WHEN $3 IS NULL THEN embedding_attempts ELSE 0 END,
           updated_at = NOW()
         WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .bind(changed)
    .bind(kind)
    .bind(body.pinned)
    .bind(vector)
    .execute(pool)
    .await?;
    if let Some(names) = &body.topics {
        let topics = named_topics(pool, user.id, names).await?;
        sqlx::query("DELETE FROM memory_links WHERE memory_id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        link(pool, user.id, id, &topics).await?;
        prune_topics(pool, user.id).await?;
    }
    Ok(Json(json!({"memory": one(pool, user.id, id).await?})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let done = sqlx::query("DELETE FROM memories WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&s.store.pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    prune_topics(&s.store.pool, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn forget_all(State(s): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let mut tx = s.store.pool.begin().await?;
    sqlx::query("DELETE FROM memories WHERE owner_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM memory_topics WHERE owner_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TopicRemoval {
    /// Also delete the memories about the topic.
    memories: Option<bool>,
}

async fn remove_topic(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<TopicRemoval>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let mut tx = s.store.pool.begin().await?;
    let mine: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM memory_topics WHERE id = $1 AND owner_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if !mine {
        return Err(ApiError::not_found());
    }
    let forgotten = if q.memories.unwrap_or(false) {
        sqlx::query(
            "DELETE FROM memories WHERE owner_id = $1
             AND id IN (SELECT memory_id FROM memory_links WHERE topic_id = $2)",
        )
        .bind(user.id)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
    } else {
        0
    };
    sqlx::query("DELETE FROM memory_topics WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    prune_topics(&s.store.pool, user.id).await?;
    Ok(Json(json!({"forgotten": forgotten})))
}

#[derive(Deserialize)]
struct TopicPatch {
    name: Option<String>,
    kind: Option<String>,
}

/// Renames a topic, or changes its kind. A new name that another topic has already
/// merges the two.
async fn rename_topic(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<TopicPatch>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let kind = match body.kind.as_deref() {
        None => None,
        Some(k) if TOPIC_KINDS.contains(&k) => Some(k.to_owned()),
        Some(_) => {
            return Err(ApiError::bad(
                "Kind must be project, tool, field, course or topic",
            ))
        }
    };
    let name = match &body.name {
        Some(name) => Some(topic_name(name).ok_or_else(|| ApiError::bad("Give the topic a name"))?),
        None => None,
    };
    let mut tx = s.store.pool.begin().await?;
    let mine: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM memory_topics WHERE id = $1 AND owner_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if !mine {
        return Err(ApiError::not_found());
    }
    let mut kept = id;
    if let Some(name) = &name {
        let other: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM memory_topics WHERE owner_id = $1 AND lower(name) = lower($2) AND id <> $3",
        )
        .bind(user.id)
        .bind(name)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        match other {
            Some(other) => {
                sqlx::query(
                    "INSERT INTO memory_links (memory_id, topic_id)
                     SELECT memory_id, $2 FROM memory_links WHERE topic_id = $1 ON CONFLICT DO NOTHING",
                )
                .bind(id)
                .bind(other)
                .execute(&mut *tx)
                .await?;
                sqlx::query("DELETE FROM memory_topics WHERE id = $1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                kept = other;
            }
            None => {
                sqlx::query("UPDATE memory_topics SET name = $2 WHERE id = $1")
                    .bind(id)
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    if let Some(kind) = &kind {
        sqlx::query("UPDATE memory_topics SET kind = $2 WHERE id = $1")
            .bind(kept)
            .bind(kind)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"id": kept})))
}

async fn admin_read(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    let settings = settings(&s.store.pool).await?;
    let (people, memories): (i64, i64) =
        sqlx::query_as("SELECT COUNT(DISTINCT owner_id), COUNT(*) FROM memories")
            .fetch_one(&s.store.pool)
            .await?;
    Ok(Json(admin_json(&settings, people, memories, &s)))
}

fn admin_json(settings: &Settings, people: i64, memories: i64, s: &AppState) -> Value {
    json!({
        "enabled": settings.enabled,
        "defaultOn": settings.default_on,
        "model": settings.model,
        "people": people,
        "memories": memories,
        "gateway": s.gateway.is_some(),
        "embeddings": s.knowledge.is_some(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminPatch {
    enabled: bool,
    default_on: bool,
    model: Option<String>,
}

async fn admin_save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AdminPatch>,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, true).await?;
    let model = body
        .model
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    if model.as_deref().is_some_and(|m| !valid_model(m)) {
        return Err(ApiError::bad("Enter a model name of up to 160 characters"));
    }
    if body.enabled && model.is_none() {
        return Err(ApiError::bad("Choose the model that keeps memories"));
    }
    if body.enabled && s.gateway.is_none() {
        return Err(ApiError::bad(
            "Memory asks models through the gateway; connect one first",
        ));
    }
    let saved: Settings = sqlx::query_as(
        "UPDATE memory_settings SET enabled = $1, default_on = $2, model = $3, updated_at = NOW()
         WHERE id RETURNING enabled, default_on, model",
    )
    .bind(body.enabled)
    .bind(body.default_on)
    .bind(model)
    .fetch_one(&s.store.pool)
    .await?;
    let (people, memories): (i64, i64) =
        sqlx::query_as("SELECT COUNT(DISTINCT owner_id), COUNT(*) FROM memories")
            .fetch_one(&s.store.pool)
            .await?;
    Ok(Json(admin_json(&saved, people, memories, &s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(content: &str) -> Value {
        json!({"choices":[{"message":{"content":content}}]})
    }

    #[test]
    fn changes_are_read_after_thinking_and_leniently() {
        let got = changes(&answer(
            "<think>they said they teach</think>Sure: {\"add\":[{\"text\":\"You teach CHM 501\",\"kind\":\"work\",\"topics\":[{\"name\":\"CHM 501\",\"kind\":\"course\"},\"chemistry\"]}],\"update\":[{\"id\":\"m2\",\"text\":\"You use Julia\"}],\"forget\":[\"m3\",{\"id\":\"m4\"}]}",
        ))
        .unwrap();
        assert_eq!(got.add.len(), 1);
        assert_eq!(got.add[0].kind, "work");
        assert_eq!(
            got.add[0].topics,
            vec![
                ("CHM 501".to_owned(), "course".to_owned()),
                ("chemistry".to_owned(), "topic".to_owned())
            ]
        );
        assert_eq!(
            got.update,
            vec![("m2".to_owned(), "You use Julia".to_owned())]
        );
        assert_eq!(got.forget, vec!["m3".to_owned(), "m4".to_owned()]);
        assert!(changes(&answer("Nothing to remember.")).is_none());
        assert_eq!(changes(&answer("{}")).unwrap(), Changes::default());
    }

    #[test]
    fn the_same_memory_ignores_case_and_punctuation() {
        assert!(same("You use PyTorch.", "you use pytorch"));
        assert!(!same("You use PyTorch", "You use JAX"));
    }

    #[test]
    fn newest_messages_fit_the_budget_in_order() {
        let texts: Vec<String> = (0..10)
            .map(|i| format!("{i}{}", "x".repeat(1500)))
            .collect();
        let read = latest(&texts);
        assert_eq!(
            read.iter().map(|t| t.chars().count()).sum::<usize>(),
            READ_CHARS
        );
        assert!(read.last().unwrap().starts_with('9'));
        assert!(read.first().unwrap().starts_with('6'));
    }

    #[test]
    fn topic_names_are_trimmed() {
        assert_eq!(topic_name("  #Kimi   port. "), Some("Kimi port".into()));
        assert_eq!(topic_name("  "), None);
    }

    #[test]
    fn short_follow_ups_match_with_the_message_before() {
        let messages = vec![
            (
                "user".to_owned(),
                "Help me tune the Gaudi kernel for MXFP4".to_owned(),
            ),
            ("assistant".to_owned(), "Sure.".to_owned()),
            ("user".to_owned(), "and faster?".to_owned()),
        ];
        assert_eq!(
            question(&messages),
            "Help me tune the Gaudi kernel for MXFP4\nand faster?"
        );
    }
}
