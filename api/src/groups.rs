//! Groups as their members see them: who is in each group and what is shared with
//! it (projects and knowledge collections). Administrators manage the groups and
//! their members under Settings → Administration.

use crate::{
    auth::{Identity, Role},
    error::ApiError,
    AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Groups a person may share projects and collections with: the ones they are in, or
/// every group for an administrator.
pub async fn shareable(pool: &PgPool, user: &Identity) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query(
        "SELECT g.id, g.name FROM user_groups g
         WHERE $2 OR g.id IN (SELECT group_id FROM user_group_members WHERE user_id = $1) ORDER BY g.name",
    )
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name")}))
    .collect())
}

/// Checks that a person may share with `group`. Keeping the group something is already
/// shared with is always allowed, so leaving a group never forces an unshare on save.
pub async fn may_share(
    pool: &PgPool,
    user: &Identity,
    group: Uuid,
    current: Option<Uuid>,
) -> Result<(), ApiError> {
    if current == Some(group) {
        return Ok(());
    }
    let (exists, member): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM user_groups WHERE id = $1),
                EXISTS(SELECT 1 FROM user_group_members WHERE group_id = $1 AND user_id = $2)",
    )
    .bind(group)
    .bind(user.id)
    .fetch_one(pool)
    .await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    if member || user.is_admin() {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "You can share only with a group you are in".into(),
        ))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/groups", get(list))
        .route("/api/groups/{id}", get(detail))
        .with_state(state)
}

/// Groups person $1 is in (every group when $2, for an administrator), with what each shares.
const MINE: &str = "SELECT g.id, g.name, g.description,
        g.id IN (SELECT group_id FROM user_group_members WHERE user_id = $1) AS member,
        (SELECT COUNT(*) FROM user_group_members m JOIN users u ON u.id = m.user_id WHERE m.group_id = g.id AND u.enabled)::bigint AS members,
        (SELECT COUNT(*) FROM projects p WHERE p.scope = 'group' AND p.group_id = g.id)::bigint AS projects,
        (SELECT COUNT(*) FROM knowledge_collections c WHERE c.scope = 'group' AND c.group_id = g.id)::bigint AS collections,
        (SELECT COUNT(*) FROM conversations v WHERE v.share_scope = 'group' AND v.share_group_id = g.id)::bigint AS chats
     FROM user_groups g WHERE $2 OR g.id IN (SELECT group_id FROM user_group_members WHERE user_id = $1)";

fn group_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"description":r.get::<String,_>("description"),
        "member":r.get::<bool,_>("member"),"members":r.get::<i64,_>("members"),"projects":r.get::<i64,_>("projects"),"collections":r.get::<i64,_>("collections"),"chats":r.get::<i64,_>("chats")})
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let rows = sqlx::query(&format!("{MINE} ORDER BY member DESC, g.name"))
        .bind(user.id)
        .bind(user.role == Role::Admin)
        .fetch_all(&s.store.pool)
        .await?;
    Ok(Json(
        json!({"groups":rows.iter().map(group_json).collect::<Vec<_>>(),"canManage":user.role == Role::Admin}),
    ))
}

async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    let row = sqlx::query(&format!("SELECT * FROM ({MINE}) g WHERE g.id = $3"))
        .bind(user.id)
        .bind(user.role == Role::Admin)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let members: Vec<Value> = sqlx::query(
        "SELECT u.id, u.display_name, u.username FROM user_group_members m JOIN users u ON u.id = m.user_id
         WHERE m.group_id = $1 AND u.enabled ORDER BY u.display_name LIMIT 500",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"id":r.get::<Uuid,_>("id"),"displayName":r.get::<String,_>("display_name"),"username":r.get::<String,_>("username")}))
    .collect();
    let projects: Vec<Value> = sqlx::query(
        "SELECT p.id, p.name, p.description, p.color, u.display_name AS owner_name, p.owner_id = $2 AS mine,
                (SELECT COUNT(*) FROM saved_items s WHERE s.project_id = p.id)::bigint AS saved
         FROM projects p JOIN users u ON u.id = p.owner_id WHERE p.scope = 'group' AND p.group_id = $1 ORDER BY p.updated_at DESC",
    )
    .bind(id)
    .bind(user.id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"description":r.get::<String,_>("description"),"color":r.get::<String,_>("color"),
        "ownerName":r.get::<String,_>("owner_name"),"mine":r.get::<bool,_>("mine"),"saved":r.get::<i64,_>("saved")}))
    .collect();
    let collections: Vec<Value> = sqlx::query(
        "SELECT c.id, c.name, u.display_name AS owner_name, (SELECT COUNT(*) FROM knowledge_documents d WHERE d.collection_id = c.id)::bigint AS documents
         FROM knowledge_collections c JOIN users u ON u.id = c.owner_id WHERE c.scope = 'group' AND c.group_id = $1 ORDER BY c.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"ownerName":r.get::<String,_>("owner_name"),"documents":r.get::<i64,_>("documents")}))
    .collect();
    let chats: Vec<Value> = sqlx::query(
        "SELECT v.id, v.title, v.updated_at, u.display_name AS owner_name, v.owner_id = $2 AS mine
         FROM conversations v JOIN users u ON u.id = v.owner_id AND u.enabled
         WHERE v.share_scope = 'group' AND v.share_group_id = $1 ORDER BY v.updated_at DESC LIMIT 50",
    )
    .bind(id)
    .bind(user.id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"id":r.get::<Uuid,_>("id"),"title":r.get::<String,_>("title"),"ownerName":r.get::<String,_>("owner_name"),
        "mine":r.get::<bool,_>("mine"),"updatedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")}))
    .collect();
    Ok(Json(
        json!({"group":group_json(&row),"members":members,"projects":projects,"collections":collections,"chats":chats,"canManage":user.role == Role::Admin}),
    ))
}
