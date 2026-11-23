//! The scratch pad: things a person jots down from anywhere in the app, each kept on
//! the server so it follows them between devices. A jotting may be tagged with a
//! project the person may use; it stays its owner's alone, even in a shared project.
//! Turning one into a saved note, a board card or a chat is done by those features'
//! own endpoints; this module only keeps the list.

use crate::paging::{Page, PageQuery};
use crate::{error::ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// The most jottings a person keeps. A new one past this is refused, never made by
/// dropping an old one.
pub const MAX_JOTTINGS: i64 = 500;
/// The longest a jotting may be, in characters (the column's check).
pub const MAX_TEXT: usize = 4000;
/// Jottings in a page, unless the request asks for a limit.
const LISTED: i64 = 20;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/scratch", get(list).post(create))
        .route("/api/scratch/{id}", patch(update).delete(remove))
        .with_state(state)
}

/// A jotting as its owner sees it. The project shows only while they may still use it.
#[derive(Serialize, FromRow, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Jotting {
    pub id: Uuid,
    pub text: String,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub project_color: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Person $1's jottings with the project each is in, when they may use it.
fn select() -> String {
    format!(
        "SELECT j.id, j.text, p.id AS project_id, p.name AS project_name, p.color AS project_color, j.created_at, j.updated_at
         FROM scratch_jottings j LEFT JOIN projects p ON p.id = j.project_id AND {}
         WHERE j.owner_id = $1",
        crate::projects::VISIBLE
    )
}

/// The text as kept: without surrounding blank space, 1 to 4,000 characters.
fn clean_text(text: &str) -> Result<String, ApiError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::bad("Write something first"));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(ApiError::bad(
            "A jotting is up to 4,000 characters. Save a longer text as a note instead.",
        ));
    }
    Ok(text.to_owned())
}

/// Person `owner`'s jottings, newest first, one page at a time.
pub async fn page(
    pool: &PgPool,
    owner: Uuid,
    page: Page,
) -> Result<(Vec<Jotting>, bool), ApiError> {
    let rows: Vec<Jotting> = sqlx::query_as(&format!(
        "{} ORDER BY j.created_at DESC, j.id DESC LIMIT $2 OFFSET $3",
        select()
    ))
    .bind(owner)
    .bind(page.fetch())
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok(page.cut(rows))
}

async fn one(pool: &PgPool, owner: Uuid, id: Uuid) -> Result<Jotting, ApiError> {
    sqlx::query_as(&format!("{} AND j.id = $2", select()))
        .bind(owner)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Keeps a new jotting for `owner`, in `project` when given (one they may use).
pub async fn add(
    pool: &PgPool,
    owner: Uuid,
    text: &str,
    project: Option<Uuid>,
) -> Result<Jotting, ApiError> {
    let text = clean_text(text)?;
    if let Some(project) = project {
        crate::board::visible(pool, owner, project).await?;
    }
    let id = Uuid::new_v4();
    // The count and the insert are one statement, so two at once cannot pass the cap
    // together by much; the person's rows are few enough to count each time.
    let made = sqlx::query(
        "INSERT INTO scratch_jottings (id, owner_id, project_id, text)
         SELECT $1, $2, $3, $4 WHERE (SELECT COUNT(*) FROM scratch_jottings WHERE owner_id = $2) < $5",
    )
    .bind(id)
    .bind(owner)
    .bind(project)
    .bind(&text)
    .bind(MAX_JOTTINGS)
    .execute(pool)
    .await?;
    if made.rows_affected() == 0 {
        return Err(ApiError::bad(
            "Your scratch pad holds 500 jottings, the most it can. Delete some, or turn them into notes or cards, first.",
        ));
    }
    one(pool, owner, id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct NewJotting {
    text: String,
    project_id: Option<Uuid>,
}

/// A change to a jotting; each field is left as it is when absent. `projectId: null`
/// takes it out of its project.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub text: Option<String>,
    #[serde(default, deserialize_with = "some")]
    pub project_id: Option<Option<Uuid>>,
}

/// Keeps "absent" (None) apart from "null" (Some(None)).
fn some<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<Uuid>>, D::Error> {
    Option::<Uuid>::deserialize(d).map(Some)
}

/// Changes one of `owner`'s jottings; not found when it is someone else's.
pub async fn change(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    change: Change,
) -> Result<Jotting, ApiError> {
    let text = change.text.as_deref().map(clean_text).transpose()?;
    if let Some(Some(project)) = change.project_id {
        crate::board::visible(pool, owner, project).await?;
    }
    let done = sqlx::query(
        "UPDATE scratch_jottings SET text = COALESCE($3, text),
             project_id = CASE WHEN $4 THEN $5 ELSE project_id END, updated_at = NOW()
         WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(owner)
    .bind(&text)
    .bind(change.project_id.is_some())
    .bind(change.project_id.flatten())
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    one(pool, owner, id).await
}

async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let (items, more) = page(&s.store.pool, user.id, q.page(LISTED)).await?;
    Ok(Json(json!({"items": items, "more": more})))
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewJotting>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let jotting = add(&s.store.pool, user.id, &body.text, body.project_id).await?;
    Ok((StatusCode::CREATED, Json(json!({"jotting": jotting}))))
}

async fn update(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Change>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let jotting = change(&s.store.pool, user.id, id, body).await?;
    Ok(Json(json!({"jotting": jotting})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let done = sqlx::query("DELETE FROM scratch_jottings WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&s.store.pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}
