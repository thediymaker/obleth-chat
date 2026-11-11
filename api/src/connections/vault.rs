//! Secrets: named values a person saves for tools to use without the model ever
//! seeing them. The model is told the names; a tool that needs a value names it,
//! and only the owner's own browser (for a device at their computer) reads the
//! value back. Values are sealed with the connections' key, bound to their row.
use super::{manager, Connections};
use crate::{error::ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// Most secrets one person keeps.
const MAX_PER_OWNER: i64 = 50;
const MAX_VALUE: usize = 4096;

pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && name.len() <= 40
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The names of a person's secrets, for the model.
pub async fn names(pool: &PgPool, owner: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM secrets WHERE owner_id=$1 ORDER BY LOWER(name)")
        .bind(owner)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

/// Opens one of the person's secrets by name.
pub fn value_of(connections: &Connections, id: Uuid, sealed: &[u8]) -> Result<String, ApiError> {
    connections
        .unseal(id.as_bytes(), sealed)
        .ok_or_else(|| ApiError::bad("The saved value can no longer be read. Enter it again."))
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, false).await?;
    let rows: Vec<(String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT name, note, updated_at FROM secrets WHERE owner_id=$1 ORDER BY LOWER(name)",
    )
    .bind(user.id)
    .fetch_all(&connections.pool)
    .await?;
    let secrets: Vec<Value> = rows
        .into_iter()
        .map(|(name, note, updated)| json!({"name":name,"note":note,"updatedAt":updated}))
        .collect();
    Ok(Json(json!({"secrets": secrets})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Put {
    /// Missing keeps the saved value (to change only the note).
    value: Option<String>,
    note: Option<String>,
}

async fn put(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<Put>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    if !valid_name(&name) {
        return Err(ApiError::bad(
            "A secret's name is up to 40 letters, digits, dots, dashes or underscores, starting with a letter or digit",
        ));
    }
    let note = body.note.as_deref().map(str::trim).unwrap_or("");
    if note.chars().count() > 200 {
        return Err(ApiError::bad("The note can be at most 200 characters"));
    }
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM secrets WHERE owner_id=$1 AND name=$2")
            .bind(user.id)
            .bind(&name)
            .fetch_optional(&connections.pool)
            .await?;
    let value = match (&body.value, existing) {
        (Some(value), _) => {
            if value.is_empty() || value.len() > MAX_VALUE {
                return Err(ApiError::bad("The value must be between 1 and 4096 bytes"));
            }
            Some(value.clone())
        }
        (None, Some(_)) => None,
        (None, None) => return Err(ApiError::bad("Enter the secret's value")),
    };
    let id = match existing {
        Some(id) => {
            let sealed = value.as_deref().map(|v| connections.seal(id.as_bytes(), v));
            sqlx::query("UPDATE secrets SET value=COALESCE($3,value), note=CASE WHEN $4 THEN $5 ELSE note END, updated_at=NOW() WHERE id=$1 AND owner_id=$2")
                .bind(id).bind(user.id).bind(sealed).bind(body.note.is_some()).bind(note)
                .execute(&connections.pool).await?;
            id
        }
        None => {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secrets WHERE owner_id=$1")
                .bind(user.id)
                .fetch_one(&connections.pool)
                .await?;
            if count >= MAX_PER_OWNER {
                return Err(ApiError::bad(format!(
                    "Up to {MAX_PER_OWNER} secrets are allowed"
                )));
            }
            let id = Uuid::new_v4();
            let sealed = connections.seal(id.as_bytes(), value.as_deref().unwrap_or_default());
            sqlx::query("INSERT INTO secrets(id,owner_id,name,value,note) VALUES($1,$2,$3,$4,$5)")
                .bind(id)
                .bind(user.id)
                .bind(&name)
                .bind(sealed)
                .bind(note)
                .execute(&connections.pool)
                .await?;
            id
        }
    };
    let (note, updated): (String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT note, updated_at FROM secrets WHERE id=$1")
            .bind(id)
            .fetch_one(&connections.pool)
            .await?;
    Ok(Json(json!({"name":name,"note":note,"updatedAt":updated})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    let done = sqlx::query("DELETE FROM secrets WHERE owner_id=$1 AND name=$2")
        .bind(user.id)
        .bind(&name)
        .execute(&connections.pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The value, for the owner's own browser when a device tool needs it.
async fn value(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, false).await?;
    let row: Option<(Uuid, Vec<u8>)> =
        sqlx::query_as("SELECT id, value FROM secrets WHERE owner_id=$1 AND name=$2")
            .bind(user.id)
            .bind(&name)
            .fetch_optional(&connections.pool)
            .await?;
    let Some((id, sealed)) = row else {
        return Err(ApiError::not_found());
    };
    Ok(Json(
        json!({"name": name, "value": value_of(&connections, id, &sealed)?}),
    ))
}

pub fn routes() -> axum::Router<AppState> {
    use axum::routing::get;
    axum::Router::new()
        .route("/api/secrets", get(list))
        .route(
            "/api/secrets/{name}",
            axum::routing::put(put).delete(remove),
        )
        .route("/api/secrets/{name}/value", get(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_short_and_plain() {
        assert!(valid_name("beaglebone-password"));
        assert!(valid_name("WIFI_PSK"));
        assert!(valid_name("a.b"));
        assert!(!valid_name(""));
        assert!(!valid_name("-x"));
        assert!(!valid_name("has space"));
        assert!(!valid_name(&"x".repeat(41)));
    }
}
