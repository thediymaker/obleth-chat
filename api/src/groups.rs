//! Groups as their members see them: who is in each group and what is shared with
//! it (projects and knowledge collections). Administrators manage the groups and
//! their members under Settings → Administration.

use crate::{auth::Role, error::ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/groups", get(list))
        .route("/api/groups/{id}", get(detail))
        .with_state(state)
}

/// Groups person $1 is in, with what each shares.
const MINE: &str = "SELECT g.id, g.name, g.description,
        (SELECT COUNT(*) FROM user_group_members m WHERE m.group_id = g.id)::bigint AS members,
        (SELECT COUNT(*) FROM projects p WHERE p.scope = 'group' AND p.group_id = g.id)::bigint AS projects,
        (SELECT COUNT(*) FROM knowledge_collections c WHERE c.scope = 'group' AND c.group_id = g.id)::bigint AS collections
     FROM user_groups g WHERE g.id IN (SELECT group_id FROM user_group_members WHERE user_id = $1)";

fn group_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"description":r.get::<String,_>("description"),
        "members":r.get::<i64,_>("members"),"projects":r.get::<i64,_>("projects"),"collections":r.get::<i64,_>("collections")})
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let rows = sqlx::query(&format!("{MINE} ORDER BY g.name"))
        .bind(user.id)
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
    let row = sqlx::query(&format!("{MINE} AND g.id = $2"))
        .bind(user.id)
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
    Ok(Json(
        json!({"group":group_json(&row),"members":members,"projects":projects,"collections":collections,"canManage":user.role == Role::Admin}),
    ))
}
