//! The project log: a log that writes itself. For each person and project, one entry a
//! day, written by a model from that day's work in the project: the person's chats
//! there, their research runs and the commits made from their chats. An entry has a
//! headline, timed lines of what was asked, done, found and decided, what was still
//! open at the end of the day, and the person's own notes. Every line cites the items
//! it came from, checked in code (write.rs).
//!
//! A person's log is written from, and shows, only their own work, also in a project
//! shared with a group. The person can add notes to a day, edit a line's text (an
//! edited line stays as they left it when the day is written again) and ask for a day
//! to be written again. Chats in a project can read the log with the `project_log`
//! tool. An administrator turns the log on and picks its model; each person can turn
//! it off for themselves. Writing a day is charged to the person as its own purpose,
//! `log`, in the usage ledger.

pub mod write;

use crate::audit::{self, Action, Target};
use crate::paging::PageQuery;
use crate::{auth::Identity, error::ApiError, gateway::model_name, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use uuid::Uuid;

/// The zone of a person whose browser has not said: days run midnight to midnight UTC.
pub const DEFAULT_ZONE: &str = "UTC";
pub const TOOL: &str = "project_log";
/// Days one export or one read of the tool covers, at most.
const MAX_RANGE_DAYS: i64 = 92;
const TOOL_RANGE_DAYS: i64 = 31;
const TOOL_CHARS: usize = 24_000;
/// Reads of the log the model may make in one reply.
pub const MAX_TOOL_READS: usize = 4;
const NOTE_CHARS: usize = 2_000;
const DAYS_SHOWN: i64 = 30;

/// What the chat's system prompt says when the tool is offered.
pub const PROMPT: &str = "# Project log\nThis project keeps a daily log of the person's own work in it, written from their chats, research runs and commits, with their own notes. Read it with project_log when they ask what happened on a day or in a week, or ask for something written from it, such as a weekly update, a methods section or a statement of how AI was used. Answer from what the log says, and name the days.";

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Settings {
    pub enabled: bool,
    pub model: Option<String>,
}

pub async fn settings(pool: &PgPool) -> Result<Settings, sqlx::Error> {
    sqlx::query_as("SELECT enabled, model FROM project_log_settings WHERE id")
        .fetch_one(pool)
        .await
}

/// Whether this server writes project logs.
pub async fn available(s: &AppState) -> bool {
    s.gateway.is_some()
        && settings(&s.store.pool)
            .await
            .is_ok_and(|l| l.enabled && l.model.is_some())
}

/// A person's choices: whether their log is written, and the zone their days are in.
#[derive(Clone, Debug, Default)]
pub struct Choice {
    pub enabled: bool,
    pub time_zone: Option<String>,
}

pub async fn choice(pool: &PgPool, owner: Uuid) -> Result<Choice, sqlx::Error> {
    let row: Option<(bool, Option<String>)> =
        sqlx::query_as("SELECT enabled, time_zone FROM project_log_choices WHERE owner_id = $1")
            .bind(owner)
            .fetch_optional(pool)
            .await?;
    Ok(match row {
        Some((enabled, time_zone)) => Choice { enabled, time_zone },
        None => Choice {
            enabled: true,
            time_zone: None,
        },
    })
}

/// Whether the log is on for this person: on this server, and not turned off by them.
pub async fn on_for(s: &AppState, owner: Uuid) -> bool {
    available(s).await && choice(&s.store.pool, owner).await.is_ok_and(|c| c.enabled)
}

/// The zone a person's days are cut in.
pub async fn zone_of(pool: &PgPool, owner: Uuid) -> Result<String, sqlx::Error> {
    Ok(choice(pool, owner)
        .await?
        .time_zone
        .unwrap_or_else(|| DEFAULT_ZONE.into()))
}

/// Today in a zone.
async fn today(pool: &PgPool, zone: &str) -> Result<NaiveDate, sqlx::Error> {
    sqlx::query_scalar("SELECT (NOW() AT TIME ZONE $1)::date")
        .bind(zone)
        .fetch_one(pool)
        .await
}

/// What a line came from: a chat, a research run or a commit. Board cards and other
/// kinds can join later under their own `kind`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub kind: String,
    pub id: String,
    pub label: String,
}

impl Ref {
    pub fn json(&self) -> Value {
        json!({"kind": self.kind, "id": self.id, "label": self.label})
    }
}

pub(crate) fn off() -> ApiError {
    ApiError::feature_off("The project log is turned off on this server")
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/log/settings", get(read_choice).put(save_choice))
        .route("/api/admin/log", get(admin_read).put(admin_save))
        .route("/api/projects/{id}/log", get(list))
        .route("/api/projects/{id}/log/week", get(week))
        .route("/api/projects/{id}/log/export", get(export))
        .route("/api/projects/{id}/log/days/{day}", get(one_day))
        .route("/api/projects/{id}/log/days/{day}/write", post(write_again))
        .route("/api/projects/{id}/log/days/{day}/notes", post(add_note))
        .route(
            "/api/projects/{id}/log/days/{day}/notes/{note}",
            patch(edit_note).delete(remove_note),
        )
        .route(
            "/api/projects/{id}/log/days/{day}/lines/{line}",
            patch(edit_line),
        )
        .with_state(state)
}

/// The person, when the log is on for this server and they may see the project.
async fn open(
    s: &AppState,
    headers: &HeaderMap,
    project: Uuid,
    mutation: bool,
) -> Result<Identity, ApiError> {
    let user = s.auth.require(headers, mutation).await?;
    if !available(s).await {
        return Err(off());
    }
    let seen: Option<i32> = sqlx::query_scalar(&format!(
        "SELECT 1 FROM projects p WHERE p.id = $2 AND {}",
        crate::projects::VISIBLE
    ))
    .bind(user.id)
    .bind(project)
    .fetch_optional(&s.store.pool)
    .await?;
    seen.map(|_| user).ok_or_else(ApiError::not_found)
}

fn parse_day(day: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map_err(|_| ApiError::bad("Days are written as YYYY-MM-DD"))
}

// ---------------------------------------------------------------------------------
// Reading.

/// One day as the page and the export read it.
#[derive(Clone, Debug)]
pub struct Day {
    pub id: Uuid,
    pub day: NaiveDate,
    pub zone: String,
    pub headline: String,
    /// writing, written or waiting (work seen, not written yet).
    pub state: &'static str,
    /// The last try failed; it is tried again later.
    pub failed: bool,
    /// There is newer work than the entry includes.
    pub newer: bool,
    pub written_at: Option<DateTime<Utc>>,
    pub model: Option<String>,
    pub lines: Vec<Line>,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub id: Uuid,
    pub kind: String,
    pub time: Option<String>,
    pub at: Option<DateTime<Utc>>,
    pub text: String,
    pub refs: Vec<Value>,
    pub edited: bool,
}

#[derive(Clone, Debug)]
pub struct Note {
    pub id: Uuid,
    pub text: String,
    pub time: String,
    pub at: DateTime<Utc>,
    /// Written on the day it is about.
    pub same_day: bool,
}

const STATE: &str = "CASE WHEN d.claimed_until > NOW() THEN 'writing' WHEN d.written_at IS NOT NULL THEN 'written' ELSE 'waiting' END";

fn state_name(state: &str) -> &'static str {
    match state {
        "writing" => "writing",
        "written" => "written",
        _ => "waiting",
    }
}

/// The person's days in a project from `from` to `to`, oldest first, with their lines
/// and notes. A chat a line cites shows its current title while it is still theirs.
pub async fn days(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<Day>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT d.id, d.day, d.time_zone, d.headline, d.written_at, d.model, {STATE} AS state,
                d.attempts > 0 AND d.failed_at IS NOT NULL AS failed,
                d.written_at IS NOT NULL AND (d.written_through IS NULL OR d.written_through < d.work_at) AS newer
         FROM project_log_days d WHERE d.project_id = $1 AND d.owner_id = $2 AND d.day BETWEEN $3 AND $4
         ORDER BY d.day"
    ))
    .bind(project)
    .bind(owner)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.get("id")).collect();
    let lines = sqlx::query(
        "SELECT l.id, l.day_id, l.kind, l.text, l.refs, l.happened_at, l.edited_at IS NOT NULL AS edited,
                to_char(l.happened_at AT TIME ZONE d.time_zone, 'HH24:MI') AS time
         FROM project_log_lines l JOIN project_log_days d ON d.id = l.day_id WHERE l.day_id = ANY($1)
         ORDER BY l.kind = 'open', l.happened_at NULLS LAST, l.position",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let notes = sqlx::query(
        "SELECT n.id, n.day_id, n.text, n.created_at, to_char(n.created_at AT TIME ZONE d.time_zone, 'HH24:MI') AS time,
                (n.created_at AT TIME ZONE d.time_zone)::date = d.day AS same_day
         FROM project_log_notes n JOIN project_log_days d ON d.id = n.day_id WHERE n.day_id = ANY($1)
         ORDER BY n.created_at",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    // Chats the lines cite, as they are called now.
    let chats: Vec<Uuid> = lines
        .iter()
        .flat_map(|l| {
            l.get::<Value, _>("refs")
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|r| r["kind"] == "chat")
        .filter_map(|r| r["id"].as_str().and_then(|id| id.parse().ok()))
        .collect();
    let titles: HashMap<String, String> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, title FROM conversations WHERE id = ANY($1) AND owner_id = $2",
    )
    .bind(&chats)
    .bind(owner)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, title)| (id.to_string(), title))
    .collect();
    let mut out: Vec<Day> = rows
        .iter()
        .map(|r| Day {
            id: r.get("id"),
            day: r.get("day"),
            zone: r.get("time_zone"),
            headline: r.get("headline"),
            state: state_name(r.get("state")),
            failed: r.get("failed"),
            newer: r.get("newer"),
            written_at: r.get("written_at"),
            model: r.get("model"),
            lines: vec![],
            notes: vec![],
        })
        .collect();
    let at = |id: Uuid| out.iter().position(|d| d.id == id);
    let mut placed: Vec<(usize, Line)> = Vec::new();
    for l in &lines {
        let Some(i) = at(l.get("day_id")) else {
            continue;
        };
        let refs = l
            .get::<Value, _>("refs")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut r| {
                if r["kind"] == "chat" {
                    match r["id"].as_str().and_then(|id| titles.get(id)) {
                        Some(title) => r["label"] = json!(title),
                        None => r["gone"] = json!(true),
                    }
                }
                r
            })
            .collect();
        placed.push((
            i,
            Line {
                id: l.get("id"),
                kind: l.get("kind"),
                time: l.get("time"),
                at: l.get("happened_at"),
                text: l.get("text"),
                refs,
                edited: l.get("edited"),
            },
        ));
    }
    let mut noted: Vec<(usize, Note)> = Vec::new();
    for n in &notes {
        let Some(i) = at(n.get("day_id")) else {
            continue;
        };
        noted.push((
            i,
            Note {
                id: n.get("id"),
                text: n.get("text"),
                time: n.get("time"),
                at: n.get("created_at"),
                same_day: n.get("same_day"),
            },
        ));
    }
    for (i, line) in placed {
        out[i].lines.push(line);
    }
    for (i, note) in noted {
        out[i].notes.push(note);
    }
    Ok(out)
}

impl Day {
    pub fn json(&self) -> Value {
        let line = |l: &Line| {
            json!({"id": l.id, "kind": l.kind, "time": l.time, "at": l.at, "text": l.text,
                "refs": l.refs, "edited": l.edited})
        };
        json!({
            "day": self.day,
            "timeZone": self.zone,
            "headline": self.headline,
            "state": self.state,
            "failed": self.failed,
            "newer": self.newer,
            "writtenAt": self.written_at,
            "model": self.model,
            "lines": self.lines.iter().filter(|l| l.kind != "open").map(line).collect::<Vec<_>>(),
            "open": self.lines.iter().filter(|l| l.kind == "open").map(line).collect::<Vec<_>>(),
            "notes": self.notes.iter().map(|n| json!({"id": n.id, "text": n.text, "time": n.time,
                "at": n.at, "sameDay": n.same_day})).collect::<Vec<_>>(),
        })
    }

    /// The day as Markdown, for the export and the model.
    pub fn markdown(&self) -> String {
        let mut text = format!("## {}\n\n", self.day.format("%A %-d %B %Y"));
        if !self.headline.is_empty() {
            text.push_str(&format!("**{}**\n\n", self.headline));
        } else if self.state != "written" {
            text.push_str("_Not written yet._\n\n");
        }
        let mut timeline: Vec<(Option<&str>, String)> = self
            .lines
            .iter()
            .filter(|l| l.kind != "open")
            .map(|l| {
                let label = if l.kind == "decision" {
                    "Decision: "
                } else {
                    ""
                };
                (
                    l.time.as_deref(),
                    format!("{label}{}{}", l.text, sources(&l.refs)),
                )
            })
            .collect();
        for n in &self.notes {
            let when = if n.same_day {
                None
            } else {
                Some(" (added later)")
            };
            timeline.push((
                n.same_day.then_some(n.time.as_str()),
                format!("Your note: {}{}", n.text, when.unwrap_or("")),
            ));
        }
        timeline.sort_by_key(|(time, _)| time.map(str::to_owned).unwrap_or_else(|| "99:99".into()));
        for (time, line) in &timeline {
            match time {
                Some(t) => text.push_str(&format!("- {t} — {line}\n")),
                None => text.push_str(&format!("- {line}\n")),
            }
        }
        let open: Vec<&Line> = self.lines.iter().filter(|l| l.kind == "open").collect();
        if !open.is_empty() {
            text.push_str("\nStill open at the end of the day:\n");
            for l in open {
                text.push_str(&format!("- {}{}\n", l.text, sources(&l.refs)));
            }
        }
        text
    }
}

/// "(chat “Sweep”; run 3; commit a41c9e2)"
fn sources(refs: &[Value]) -> String {
    let named: Vec<String> = refs
        .iter()
        .filter_map(|r| {
            let label = r["label"].as_str()?;
            Some(match r["kind"].as_str()? {
                "chat" => format!("chat “{label}”"),
                "commit" => format!("commit {label}"),
                _ => label.to_lowercase(),
            })
        })
        .collect();
    if named.is_empty() {
        String::new()
    } else {
        format!(" ({})", named.join("; "))
    }
}

/// The days from `from` to `to` as one Markdown document.
pub async fn markdown(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    name: &str,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(String, usize), sqlx::Error> {
    let found = days(pool, project, owner, from, to).await?;
    let range = if from == to {
        from.format("%-d %B %Y").to_string()
    } else {
        format!("{} to {}", from.format("%-d %B %Y"), to.format("%-d %B %Y"))
    };
    let mut text = format!("# {name}: log, {range}\n\n");
    match found.first() {
        Some(first) => text.push_str(&format!(
            "Times are in {}. Written from your chats, runs and commits in the project; your notes are marked.\n\n",
            first.zone
        )),
        None => text.push_str("No day in this range had work in the project.\n"),
    }
    for day in &found {
        text.push_str(&day.markdown());
        text.push('\n');
    }
    Ok((text, found.len()))
}

async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = open(&s, &headers, project, false).await?;
    let pool = &s.store.pool;
    let page = page.page(DAYS_SHOWN);
    let (items, more) = day_list(pool, project, user.id, page).await?;
    let choice = choice(pool, user.id).await?;
    Ok(Json(json!({
        "items": items,
        "more": more,
        "enabled": choice.enabled,
        "timeZone": choice.time_zone,
    })))
}

/// The person's days in a project, newest first: date, headline and state.
pub async fn day_list(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    page: crate::paging::Page,
) -> Result<(Vec<Value>, bool), sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT d.day, d.headline, {STATE} AS state, d.attempts > 0 AND d.failed_at IS NOT NULL AS failed,
                (SELECT COUNT(*) FROM project_log_lines l WHERE l.day_id = d.id AND l.kind = 'decision') AS decisions,
                (SELECT COUNT(*) FROM project_log_notes n WHERE n.day_id = d.id) AS notes
         FROM project_log_days d WHERE d.project_id = $1 AND d.owner_id = $2
         ORDER BY d.day DESC LIMIT $3 OFFSET $4"
    ))
    .bind(project)
    .bind(owner)
    .bind(page.fetch())
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    let (rows, more) = page.cut(rows);
    Ok((
        rows.iter()
            .map(|r| {
                json!({"day": r.get::<NaiveDate, _>("day"), "headline": r.get::<String, _>("headline"),
                    "state": state_name(r.get("state")), "failed": r.get::<bool, _>("failed"),
                    "decisions": r.get::<i64, _>("decisions"), "notes": r.get::<i64, _>("notes")})
            })
            .collect(),
        more,
    ))
}

async fn one_day(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, day)): Path<(Uuid, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = open(&s, &headers, project, false).await?;
    let day = parse_day(&day)?;
    let found = days(&s.store.pool, project, user.id, day, day).await?;
    let day = found.first().ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"day": day.json()})))
}

/// The id of the person's day, or Not found.
async fn day_id(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    day: NaiveDate,
) -> Result<Uuid, ApiError> {
    sqlx::query_scalar(
        "SELECT id FROM project_log_days WHERE project_id = $1 AND owner_id = $2 AND day = $3",
    )
    .bind(project)
    .bind(owner)
    .bind(day)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)
}

async fn reply_day(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    day: NaiveDate,
) -> Result<Json<Value>, ApiError> {
    let found = days(pool, project, owner, day, day).await?;
    let day = found.first().ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"day": day.json()})))
}

async fn write_again(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, day)): Path<(Uuid, String)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = open(&s, &headers, project, true).await?;
    let day = parse_day(&day)?;
    let id = day_id(&s.store.pool, project, user.id, day).await?;
    if !write::request(&s, id).await? {
        return Err(ApiError::conflict("This day is being written now"));
    }
    let Json(body) = reply_day(&s.store.pool, project, user.id, day).await?;
    Ok((StatusCode::ACCEPTED, Json(body)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextInput {
    text: String,
}

fn note_text(text: &str) -> Result<String, ApiError> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > NOTE_CHARS {
        return Err(ApiError::bad("Notes are 1 to 2,000 characters"));
    }
    Ok(text.to_owned())
}

/// Adds the person's own note to a day of theirs.
pub async fn add_note_to(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    day: NaiveDate,
    text: &str,
) -> Result<Uuid, ApiError> {
    let text = note_text(text)?;
    let id = day_id(pool, project, owner, day).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_log_notes WHERE day_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if count >= 50 {
        return Err(ApiError::bad("A day holds 50 notes at most"));
    }
    let note = Uuid::new_v4();
    sqlx::query("INSERT INTO project_log_notes (id, day_id, text) VALUES ($1, $2, $3)")
        .bind(note)
        .bind(id)
        .bind(text)
        .execute(pool)
        .await?;
    Ok(note)
}

async fn add_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, day)): Path<(Uuid, String)>,
    Json(body): Json<TextInput>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = open(&s, &headers, project, true).await?;
    let day = parse_day(&day)?;
    add_note_to(&s.store.pool, project, user.id, day, &body.text).await?;
    let Json(body) = reply_day(&s.store.pool, project, user.id, day).await?;
    Ok((StatusCode::CREATED, Json(body)))
}

async fn edit_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, day, note)): Path<(Uuid, String, Uuid)>,
    Json(body): Json<TextInput>,
) -> Result<Json<Value>, ApiError> {
    let user = open(&s, &headers, project, true).await?;
    let day = parse_day(&day)?;
    let text = note_text(&body.text)?;
    let id = day_id(&s.store.pool, project, user.id, day).await?;
    let done = sqlx::query(
        "UPDATE project_log_notes SET text = $3, updated_at = NOW() WHERE id = $1 AND day_id = $2",
    )
    .bind(note)
    .bind(id)
    .bind(text)
    .execute(&s.store.pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    reply_day(&s.store.pool, project, user.id, day).await
}

async fn remove_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, day, note)): Path<(Uuid, String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let user = open(&s, &headers, project, true).await?;
    let day = parse_day(&day)?;
    let id = day_id(&s.store.pool, project, user.id, day).await?;
    let done = sqlx::query("DELETE FROM project_log_notes WHERE id = $1 AND day_id = $2")
        .bind(note)
        .bind(id)
        .execute(&s.store.pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Changes the text of a line the model wrote. The line keeps its time and references,
/// and stays as the person left it when the day is written again.
pub async fn edit_line_text(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    day: NaiveDate,
    line: Uuid,
    text: &str,
) -> Result<(), ApiError> {
    let text = write::one_line(text, usize::MAX);
    if text.is_empty() || text.chars().count() > write::LINE_CHARS {
        return Err(ApiError::bad("A line is 1 to 600 characters"));
    }
    let id = day_id(pool, project, owner, day).await?;
    let done = sqlx::query(
        "UPDATE project_log_lines SET text = $3, edited_at = NOW() WHERE id = $1 AND day_id = $2",
    )
    .bind(line)
    .bind(id)
    .bind(text)
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn edit_line(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((project, day, line)): Path<(Uuid, String, Uuid)>,
    Json(body): Json<TextInput>,
) -> Result<Json<Value>, ApiError> {
    let user = open(&s, &headers, project, true).await?;
    let day = parse_day(&day)?;
    edit_line_text(&s.store.pool, project, user.id, day, line, &body.text).await?;
    reply_day(&s.store.pool, project, user.id, day).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeekQuery {
    day: Option<String>,
}

/// The figures of the week (Monday to Sunday) that holds `day`, all of them counted
/// from the person's own work in the project.
pub async fn week_figures(
    pool: &PgPool,
    project: Uuid,
    owner: Uuid,
    zone: &str,
    day: NaiveDate,
) -> Result<Value, sqlx::Error> {
    let monday = day - chrono::Duration::days(i64::from(day.weekday().num_days_from_monday()));
    let sunday = monday + chrono::Duration::days(6);
    let row = sqlx::query(
        "WITH w AS (SELECT ($3::date)::timestamp AT TIME ZONE $4 AS start, ($3::date + 7)::timestamp AT TIME ZONE $4 AS finish),
              d AS (SELECT id FROM project_log_days WHERE project_id = $1 AND owner_id = $2 AND day BETWEEN $3 AND $3 + 6)
         SELECT (SELECT COUNT(*) FROM d) AS days,
                (SELECT COUNT(*) FROM project_log_lines l WHERE l.day_id IN (SELECT id FROM d) AND l.kind = 'decision') AS decisions,
                (SELECT COUNT(*) FROM project_log_notes n WHERE n.day_id IN (SELECT id FROM d)) AS notes,
                (SELECT COUNT(*) FROM conversations c, w WHERE c.project_id = $1 AND c.owner_id = $2 AND NOT c.temporary
                   AND EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id = c.id AND m.created_at >= w.start AND m.created_at < w.finish)) AS chats,
                (SELECT COUNT(*) FROM research_runs r JOIN research rs ON rs.id = r.research_id, w
                   WHERE rs.project_id = $1 AND rs.owner_id = $2 AND r.started_at >= w.start AND r.started_at < w.finish) AS runs",
    )
    .bind(project)
    .bind(owner)
    .bind(monday)
    .bind(zone)
    .fetch_one(pool)
    .await?;
    Ok(json!({
        "from": monday, "to": sunday, "timeZone": zone,
        "daysWithWork": row.get::<i64, _>("days"),
        "chats": row.get::<i64, _>("chats"),
        "runs": row.get::<i64, _>("runs"),
        "decisions": row.get::<i64, _>("decisions"),
        "notes": row.get::<i64, _>("notes"),
    }))
}

async fn week(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Query(q): Query<WeekQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = open(&s, &headers, project, false).await?;
    let pool = &s.store.pool;
    let zone = zone_of(pool, user.id).await?;
    let day = match q.day.as_deref() {
        Some(day) => parse_day(day)?,
        None => today(pool, &zone).await?,
    };
    Ok(Json(
        json!({"week": week_figures(pool, project, user.id, &zone, day).await?}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
}

/// A range of days, checked: `to` not before `from`, and at most `most` days.
fn range(from: NaiveDate, to: NaiveDate, most: i64) -> Result<(NaiveDate, NaiveDate), String> {
    if to < from {
        return Err("The range ends before it starts".into());
    }
    if (to - from).num_days() + 1 > most {
        return Err(format!("A range covers {most} days at most"));
    }
    Ok((from, to))
}

async fn export(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = open(&s, &headers, project, false).await?;
    let pool = &s.store.pool;
    let name: String = sqlx::query_scalar("SELECT name FROM projects WHERE id = $1")
        .bind(project)
        .fetch_one(pool)
        .await?;
    let zone = zone_of(pool, user.id).await?;
    let to = match q.to.as_deref() {
        Some(to) => parse_day(to)?,
        None => today(pool, &zone).await?,
    };
    let from = match q.from.as_deref() {
        Some(from) => parse_day(from)?,
        None => to - chrono::Duration::days(6),
    };
    let (from, to) = range(from, to, MAX_RANGE_DAYS).map_err(ApiError::bad)?;
    let (text, count) = markdown(pool, project, user.id, &name, from, to).await?;
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        "project".into()
    } else {
        slug
    };
    Ok(Json(json!({
        "markdown": text, "days": count, "from": from, "to": to,
        "filename": format!("{slug}-log-{from}-to-{to}.md"),
    })))
}

// ---------------------------------------------------------------------------------
// The person's own switch and zone.

async fn read_choice(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let c = choice(&s.store.pool, user.id).await?;
    Ok(Json(
        json!({"available": available(&s).await, "enabled": c.enabled, "timeZone": c.time_zone}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ChoiceInput {
    enabled: Option<bool>,
    time_zone: Option<String>,
}

/// Saves the person's switch and zone; a zone must be one Postgres knows.
pub async fn save_choice_for(
    pool: &PgPool,
    owner: Uuid,
    enabled: Option<bool>,
    time_zone: Option<&str>,
) -> Result<Choice, ApiError> {
    if let Some(zone) = time_zone {
        let shaped = !zone.is_empty()
            && zone.len() <= 64
            && zone.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && zone
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_+/-".contains(c));
        let known = shaped
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)",
            )
            .bind(zone)
            .fetch_one(pool)
            .await?;
        if !known {
            return Err(ApiError::bad("That is not a time zone this server knows"));
        }
    }
    let (enabled, time_zone): (bool, Option<String>) = sqlx::query_as(
        "INSERT INTO project_log_choices (owner_id, enabled, time_zone) VALUES ($1, COALESCE($2, TRUE), $3)
         ON CONFLICT (owner_id) DO UPDATE SET enabled = COALESCE($2, project_log_choices.enabled),
           time_zone = COALESCE($3, project_log_choices.time_zone), updated_at = NOW()
         RETURNING enabled, time_zone",
    )
    .bind(owner)
    .bind(enabled)
    .bind(time_zone)
    .fetch_one(pool)
    .await?;
    Ok(Choice { enabled, time_zone })
}

async fn save_choice(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChoiceInput>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let c = save_choice_for(
        &s.store.pool,
        user.id,
        body.enabled,
        body.time_zone.as_deref(),
    )
    .await?;
    Ok(Json(
        json!({"available": available(&s).await, "enabled": c.enabled, "timeZone": c.time_zone}),
    ))
}

// ---------------------------------------------------------------------------------
// Administration.

async fn admin_json(s: &AppState, settings: &Settings) -> Result<Value, sqlx::Error> {
    let (people, days): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(DISTINCT owner_id), COUNT(*) FILTER (WHERE written_at IS NOT NULL) FROM project_log_days",
    )
    .fetch_one(&s.store.pool)
    .await?;
    Ok(json!({
        "enabled": settings.enabled,
        "model": settings.model,
        "people": people,
        "days": days,
        "gateway": s.gateway.is_some(),
    }))
}

async fn admin_read(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    let settings = settings(&s.store.pool).await?;
    Ok(Json(admin_json(&s, &settings).await?))
}

/// Writes the log's settings and the audit row that says who changed them.
pub async fn save_settings(
    pool: &PgPool,
    admin: Uuid,
    enabled: bool,
    model: Option<String>,
) -> Result<Settings, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let saved: Settings = sqlx::query_as(
        "UPDATE project_log_settings SET enabled = $1, model = $2, updated_at = NOW() WHERE id RETURNING enabled, model",
    )
    .bind(enabled)
    .bind(model)
    .fetch_one(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(admin),
        Action::LOG_UPDATE,
        Target::site(),
        json!({"enabled": saved.enabled, "model": saved.model}),
    )
    .await?;
    tx.commit().await?;
    Ok(saved)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminPatch {
    enabled: bool,
    model: Option<String>,
}

async fn admin_save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AdminPatch>,
) -> Result<Json<Value>, ApiError> {
    let admin = s.auth.require_admin(&headers, true).await?;
    let model = model_name(body.model)?;
    if body.enabled && model.is_none() {
        return Err(ApiError::bad("Choose the model that writes the log"));
    }
    if body.enabled && s.gateway.is_none() {
        return Err(ApiError::bad(
            "The project log asks models through the gateway; connect one first",
        ));
    }
    let saved = save_settings(&s.store.pool, admin.id, body.enabled, model).await?;
    Ok(Json(admin_json(&s, &saved).await?))
}

// ---------------------------------------------------------------------------------
// The project_log tool.

pub fn definition() -> Value {
    json!({"type":"function","function":{"name":TOOL,
        "description":"Read the person's daily log of their own work in this project: for each day a headline, timed lines of what was asked, done, found and decided (with the chats, research runs and commits they came from), what was still open, and their own notes. Use it when they ask what happened, or for something written from the log such as a weekly update, a methods section or a statement of how AI was used. Give one day, or a range of up to 31 days; without dates it reads the last 7 days.",
        "parameters":{"type":"object","properties":{
            "from":{"type":"string","description":"The first day, as YYYY-MM-DD"},
            "to":{"type":"string","description":"The last day, as YYYY-MM-DD; the same as from for one day"}},
        "additionalProperties":false}}})
}

/// What one call of the tool returns.
pub struct Read {
    pub ok: bool,
    /// What the model reads.
    pub output: String,
    /// What the reply's trace shows.
    pub summary: String,
    pub detail: String,
}

fn span(from: NaiveDate, to: NaiveDate) -> String {
    if from == to {
        from.format("%-d %B").to_string()
    } else if from.month() == to.month() && from.year() == to.year() {
        format!("{}–{}", from.format("%-d"), to.format("%-d %B"))
    } else {
        format!("{} – {}", from.format("%-d %B"), to.format("%-d %B"))
    }
}

/// Runs a project_log call: the person's own days in the chat's project.
pub async fn read_for_tool(
    pool: &PgPool,
    owner: Uuid,
    project: Uuid,
    name: &str,
    arguments: &str,
) -> Read {
    let failed = |why: String| Read {
        ok: false,
        output: why.clone(),
        summary: "Could not read the project log".into(),
        detail: why,
    };
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    let date = |key: &str| -> Result<Option<NaiveDate>, String> {
        match args[key].as_str().map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => NaiveDate::parse_from_str(v, "%Y-%m-%d")
                .map(Some)
                .map_err(|_| format!("Give {key} as YYYY-MM-DD.")),
            None => Ok(None),
        }
    };
    let (from, to) = match (date("from"), date("to")) {
        (Ok(from), Ok(to)) => (from, to),
        (Err(why), _) | (_, Err(why)) => return failed(why),
    };
    let zone = match zone_of(pool, owner).await {
        Ok(zone) => zone,
        Err(_) => return failed("The log could not be read just now.".into()),
    };
    let (from, to) = match (from, to) {
        (Some(from), Some(to)) => (from, to),
        (Some(day), None) => (day, day),
        (None, Some(day)) => (day - chrono::Duration::days(6), day),
        (None, None) => match today(pool, &zone).await {
            Ok(today) => (today - chrono::Duration::days(6), today),
            Err(_) => return failed("The log could not be read just now.".into()),
        },
    };
    let (from, to) = match range(from, to, TOOL_RANGE_DAYS) {
        Ok(range) => range,
        Err(why) => return failed(format!("{why}; read a shorter range.")),
    };
    let (mut text, count) = match markdown(pool, project, owner, name, from, to).await {
        Ok(found) => found,
        Err(_) => return failed("The log could not be read just now.".into()),
    };
    if text.chars().count() > TOOL_CHARS {
        text = text.chars().take(TOOL_CHARS).collect();
        text.push_str("\n\n[The log is cut here; read fewer days for the rest.]");
    }
    Read {
        ok: true,
        output: text,
        summary: format!("Read the project log for {}", span(from, to)),
        detail: match count {
            0 => "No day with work in this range".into(),
            1 => "1 day".into(),
            n => format!("{n} days"),
        },
    }
}

#[cfg(test)]
mod tests;
