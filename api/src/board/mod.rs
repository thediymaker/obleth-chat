//! A project's board: cards that the people in the project and the assistant make
//! and work through. A card starts as an idea, waits in Up next, is worked on (Now),
//! may stop to ask the person something (Needs you) and ends Done. Cards belong to
//! the project, so whoever may use the project may see and change them; group
//! members trust each other, and no card has permissions of its own.
//!
//! Every change to a board takes the project's row lock first, so two people moving
//! or reordering cards at once take turns and a column's order stays whole.

pub mod tools;

use crate::paging::PageQuery;
use crate::projects::VISIBLE;
use crate::{error::ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

pub const STATUSES: [&str; 7] = [
    "suggested",
    "idea",
    "next",
    "doing",
    "waiting",
    "done",
    "dismissed",
];
/// The columns a board shows in full; done cards come a few at a time.
const OPEN: [&str; 5] = ["suggested", "idea", "next", "doing", "waiting"];
pub const MAX_CARDS: i64 = 1000;
pub const MAX_STEPS: usize = 12;
pub const MAX_TITLE: usize = 200;
pub const MAX_NOTES: usize = 4000;
pub const MAX_KIND: usize = 40;
pub const MAX_STEP: usize = 200;
pub const MAX_PROGRESS: usize = 200;
pub const MAX_QUESTION: usize = 500;
pub const MAX_OPTIONS: usize = 4;
pub const MAX_OPTION: usize = 80;
pub const MAX_ANSWER: usize = 500;
pub const MAX_LINKS: usize = 20;
const MAX_LINK_KIND: usize = 40;
const MAX_LINK_LABEL: usize = 200;
const MAX_LINK_REF: usize = 500;
/// History kept per card.
const KEPT_EVENTS: i64 = 50;
/// Done cards the board shows; the rest come with `?status=done`.
pub const DONE_SHOWN: i64 = 5;

/// One step of a card's work.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub text: String,
    #[serde(default = "todo")]
    pub state: String,
}
fn todo() -> String {
    "todo".into()
}

/// Something a card touched: a job, a run, a transfer, a section of a paper.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub kind: String,
    pub label: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Who changes a board: a person, or the assistant in a chat of that person's.
#[derive(Clone, Copy, Debug)]
pub struct Actor {
    pub user: Uuid,
    pub assistant: bool,
}
impl Actor {
    pub fn person(user: Uuid) -> Self {
        Self {
            user,
            assistant: false,
        }
    }
    pub fn assistant(user: Uuid) -> Self {
        Self {
            user,
            assistant: true,
        }
    }
    fn kind(&self) -> &'static str {
        if self.assistant {
            "assistant"
        } else {
            "person"
        }
    }
}

/// A new card.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct NewCard {
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub kind: String,
    /// idea (the default) or next; the assistant may also suggest.
    pub status: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// The chat it came from.
    #[serde(skip)]
    pub source_chat: Option<Uuid>,
}

/// A change to a card. Each field is left as it is when absent.
#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub kind: Option<String>,
    /// The column to move it to.
    pub status: Option<String>,
    /// Its place in the column (its new one when `status` moves it), from 0.
    pub position: Option<i64>,
    pub steps: Option<Vec<Step>>,
    pub progress: Option<String>,
    /// "me" puts the person on it, "assistant" the assistant of its chat, "none" nobody.
    pub worker: Option<String>,
    /// The answer to the card's question; it goes back to Now with a chat, else Up next.
    pub answer: Option<String>,
    /// Moves it to Needs you with this question, and the options to answer with.
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    /// The person's chat that works the card now; the assistant is put on it.
    pub chat_id: Option<Uuid>,
    pub links: Option<Vec<Link>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/projects/{id}/cards", get(list).post(create))
        .route("/api/projects/{id}/cards/order", put(reorder))
        .route(
            "/api/projects/{id}/cards/{card}",
            get(detail).patch(update).delete(remove),
        )
        .with_state(state)
}

/// A card as the board shows it to person $1.
#[derive(FromRow, Debug)]
pub struct Card {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub notes: String,
    pub kind: String,
    pub status: String,
    pub position: i32,
    pub made_by: String,
    pub created_by: Option<Uuid>,
    pub maker_name: Option<String>,
    pub worker: Option<String>,
    pub worker_id: Option<Uuid>,
    pub worker_name: Option<String>,
    pub chat_id: Option<Uuid>,
    pub chat_mine: Option<bool>,
    pub chat_title: Option<String>,
    pub chat_working: bool,
    pub chat_waiting: bool,
    pub source_chat_id: Option<Uuid>,
    pub source_mine: Option<bool>,
    pub source_title: Option<String>,
    pub steps: Value,
    pub progress: String,
    pub question: String,
    pub options: Value,
    pub answer: String,
    pub answered_at: Option<DateTime<Utc>>,
    pub links: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub done_at: Option<DateTime<Utc>>,
}

/// The columns of a `Card` for person $1, read `FROM project_cards c`. A chat's title
/// shows only to its owner; whether it is writing a reply shows to everyone.
const CARD: &str = "SELECT c.id, c.project_id, c.title, c.notes, c.kind, c.status, c.position, c.made_by,
        c.created_by, COALESCE(NULLIF(mu.display_name, ''), mu.username) AS maker_name,
        c.worker, c.worker_id, COALESCE(NULLIF(wu.display_name, ''), wu.username) AS worker_name,
        c.chat_id, v.owner_id = $1 AS chat_mine, CASE WHEN v.owner_id = $1 THEN v.title END AS chat_title,
        COALESCE(v.active_generation IS NOT NULL AND v.generation_expires > clock_timestamp(), FALSE) AS chat_working,
        COALESCE(v.active_generation IS NOT NULL AND v.generation_expires > clock_timestamp() AND EXISTS(
            SELECT 1 FROM tool_approvals a WHERE a.generation_id = v.active_generation AND a.decision IS NULL), FALSE) AS chat_waiting,
        c.source_chat_id, s.owner_id = $1 AS source_mine, CASE WHEN s.owner_id = $1 THEN s.title END AS source_title,
        c.steps, c.progress, c.question, c.options, c.answer, c.answered_at, c.links,
        c.created_at, c.updated_at, c.started_at, c.done_at
    FROM project_cards c
    LEFT JOIN users mu ON mu.id = c.created_by
    LEFT JOIN users wu ON wu.id = c.worker_id
    LEFT JOIN conversations v ON v.id = c.chat_id
    LEFT JOIN conversations s ON s.id = c.source_chat_id";

impl Card {
    pub fn json(&self) -> Value {
        let person = |id: Option<Uuid>, name: &Option<String>| {
            id.map(|id| json!({"id": id, "name": name.clone().unwrap_or_default()}))
        };
        json!({
            "id": self.id,
            "projectId": self.project_id,
            "title": self.title,
            "notes": self.notes,
            "kind": self.kind,
            "status": self.status,
            "position": self.position,
            "madeBy": self.made_by,
            "maker": person(self.created_by, &self.maker_name),
            "worker": self.worker,
            "workerUser": person(self.worker_id, &self.worker_name),
            "chat": self.chat_id.map(|id| json!({
                "id": id,
                "mine": self.chat_mine.unwrap_or(false),
                "title": self.chat_title,
                "working": self.chat_working,
                "waiting": self.chat_waiting,
            })),
            "sourceChat": self.source_chat_id.map(|id| json!({
                "id": id,
                "mine": self.source_mine.unwrap_or(false),
                "title": self.source_title,
            })),
            "steps": self.steps,
            "progress": self.progress,
            "question": self.question,
            "options": self.options,
            "answer": self.answer,
            "answeredAt": self.answered_at,
            "links": self.links,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
            "startedAt": self.started_at,
            "doneAt": self.done_at,
        })
    }

    pub fn steps(&self) -> Vec<Step> {
        serde_json::from_value(self.steps.clone()).unwrap_or_default()
    }

    pub fn options(&self) -> Vec<String> {
        serde_json::from_value(self.options.clone()).unwrap_or_default()
    }
}

/// One line of a card's history.
#[derive(FromRow)]
struct Event {
    action: String,
    actor: String,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    from_status: Option<String>,
    to_status: Option<String>,
    detail: String,
    created_at: DateTime<Utc>,
}

/// Whether person `user` may use the project; not found when not.
pub async fn visible(pool: &PgPool, user: Uuid, project: Uuid) -> Result<(), ApiError> {
    let found: Option<Uuid> = sqlx::query_scalar(&format!(
        "SELECT p.id FROM projects p WHERE p.id = $2 AND {VISIBLE}"
    ))
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?;
    found.map(|_| ()).ok_or_else(ApiError::not_found)
}

/// Takes the project's row lock for a change to its board, after checking person
/// `user` may use it. Every board write takes it first, so writes to one board take
/// turns. FOR NO KEY UPDATE leaves rows that only point at the project free to change.
async fn lock(tx: &mut PgConnection, user: Uuid, project: Uuid) -> Result<(), ApiError> {
    let found: Option<Uuid> = sqlx::query_scalar(&format!(
        "SELECT p.id FROM projects p WHERE p.id = $2 AND {VISIBLE} FOR NO KEY UPDATE OF p"
    ))
    .bind(user)
    .bind(project)
    .fetch_optional(&mut *tx)
    .await?;
    found.map(|_| ()).ok_or_else(ApiError::not_found)
}

/// One card as person `user` sees it.
pub async fn card(pool: &PgPool, user: Uuid, project: Uuid, id: Uuid) -> Result<Card, ApiError> {
    sqlx::query_as(&format!("{CARD} WHERE c.id = $2 AND c.project_id = $3"))
        .bind(user)
        .bind(id)
        .bind(project)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// The board: every card in the open columns in order, and the latest done ones.
pub async fn board(pool: &PgPool, user: Uuid, project: Uuid) -> Result<(Vec<Card>, i64), ApiError> {
    let mut cards: Vec<Card> = sqlx::query_as(&format!(
        "{CARD} WHERE c.project_id = $2 AND c.status = ANY($3) ORDER BY c.status, c.position, c.created_at, c.id"
    ))
    .bind(user)
    .bind(project)
    .bind(OPEN)
    .fetch_all(pool)
    .await?;
    let done: Vec<Card> = sqlx::query_as(&format!(
        "{CARD} WHERE c.project_id = $2 AND c.status = 'done' ORDER BY c.done_at DESC NULLS LAST, c.id LIMIT $3"
    ))
    .bind(user)
    .bind(project)
    .bind(DONE_SHOWN)
    .fetch_all(pool)
    .await?;
    let done_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM project_cards WHERE project_id = $1 AND status = 'done'",
    )
    .bind(project)
    .fetch_one(pool)
    .await?;
    cards.extend(done);
    Ok((cards, done_count))
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn clean_title(title: &str) -> Result<String, ApiError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ApiError::bad("Give the card a title"));
    }
    if chars(title) > MAX_TITLE {
        return Err(ApiError::bad("Titles are up to 200 characters"));
    }
    Ok(title.to_owned())
}

fn clean_text(text: &str, max: usize, what: &str) -> Result<String, ApiError> {
    let text = text.trim();
    if chars(text) > max {
        return Err(ApiError::bad(format!(
            "{what} {} up to {} characters",
            if what.ends_with('s') { "are" } else { "is" },
            group(max)
        )));
    }
    Ok(text.to_owned())
}

/// 4000 as "4,000".
fn group(n: usize) -> String {
    if n >= 1000 {
        format!("{},{:03}", n / 1000, n % 1000)
    } else {
        n.to_string()
    }
}

fn clean_status(status: &str) -> Result<&str, ApiError> {
    STATUSES
        .iter()
        .find(|s| **s == status)
        .copied()
        .ok_or_else(|| ApiError::bad("Unknown column"))
}

pub fn clean_steps(steps: &[Step]) -> Result<Value, ApiError> {
    if steps.len() > MAX_STEPS {
        return Err(ApiError::bad("A card has up to 12 steps"));
    }
    let mut out = vec![];
    for step in steps {
        let text = step.text.trim();
        if text.is_empty() {
            return Err(ApiError::bad("A step needs some text"));
        }
        if chars(text) > MAX_STEP {
            return Err(ApiError::bad("Steps are up to 200 characters"));
        }
        if !["todo", "now", "done"].contains(&step.state.as_str()) {
            return Err(ApiError::bad("A step is todo, now or done"));
        }
        out.push(json!({"text": text, "state": step.state}));
    }
    Ok(Value::Array(out))
}

fn clean_options(options: &[String]) -> Result<Value, ApiError> {
    if options.len() > MAX_OPTIONS {
        return Err(ApiError::bad("A question has up to 4 answers to pick from"));
    }
    let mut out: Vec<String> = vec![];
    for option in options {
        let option = option.trim();
        if option.is_empty() || chars(option) > MAX_OPTION {
            return Err(ApiError::bad("Answers to pick from are 1 to 80 characters"));
        }
        if !out.iter().any(|o| o == option) {
            out.push(option.to_owned());
        }
    }
    Ok(json!(out))
}

fn clean_links(links: &[Link]) -> Result<Value, ApiError> {
    if links.len() > MAX_LINKS {
        return Err(ApiError::bad("A card has up to 20 links"));
    }
    for link in links {
        if link.kind.trim().is_empty()
            || chars(&link.kind) > MAX_LINK_KIND
            || chars(&link.label) > MAX_LINK_LABEL
            || chars(&link.reference) > MAX_LINK_REF
        {
            return Err(ApiError::bad(
                "A link has a kind of up to 40 characters, a label of up to 200 and a reference of up to 500",
            ));
        }
    }
    Ok(serde_json::to_value(links).unwrap_or_else(|_| json!([])))
}

/// Puts `card` into column `status` at `index` (from 0), renumbering the column. With
/// no index a card goes on top of Ideas and Done and at the bottom of the others.
async fn place(
    tx: &mut PgConnection,
    project: Uuid,
    card: Uuid,
    status: &str,
    index: Option<i64>,
) -> Result<(), ApiError> {
    let mut ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM project_cards WHERE project_id = $1 AND status = $2 AND id <> $3 ORDER BY position, created_at, id",
    )
    .bind(project)
    .bind(status)
    .bind(card)
    .fetch_all(&mut *tx)
    .await?;
    let at = match index {
        Some(i) => i.clamp(0, ids.len() as i64) as usize,
        None if matches!(status, "suggested" | "idea" | "done" | "dismissed") => 0,
        None => ids.len(),
    };
    ids.insert(at, card);
    sqlx::query(
        "UPDATE project_cards c SET status = $2, position = (t.n - 1)::int
         FROM unnest($1::uuid[]) WITH ORDINALITY AS t(id, n) WHERE c.id = t.id",
    )
    .bind(&ids)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn note(
    tx: &mut PgConnection,
    card: Uuid,
    action: &str,
    actor: Actor,
    from: Option<&str>,
    to: Option<&str>,
    detail: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO project_card_events (id, card_id, action, actor, actor_id, from_status, to_status, detail)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(card)
    .bind(action)
    .bind(actor.kind())
    .bind(actor.user)
    .bind(from)
    .bind(to)
    .bind(detail.chars().take(500).collect::<String>())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM project_card_events WHERE card_id = $1 AND id NOT IN (
             SELECT id FROM project_card_events WHERE card_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2)",
    )
    .bind(card)
    .bind(KEPT_EVENTS)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Makes a card. People make ideas and Up next cards; the assistant may also suggest.
pub async fn make(
    pool: &PgPool,
    project: Uuid,
    actor: Actor,
    new: NewCard,
) -> Result<Uuid, ApiError> {
    let title = clean_title(&new.title)?;
    let notes = clean_text(&new.notes, MAX_NOTES, "Notes")?;
    let kind = clean_text(&new.kind, MAX_KIND, "The kind")?;
    let status = new.status.as_deref().unwrap_or("idea");
    let allowed: &[&str] = if actor.assistant {
        &["suggested", "idea", "next"]
    } else {
        &["idea", "next"]
    };
    if !allowed.contains(&status) {
        return Err(ApiError::bad("A new card goes into Ideas or Up next"));
    }
    let steps = clean_steps(&new.steps)?;
    let mut tx = pool.begin().await?;
    lock(&mut tx, actor.user, project).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_cards WHERE project_id = $1")
        .bind(project)
        .fetch_one(&mut *tx)
        .await?;
    if count >= MAX_CARDS {
        return Err(ApiError::bad(
            "This board has 1,000 cards, the most there can be. Delete some done ones first.",
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO project_cards (id, project_id, title, notes, kind, status, made_by, created_by, source_chat_id, steps)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(project)
    .bind(&title)
    .bind(&notes)
    .bind(&kind)
    .bind(status)
    .bind(actor.kind())
    .bind(actor.user)
    .bind(new.source_chat)
    .bind(&steps)
    .execute(&mut *tx)
    .await?;
    place(&mut tx, project, id, status, None).await?;
    note(&mut tx, id, "made", actor, None, Some(status), "").await?;
    tx.commit().await?;
    Ok(id)
}

/// A card's columns a change reads and writes.
#[derive(FromRow)]
struct Stored {
    status: String,
    title: String,
    notes: String,
    kind: String,
    worker: Option<String>,
    worker_id: Option<Uuid>,
    chat_id: Option<Uuid>,
    steps: Value,
    progress: String,
    question: String,
    options: Value,
    answer: String,
    answered_at: Option<DateTime<Utc>>,
    links: Value,
    started_at: Option<DateTime<Utc>>,
    done_at: Option<DateTime<Utc>>,
}

/// Changes a card, recording what happened to it in its history.
pub async fn change(
    pool: &PgPool,
    project: Uuid,
    id: Uuid,
    actor: Actor,
    change: Change,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    lock(&mut tx, actor.user, project).await?;
    let mut card: Stored = sqlx::query_as(
        "SELECT status, title, notes, kind, worker, worker_id, chat_id, steps, progress, question, options, answer,
                answered_at, links, started_at, done_at
         FROM project_cards WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(project)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let from = card.status.clone();
    let mut target: Option<String> = None;
    // What happened, for the history: the most telling of what this change did.
    let mut event: Option<(&str, String)> = None;
    if let Some(title) = &change.title {
        card.title = clean_title(title)?;
    }
    if let Some(notes) = &change.notes {
        card.notes = clean_text(notes, MAX_NOTES, "Notes")?;
    }
    if let Some(kind) = &change.kind {
        card.kind = clean_text(kind, MAX_KIND, "The kind")?;
    }
    if let Some(steps) = &change.steps {
        card.steps = clean_steps(steps)?;
    }
    if let Some(progress) = &change.progress {
        card.progress = clean_text(progress, MAX_PROGRESS, "The progress note")?;
    }
    if let Some(links) = &change.links {
        card.links = clean_links(links)?;
    }
    if let Some(chat) = change.chat_id {
        let ok: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM conversations WHERE id = $1 AND owner_id = $2 AND project_id = $3",
        )
        .bind(chat)
        .bind(actor.user)
        .bind(project)
        .fetch_optional(&mut *tx)
        .await?;
        if ok.is_none() {
            return Err(ApiError::bad(
                "That chat is not one of yours in this project",
            ));
        }
        card.chat_id = Some(chat);
        card.worker = Some("assistant".into());
        card.worker_id = Some(actor.user);
        target = Some("doing".into());
        event = Some(("picked", String::new()));
    }
    match change.worker.as_deref() {
        None => {}
        Some("me") => {
            card.worker = Some("person".into());
            card.worker_id = Some(actor.user);
            if from != "doing" {
                target = Some("doing".into());
            }
            event = Some(("picked", String::new()));
        }
        Some("assistant") => {
            if card.chat_id.is_none() {
                return Err(ApiError::bad(
                    "Start the card with the assistant in a chat first",
                ));
            }
            card.worker = Some("assistant".into());
            card.worker_id = Some(actor.user);
            if from != "doing" {
                target = Some("doing".into());
            }
            event = Some(("picked", String::new()));
        }
        Some("none") => {
            card.worker = None;
            card.worker_id = None;
        }
        Some(_) => return Err(ApiError::bad("Someone works a card: me, assistant or none")),
    }
    if let Some(question) = &change.question {
        let question = clean_text(question, MAX_QUESTION, "A question")?;
        card.options = clean_options(change.options.as_deref().unwrap_or_default())?;
        card.answer = String::new();
        card.answered_at = None;
        if !question.is_empty() {
            target = Some("waiting".into());
            event = Some(("asked", question.clone()));
        }
        card.question = question;
    } else if let Some(options) = &change.options {
        card.options = clean_options(options)?;
    }
    if let Some(answer) = &change.answer {
        let answer = clean_text(answer, MAX_ANSWER, "An answer")?;
        if answer.is_empty() {
            return Err(ApiError::bad("Give an answer"));
        }
        if from != "waiting" {
            return Err(ApiError::conflict(
                "This card is not waiting for an answer any more",
            ));
        }
        card.answer = answer.clone();
        card.answered_at = Some(Utc::now());
        if card.chat_id.is_some() && card.worker.is_none() {
            card.worker = Some("assistant".into());
            card.worker_id = Some(actor.user);
        }
        target = Some(
            if card.chat_id.is_some() {
                "doing"
            } else {
                "next"
            }
            .into(),
        );
        event = Some(("answered", answer));
    }
    if let Some(status) = &change.status {
        let status = clean_status(status)?;
        if status == "suggested" && from != "suggested" {
            return Err(ApiError::bad("Only the assistant suggests cards"));
        }
        target = Some(status.to_owned());
    }
    let to = target.unwrap_or_else(|| from.clone());
    if to != from {
        match to.as_str() {
            "done" => card.done_at = Some(Utc::now()),
            "suggested" | "idea" | "next" | "dismissed" => {
                card.worker = None;
                card.worker_id = None;
            }
            _ => {}
        }
        if from == "done" {
            card.done_at = None;
        }
        if to == "doing" {
            card.started_at.get_or_insert_with(Utc::now);
            if card.worker.is_none() {
                card.worker = Some(actor.kind().into());
                card.worker_id = Some(actor.user);
            }
        }
        if event.is_none() {
            event = Some((if to == "done" { "done" } else { "moved" }, String::new()));
        }
    }
    sqlx::query(
        "UPDATE project_cards SET title = $2, notes = $3, kind = $4, worker = $5, worker_id = $6, chat_id = $7,
             steps = $8, progress = $9, question = $10, options = $11, answer = $12, answered_at = $13, links = $14,
             started_at = $15, done_at = $16, updated_at = NOW()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&card.title)
    .bind(&card.notes)
    .bind(&card.kind)
    .bind(&card.worker)
    .bind(card.worker_id)
    .bind(card.chat_id)
    .bind(&card.steps)
    .bind(&card.progress)
    .bind(&card.question)
    .bind(&card.options)
    .bind(&card.answer)
    .bind(card.answered_at)
    .bind(&card.links)
    .bind(card.started_at)
    .bind(card.done_at)
    .execute(&mut *tx)
    .await?;
    if to != from || change.position.is_some() {
        place(&mut tx, project, id, &to, change.position).await?;
    }
    if let Some((action, detail)) = event {
        let moved = to != from;
        note(
            &mut tx,
            id,
            action,
            actor,
            moved.then_some(from.as_str()),
            moved.then_some(to.as_str()),
            &detail,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    visible(pool, user.id, project).await?;
    if let Some(status) = &query.status {
        let status = clean_status(status)?;
        let page = PageQuery {
            offset: query.offset,
            limit: query.limit,
        }
        .page(50);
        let order = if status == "done" {
            "c.done_at DESC NULLS LAST, c.id"
        } else {
            "c.position, c.created_at, c.id"
        };
        let rows: Vec<Card> = sqlx::query_as(&format!(
            "{CARD} WHERE c.project_id = $2 AND c.status = $3 ORDER BY {order} OFFSET $4 LIMIT $5"
        ))
        .bind(user.id)
        .bind(project)
        .bind(status)
        .bind(page.offset)
        .bind(page.fetch())
        .fetch_all(pool)
        .await?;
        let (rows, more) = page.cut(rows);
        return Ok(Json(json!({
            "items": rows.iter().map(Card::json).collect::<Vec<_>>(),
            "more": more,
        })));
    }
    let (cards, done_count) = board(pool, user.id, project).await?;
    Ok(Json(json!({
        "cards": cards.iter().map(Card::json).collect::<Vec<_>>(),
        "doneCount": done_count,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    status: Option<String>,
    offset: Option<i64>,
    limit: Option<i64>,
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Json(body): Json<NewCard>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    let id = make(pool, project, Actor::person(user.id), body).await?;
    let card = card(pool, user.id, project, id).await?;
    Ok((StatusCode::CREATED, Json(json!({"card": card.json()}))))
}

async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    visible(pool, user.id, project).await?;
    let card = card(pool, user.id, project, id).await?;
    Ok(Json(
        json!({"card": card.json(), "history": history(pool, id).await?}),
    ))
}

/// A card's history, oldest first.
async fn history(pool: &PgPool, card: Uuid) -> Result<Vec<Value>, ApiError> {
    let events: Vec<Event> = sqlx::query_as(
        "SELECT e.action, e.actor, e.actor_id, COALESCE(NULLIF(u.display_name, ''), u.username) AS actor_name,
                e.from_status, e.to_status, e.detail, e.created_at
         FROM project_card_events e LEFT JOIN users u ON u.id = e.actor_id
         WHERE e.card_id = $1 ORDER BY e.created_at, e.id",
    )
    .bind(card)
    .fetch_all(pool)
    .await?;
    Ok(events
        .into_iter()
        .map(|e| {
            json!({
                "action": e.action,
                "actor": e.actor,
                "actorId": e.actor_id,
                "actorName": e.actor_name,
                "from": e.from_status,
                "to": e.to_status,
                "detail": e.detail,
                "at": e.created_at,
            })
        })
        .collect())
}

async fn update(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<Change>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    change(pool, project, id, Actor::person(user.id), body).await?;
    let card = card(pool, user.id, project, id).await?;
    Ok(Json(json!({"card": card.json()})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let mut tx = s.store.pool.begin().await?;
    lock(&mut tx, user.id, project).await?;
    let done = sqlx::query("DELETE FROM project_cards WHERE id = $1 AND project_id = $2")
        .bind(id)
        .bind(project)
        .execute(&mut *tx)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Order {
    status: String,
    ids: Vec<Uuid>,
}

/// Puts a column in the order given. Cards someone else added to it meanwhile, or
/// left out, keep their order after the ones named; ids not in the column are skipped.
async fn reorder(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Json(body): Json<Order>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let status = clean_status(&body.status)?;
    if body.ids.len() as i64 > MAX_CARDS {
        return Err(ApiError::bad("Too many cards"));
    }
    let mut tx = s.store.pool.begin().await?;
    lock(&mut tx, user.id, project).await?;
    let current: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM project_cards WHERE project_id = $1 AND status = $2 ORDER BY position, created_at, id",
    )
    .bind(project)
    .bind(status)
    .fetch_all(&mut *tx)
    .await?;
    let mut ids: Vec<Uuid> = vec![];
    for id in body.ids.iter().chain(current.iter()) {
        if current.contains(id) && !ids.contains(id) {
            ids.push(*id);
        }
    }
    sqlx::query(
        "UPDATE project_cards c SET position = (t.n - 1)::int
         FROM unnest($1::uuid[]) WITH ORDINALITY AS t(id, n) WHERE c.id = t.id",
    )
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
