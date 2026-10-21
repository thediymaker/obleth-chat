//! Projects: what a chat starts with. A project holds instructions, knowledge
//! collections and saved items; every chat in it gets them. The owner can share a
//! project with a group, whose members start their own chats in it and see the items
//! saved there. Each person's chats stay their own.

use crate::{auth::Identity, error::ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Projects person $1 may use: their own and those shared with a group they are in.
pub const VISIBLE: &str = "(p.owner_id = $1 OR (p.scope = 'group' AND p.group_id IN (SELECT group_id FROM user_group_members WHERE user_id = $1)))";
const COLORS: [&str; 6] = ["blue", "green", "gold", "rose", "violet", "gray"];
const MAX_PROJECTS: i64 = 200;
const MAX_COLLECTIONS: usize = 8;

/// What a chat in a project starts with.
pub struct Context {
    pub id: Uuid,
    pub name: String,
    pub instructions: String,
    pub collections: Vec<Uuid>,
}

impl Context {
    pub fn prompt(&self) -> String {
        let mut text = format!(
            "# Project: {}\nThis chat is part of the project “{}”.",
            self.name, self.name
        );
        if !self.instructions.trim().is_empty() {
            text.push_str(&format!(
                " Follow the project's instructions:\n\n{}",
                self.instructions.trim()
            ));
        }
        text
    }
}

/// The project a conversation belongs to, if the person may still use it.
pub async fn for_conversation(pool: &PgPool, user: Uuid, conversation: Uuid) -> Option<Context> {
    let row = sqlx::query(&format!(
        "SELECT p.id, p.name, p.instructions FROM conversations v JOIN projects p ON p.id = v.project_id
         WHERE v.id = $2 AND v.owner_id = $1 AND {VISIBLE}"
    ))
    .bind(user)
    .bind(conversation)
    .fetch_optional(pool)
    .await
    .ok()??;
    let id: Uuid = row.get("id");
    let collections =
        sqlx::query_scalar("SELECT collection_id FROM project_collections WHERE project_id = $1")
            .bind(id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    Some(Context {
        id,
        name: row.get("name"),
        instructions: row.get("instructions"),
        collections,
    })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/projects", get(list).post(create))
        .route(
            "/api/projects/{id}",
            get(detail).patch(update).delete(remove),
        )
        .route("/api/projects/{id}/collections", put(set_collections))
        .route(
            "/api/conversations/{id}/project",
            get(conversation_project).put(set_conversation_project),
        )
        .with_state(state)
}

fn bad(message: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

fn project_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"description":r.get::<String,_>("description"),
        "instructions":r.get::<String,_>("instructions"),"color":r.get::<String,_>("color"),"scope":r.get::<String,_>("scope"),
        "groupId":r.get::<Option<Uuid>,_>("group_id"),"groupName":r.get::<Option<String>,_>("group_name"),
        "mine":r.get::<bool,_>("mine"),"ownerName":r.get::<String,_>("owner_name"),
        "chats":r.get::<i64,_>("chats"),"saved":r.get::<i64,_>("saved"),
        "updatedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")})
}

/// Projects with counts as person $1 sees them; $2 narrows to one project when set.
fn select() -> String {
    format!(
        "SELECT p.id, p.name, p.description, p.instructions, p.color, p.scope, p.group_id, g.name AS group_name,
                p.owner_id = $1 AS mine, u.display_name AS owner_name, p.updated_at,
                (SELECT COUNT(*) FROM conversations v WHERE v.project_id = p.id AND v.owner_id = $1)::bigint AS chats,
                (SELECT COUNT(*) FROM saved_items s WHERE s.project_id = p.id)::bigint AS saved
         FROM projects p JOIN users u ON u.id = p.owner_id LEFT JOIN user_groups g ON g.id = p.group_id
         WHERE {VISIBLE} AND ($2::uuid IS NULL OR p.id = $2)"
    )
}

pub async fn groups_of(pool: &PgPool, user: Uuid) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query("SELECT g.id, g.name FROM user_groups g JOIN user_group_members m ON m.group_id = g.id WHERE m.user_id = $1 ORDER BY g.name")
        .bind(user)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|r| json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name")}))
        .collect())
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let rows = sqlx::query(&format!(
        "{} ORDER BY p.owner_id = $1 DESC, p.updated_at DESC",
        select()
    ))
    .bind(user.id)
    .bind(None::<Uuid>)
    .fetch_all(&s.store.pool)
    .await?;
    Ok(Json(
        json!({"projects":rows.iter().map(project_json).collect::<Vec<_>>(),"groups":groups_of(&s.store.pool, user.id).await?}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
    color: Option<String>,
    scope: Option<String>,
    group_id: Option<Uuid>,
}

/// Checks a sharing choice: a group must be one the person belongs to.
async fn check_scope(
    pool: &PgPool,
    user: &Identity,
    scope: &str,
    group: Option<Uuid>,
) -> Result<Option<Uuid>, ApiError> {
    match scope {
        "personal" => Ok(None),
        "group" => {
            let group = group.ok_or_else(|| bad("Choose a group to share it with"))?;
            let member: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM user_group_members WHERE group_id=$1 AND user_id=$2)",
            )
            .bind(group)
            .bind(user.id)
            .fetch_one(pool)
            .await?;
            if !member {
                return Err(ApiError(
                    StatusCode::FORBIDDEN,
                    "You can share only with a group you are in".into(),
                ));
            }
            Ok(Some(group))
        }
        _ => Err(bad("Sharing must be personal or group")),
    }
}

fn clean(input: &Input) -> Result<(), ApiError> {
    if input
        .name
        .as_ref()
        .is_some_and(|n| n.trim().is_empty() || n.trim().chars().count() > 120)
    {
        return Err(bad("Names are 1 to 120 characters"));
    }
    if input
        .description
        .as_ref()
        .is_some_and(|d| d.chars().count() > 1000)
    {
        return Err(bad("Descriptions are up to 1,000 characters"));
    }
    if input
        .instructions
        .as_ref()
        .is_some_and(|d| d.chars().count() > 20000)
    {
        return Err(bad("Instructions are up to 20,000 characters"));
    }
    if input
        .color
        .as_ref()
        .is_some_and(|c| !COLORS.contains(&c.as_str()))
    {
        return Err(bad("Unknown color"));
    }
    Ok(())
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    clean(&body)?;
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| bad("Give the project a name"))?;
    let scope = body.scope.clone().unwrap_or_else(|| "personal".into());
    let group = check_scope(&s.store.pool, &user, &scope, body.group_id).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE owner_id=$1")
        .bind(user.id)
        .fetch_one(&s.store.pool)
        .await?;
    if count >= MAX_PROJECTS {
        return Err(bad("You have 200 projects, the most there can be"));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, owner_id, name, description, instructions, color, scope, group_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id)
        .bind(user.id)
        .bind(name)
        .bind(body.description.as_deref().unwrap_or("").trim())
        .bind(body.instructions.as_deref().unwrap_or("").trim())
        .bind(body.color.as_deref().unwrap_or("blue"))
        .bind(&scope)
        .bind(group)
        .execute(&s.store.pool)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"project":one(&s.store.pool, user.id, id).await?})),
    ))
}

async fn one(pool: &PgPool, user: Uuid, id: Uuid) -> Result<Value, ApiError> {
    let row = sqlx::query(&select())
        .bind(user)
        .bind(Some(id))
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(project_json(&row))
}

async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    let project = one(pool, user.id, id).await?;
    let collections: Vec<Value> = sqlx::query(
        "SELECT c.id, c.name, (SELECT COUNT(*) FROM knowledge_documents d WHERE d.collection_id = c.id)::bigint AS documents
         FROM project_collections pc JOIN knowledge_collections c ON c.id = pc.collection_id WHERE pc.project_id = $1 ORDER BY c.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"documents":r.get::<i64,_>("documents")}))
    .collect();
    let chats: Vec<Value> = sqlx::query(
        "SELECT id, title, updated_at FROM conversations WHERE project_id = $1 AND owner_id = $2 AND NOT archived ORDER BY updated_at DESC LIMIT 50",
    )
    .bind(id)
    .bind(user.id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"id":r.get::<Uuid,_>("id"),"title":r.get::<String,_>("title"),"updatedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")}))
    .collect();
    let saved = crate::knowledge::saved::in_project(pool, user.id, id, 30).await?;
    Ok(Json(
        json!({"project":project,"collections":collections,"chats":chats,"saved":saved}),
    ))
}

async fn owned(pool: &PgPool, user: Uuid, id: Uuid) -> Result<(), ApiError> {
    let owner: Option<Uuid> = sqlx::query_scalar(&format!(
        "SELECT p.owner_id FROM projects p WHERE p.id = $2 AND {VISIBLE}"
    ))
    .bind(user)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match owner {
        Some(owner) if owner == user => Ok(()),
        Some(_) => Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the project's owner can change it".into(),
        )),
        None => Err(ApiError::not_found()),
    }
}

async fn update(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Input>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    owned(pool, user.id, id).await?;
    clean(&body)?;
    let group = match &body.scope {
        Some(scope) => Some(check_scope(pool, &user, scope, body.group_id).await?),
        None => None,
    };
    sqlx::query(
        "UPDATE projects SET name = COALESCE($3, name), description = COALESCE($4, description), instructions = COALESCE($5, instructions),
             color = COALESCE($6, color), scope = COALESCE($7, scope), group_id = CASE WHEN $7 IS NULL THEN group_id ELSE $8 END, updated_at = NOW()
         WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .bind(body.name.as_deref().map(str::trim))
    .bind(body.description.as_deref().map(str::trim))
    .bind(body.instructions.as_deref().map(str::trim))
    .bind(&body.color)
    .bind(&body.scope)
    .bind(group.flatten())
    .execute(pool)
    .await?;
    Ok(Json(json!({"project":one(pool, user.id, id).await?})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    owned(&s.store.pool, user.id, id).await?;
    sqlx::query("DELETE FROM projects WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&s.store.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Collections {
    collection_ids: Vec<Uuid>,
}

async fn set_collections(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Collections>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    owned(pool, user.id, id).await?;
    if body.collection_ids.len() > MAX_COLLECTIONS {
        return Err(bad("A project can search up to 8 collections"));
    }
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM project_collections WHERE project_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // Only collections the owner may open; members search them through the project.
    sqlx::query(
        "INSERT INTO project_collections (project_id, collection_id)
         SELECT $1, c.id FROM knowledge_collections c WHERE c.id = ANY($3)
           AND (c.owner_id = $2 OR c.scope = 'everyone' OR (c.scope = 'group' AND c.group_id IN (SELECT group_id FROM user_group_members WHERE user_id = $2)))",
    )
    .bind(id)
    .bind(user.id)
    .bind(&body.collection_ids)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE projects SET updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT collection_id FROM project_collections WHERE project_id = $1")
            .bind(id)
            .fetch_all(pool)
            .await?;
    Ok(Json(json!({"collectionIds":ids})))
}

async fn conversation_project(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let project: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT project_id FROM conversations WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&s.store.pool)
            .await?;
    let project = project.ok_or_else(ApiError::not_found)?;
    let project = match project {
        Some(p) => one(&s.store.pool, user.id, p).await.ok(),
        None => None,
    };
    Ok(Json(json!({"project":project})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetProject {
    project_id: Option<Uuid>,
}

async fn set_conversation_project(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<SetProject>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    if let Some(project) = body.project_id {
        one(pool, user.id, project).await?;
    }
    let done =
        sqlx::query("UPDATE conversations SET project_id = $3 WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(user.id)
            .bind(body.project_id)
            .execute(pool)
            .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    if let Some(project) = body.project_id {
        sqlx::query("UPDATE projects SET updated_at = NOW() WHERE id = $1")
            .bind(project)
            .execute(pool)
            .await?;
    }
    let project = match body.project_id {
        Some(p) => Some(one(pool, user.id, p).await?),
        None => None,
    };
    Ok(Json(json!({"project":project})))
}
