//! Feedback people send with the sidebar's Feedback button: a problem, an idea or
//! anything else. It stays in Chat. Administrators read it on the Feedback tab with the
//! page and Chat version it came from, and see who sent it only when that person said
//! they may be contacted about it.
use crate::{error::ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

pub const KINDS: [&str; 3] = ["problem", "idea", "other"];
pub const MODES: [&str; 3] = ["off", "sidebar", "highlighted"];
pub const MAX_MESSAGE_CHARS: usize = 4_000;
const MAX_PAGE_CHARS: usize = 512;
/// How many a person may send in a day, so a stuck form can't fill the table.
const DAILY_LIMIT: i64 = 20;
/// The most the Feedback tab lists at once, newest first.
const LISTED: i64 = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    kind: String,
    message: String,
    #[serde(default)]
    contact: bool,
    page: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Clean {
    kind: String,
    message: String,
    page: Option<String>,
}

fn clean(body: &Send) -> Result<Clean, ApiError> {
    if !KINDS.contains(&body.kind.as_str()) {
        return Err(ApiError::bad("Feedback is a problem, an idea or other"));
    }
    let message = body.message.trim();
    if message.is_empty() {
        return Err(ApiError::bad("Write what you want to tell us"));
    }
    if message.chars().count() > MAX_MESSAGE_CHARS || message.contains('\0') {
        return Err(ApiError::bad("Feedback is up to 4,000 characters"));
    }
    // Only the path: a query string or fragment could carry a token.
    let page = body
        .page
        .as_deref()
        .map(|p| p.split(['?', '#']).next().unwrap_or("").trim())
        .filter(|p| p.starts_with('/') && !p.chars().any(char::is_control))
        .map(|p| p.chars().take(MAX_PAGE_CHARS).collect());
    Ok(Clean {
        kind: body.kind.clone(),
        message: message.to_owned(),
        page,
    })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/feedback", post(send))
        .route("/api/admin/feedback/sent", get(list))
        .route("/api/admin/feedback/sent/{id}", patch(mark))
        .route("/api/admin/feedback/button", get(button).put(set_button))
        .with_state(state)
}

/// How the Feedback button shows for everyone: off, sidebar or highlighted.
pub async fn mode(pool: &PgPool) -> Result<String, ApiError> {
    Ok(
        sqlx::query_scalar("SELECT mode FROM feedback_button WHERE id")
            .fetch_one(pool)
            .await?,
    )
}

async fn send(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Send>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    if mode(pool).await? == "off" {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Feedback is turned off on this site".into(),
        ));
    }
    let feedback = clean(&body)?;
    let today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sent_feedback WHERE user_id=$1 AND created_at>NOW()-INTERVAL '1 day'",
    )
    .bind(user.id)
    .fetch_one(pool)
    .await?;
    if today >= DAILY_LIMIT {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            format!("You can send {DAILY_LIMIT} a day. Try again tomorrow."),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sent_feedback(id,user_id,kind,message,contact,page,version) VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(id)
    .bind(user.id)
    .bind(&feedback.kind)
    .bind(&feedback.message)
    .bind(body.contact)
    .bind(&feedback.page)
    .bind(env!("CARGO_PKG_VERSION"))
    .execute(pool)
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    kind: Option<String>,
    done: Option<bool>,
}

/// One piece of feedback as administrators see it. The sender is named only when
/// they said they may be contacted.
#[derive(FromRow)]
struct Row {
    id: Uuid,
    kind: String,
    message: String,
    contact: bool,
    page: Option<String>,
    version: String,
    done: bool,
    created_at: DateTime<Utc>,
    username: Option<String>,
    display_name: Option<String>,
}

fn document(row: &Row) -> Value {
    let from = match (row.contact, &row.username) {
        (true, Some(username)) => {
            json!({"username": username, "name": row.display_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(username)})
        }
        _ => Value::Null,
    };
    json!({
        "id": row.id,
        "kind": row.kind,
        "message": row.message,
        "page": row.page,
        "version": row.version,
        "done": row.done,
        "at": row.created_at,
        "from": from,
    })
}

async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(filter): Query<Filter>,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    if filter.kind.as_deref().is_some_and(|k| !KINDS.contains(&k)) {
        return Err(ApiError::bad("Unknown kind of feedback"));
    }
    let pool = &s.store.pool;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT f.id,f.kind,f.message,f.contact,f.page,f.version,f.done,f.created_at,u.username,u.display_name
         FROM sent_feedback f LEFT JOIN users u ON u.id=f.user_id AND f.contact
         WHERE ($1::text IS NULL OR f.kind=$1) AND ($2::boolean IS NULL OR f.done=$2)
         ORDER BY f.created_at DESC LIMIT $3",
    )
    .bind(&filter.kind)
    .bind(filter.done)
    .bind(LISTED)
    .fetch_all(pool)
    .await?;
    let counts: Vec<(String, i64)> =
        sqlx::query_as("SELECT kind,COUNT(*) FROM sent_feedback WHERE NOT done GROUP BY kind")
            .fetch_all(pool)
            .await?;
    let open = |kind: &str| {
        counts
            .iter()
            .find(|(k, _)| k == kind)
            .map_or(0, |(_, n)| *n)
    };
    Ok(Json(json!({
        "items": rows.iter().map(document).collect::<Vec<_>>(),
        "open": {"problem": open("problem"), "idea": open("idea"), "other": open("other")},
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mark {
    done: bool,
}

async fn mark(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Mark>,
) -> Result<StatusCode, ApiError> {
    s.auth.require_admin(&headers, true).await?;
    let changed = sqlx::query("UPDATE sent_feedback SET done=$2 WHERE id=$1")
        .bind(id)
        .bind(body.done)
        .execute(&s.store.pool)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Button {
    mode: String,
}

async fn button(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    Ok(Json(json!({ "mode": mode(&s.store.pool).await? })))
}

async fn set_button(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Button>,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, true).await?;
    if !MODES.contains(&body.mode.as_str()) {
        return Err(ApiError::bad(
            "The Feedback button is off, sidebar or highlighted",
        ));
    }
    sqlx::query("UPDATE feedback_button SET mode=$1,updated_at=NOW() WHERE id")
        .bind(&body.mode)
        .execute(&s.store.pool)
        .await?;
    Ok(Json(json!({ "mode": body.mode })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(kind: &str, message: &str, page: Option<&str>) -> Result<Clean, ApiError> {
        clean(&Send {
            kind: kind.into(),
            message: message.into(),
            contact: false,
            page: page.map(str::to_owned),
        })
    }

    #[test]
    fn feedback_keeps_a_known_kind_the_trimmed_message_and_only_the_page_path() {
        assert_eq!(
            send(
                "idea",
                "  dark mode for code  ",
                Some("/chat/abc?token=x#top")
            )
            .unwrap(),
            Clean {
                kind: "idea".into(),
                message: "dark mode for code".into(),
                page: Some("/chat/abc".into()),
            }
        );
        assert_eq!(
            send("problem", "x", Some("https://elsewhere/"))
                .unwrap()
                .page,
            None
        );
        assert!(send("praise", "x", None).is_err());
        assert!(send("other", "   ", None).is_err());
        assert!(send("other", &"x".repeat(4_001), None).is_err());
        assert!(send("other", &"x".repeat(4_000), None).is_ok());
    }
}
