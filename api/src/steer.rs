//! Steering: a note the person sends while a reply works. It is read at the next
//! step boundary of the tool loop, so the run continues with it instead of stopping.
use crate::{error::ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/conversations/{id}/steer", post(send))
        .with_state(state)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Note {
    text: String,
}

async fn send(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Note>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let text = body.text.trim();
    if text.is_empty() || text.chars().count() > 2000 {
        return Err(ApiError::bad("Write a note of up to 2,000 characters"));
    }
    let generation: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT active_generation FROM conversations WHERE id=$1 AND owner_id=$2",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.store.pool)
    .await?;
    let generation = match generation {
        None => {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "Conversation not found".into(),
            ))
        }
        Some(None) => {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "The reply has finished; send your note as a message".into(),
            ))
        }
        Some(Some(generation)) => generation,
    };
    sqlx::query("DELETE FROM steer_notes WHERE created_at < NOW() - INTERVAL '1 day'")
        .execute(&s.store.pool)
        .await?;
    let note = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO steer_notes(id,conversation_id,generation_id,text) VALUES($1,$2,$3,$4)",
    )
    .bind(note)
    .bind(id)
    .bind(generation)
    .bind(text)
    .execute(&s.store.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": note}))))
}

/// The notes sent for this generation and not read yet, oldest first; they are
/// marked read.
pub async fn take(pool: &PgPool, generation: Uuid) -> Vec<(Uuid, String)> {
    let mut notes: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "UPDATE steer_notes SET consumed_at=NOW() WHERE generation_id=$1 AND consumed_at IS NULL RETURNING id,text,created_at",
    )
    .bind(generation)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    notes.sort_by_key(|n| n.2);
    notes.into_iter().map(|(id, text, _)| (id, text)).collect()
}
