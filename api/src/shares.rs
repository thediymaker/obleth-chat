//! Shared chats: an owner shares a chat, read-only and live, with one group or with
//! every signed-in account that has its link. Viewers read it as it grows, open its
//! attachments and images, and can fork it into a chat of their own. Everything that
//! changes the chat stays the owner's.

use crate::{
    auth::Identity,
    error::ApiError,
    groups::{may_share, shareable},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/conversations/{id}/share",
            get(sharing).put(set_sharing),
        )
        .route("/api/shared/{id}", get(read))
        .route("/api/shared/{id}/fork", post(fork))
        .route("/api/shared/{id}/attachments/{file}", get(attachment))
        .route("/api/shared/{id}/images/{image}", get(image))
        .with_state(state)
}

/// Chats person $1 may read besides their own, as SQL over `c`. A disabled owner's
/// chats are hidden with the rest of their account.
pub const SEES: &str = "(c.share_scope = 'signed_in' OR (c.share_scope = 'group' AND c.share_group_id IN (SELECT group_id FROM user_group_members WHERE user_id = $1)))
    AND EXISTS (SELECT 1 FROM users u WHERE u.id = c.owner_id AND u.enabled)";

/// The owner of a chat `viewer` may read, or not found.
async fn readable(pool: &PgPool, viewer: Uuid, id: Uuid) -> Result<Uuid, ApiError> {
    sqlx::query_scalar(&format!(
        "SELECT c.owner_id FROM conversations c WHERE c.id = $2 AND (c.owner_id = $1 OR {SEES})"
    ))
    .bind(viewer)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)
}

async fn share_json(pool: &PgPool, id: Uuid) -> Result<Value, ApiError> {
    let row = sqlx::query(
        "SELECT c.share_scope, c.share_group_id, g.name AS group_name, u.display_name AS owner_name
         FROM conversations c JOIN users u ON u.id = c.owner_id LEFT JOIN user_groups g ON g.id = c.share_group_id WHERE c.id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(
        json!({"scope":row.get::<String,_>("share_scope"),"groupId":row.get::<Option<Uuid>,_>("share_group_id"),
        "groupName":row.get::<Option<String>,_>("group_name"),"ownerName":row.get::<String,_>("owner_name")}),
    )
}

async fn owned(pool: &PgPool, user: &Identity, id: Uuid) -> Result<Option<Uuid>, ApiError> {
    let row: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT share_group_id FROM conversations WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(ApiError::not_found)
}

async fn sharing(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    owned(pool, &user, id).await?;
    Ok(Json(
        json!({"share":share_json(pool, id).await?,"groups":shareable(pool, &user).await?}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareInput {
    scope: String,
    group_id: Option<Uuid>,
}

async fn set_sharing(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ShareInput>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    let current = owned(pool, &user, id).await?;
    let group = match body.scope.as_str() {
        "none" | "signed_in" => None,
        "group" => {
            let group = body
                .group_id
                .ok_or_else(|| ApiError::bad("Choose a group to share it with"))?;
            may_share(pool, &user, group, current).await?;
            Some(group)
        }
        _ => {
            return Err(ApiError::bad(
                "Share with no one, a group, or anyone signed in",
            ))
        }
    };
    sqlx::query("UPDATE conversations SET share_scope = $3, share_group_id = $4 WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(user.id)
        .bind(&body.scope)
        .bind(group)
        .execute(pool)
        .await?;
    Ok(Json(json!({"share":share_json(pool, id).await?})))
}

/// The chat as a viewer reads it. Its own settings stay out, and images and
/// attachments point at the shared routes.
async fn read(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    let owner = readable(pool, user.id, id).await?;
    let conversation = s.store.read_shared(owner, id).await?;
    let mut value = serde_json::to_value(conversation).map_err(|_| ApiError::internal())?;
    if let Some(c) = value.as_object_mut() {
        c.remove("settings");
    }
    let images = format!("/api/shared/{id}/images/");
    for m in value["messages"].as_array_mut().into_iter().flatten() {
        if let Some(text) = m["content"].as_str() {
            m["content"] = json!(text.replace("/api/images/", &images));
        }
        for a in m["attachments"].as_array_mut().into_iter().flatten() {
            if let Some(file) = a["id"].as_str().map(str::to_owned) {
                a["url"] = json!(format!("/api/shared/{id}/attachments/{file}"));
            }
        }
    }
    Ok(Json(
        json!({"conversation":value,"share":share_json(pool, id).await?,"mine":owner == user.id}),
    ))
}

async fn fork(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let owner = readable(&s.store.pool, user.id, id).await?;
    let conversation = s.store.fork_shared(user.id, owner, id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"conversation":conversation})),
    ))
}

async fn attachment(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, file)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let owner = readable(&s.store.pool, user.id, id).await?;
    let row: Option<crate::attachments::DownloadRow> = sqlx::query_as(
        "SELECT a.name, a.mime_type, a.data, a.object_key FROM attachments a
         WHERE a.id = $1 AND a.owner_id = $2
           AND EXISTS (SELECT 1 FROM message_attachments ma JOIN messages m ON m.id = ma.message_id WHERE ma.attachment_id = a.id AND m.conversation_id = $3)",
    )
    .bind(file)
    .bind(owner)
    .bind(id)
    .fetch_optional(&s.store.pool)
    .await?;
    let (name, mime, data, key) = row.ok_or_else(ApiError::not_found)?;
    let data = s
        .store
        .files
        .read(data, key, crate::attachments::MAX_IMAGE_BYTES)
        .await?;
    crate::attachments::file_response(&name, &mime, data)
}

async fn image(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, image)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    readable(&s.store.pool, user.id, id).await?;
    let row: Option<(String, Option<Vec<u8>>, Option<String>)> = sqlx::query_as(
        "SELECT mime_type, data, object_key FROM generated_images WHERE id = $1 AND conversation_id = $2",
    )
    .bind(image)
    .bind(id)
    .fetch_optional(&s.store.pool)
    .await?;
    let (mime, data, key) = row.ok_or_else(ApiError::not_found)?;
    let data = s
        .store
        .files
        .read(data, key, crate::images::MAX_IMAGE_BYTES)
        .await?;
    crate::images::image_response(&mime, data)
}
