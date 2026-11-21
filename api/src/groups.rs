//! Groups as their members see them: who is in each group and what is shared with
//! it (projects and knowledge collections). Administrators manage the groups and
//! their members under Settings → Administration.

use crate::paging::Page;
use crate::{
    auth::{Identity, Role},
    error::ApiError,
    AppState,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// A group named in a list to share with.
#[derive(Serialize, FromRow)]
pub struct GroupName {
    id: Uuid,
    name: String,
}

/// Groups a person may share projects and collections with: the ones they are in, or
/// every group for an administrator.
pub async fn shareable(pool: &PgPool, user: &Identity) -> Result<Vec<GroupName>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT g.id, g.name FROM user_groups g
         WHERE $2 OR g.id IN (SELECT group_id FROM user_group_members WHERE user_id = $1) ORDER BY g.name",
    )
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_all(pool)
    .await?)
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
        Err(ApiError::denied(
            "You can share only with a group you are in",
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

/// A group as `MINE` reads it.
#[derive(Serialize, FromRow)]
struct Group {
    id: Uuid,
    name: String,
    description: String,
    member: bool,
    members: i64,
    projects: i64,
    collections: i64,
    chats: i64,
}

/// A project shared with a group.
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct GroupProject {
    id: Uuid,
    name: String,
    description: String,
    color: String,
    owner_name: String,
    mine: bool,
    saved: i64,
}

/// A knowledge collection shared with a group.
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct GroupCollection {
    id: Uuid,
    name: String,
    owner_name: String,
    documents: i64,
}

/// A member a group page lists.
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Member {
    id: Uuid,
    display_name: String,
    username: String,
}

/// A chat shared with a group.
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct GroupChat {
    id: Uuid,
    title: String,
    owner_name: String,
    mine: bool,
    updated_at: DateTime<Utc>,
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let groups: Vec<Group> = sqlx::query_as(&format!("{MINE} ORDER BY member DESC, g.name"))
        .bind(user.id)
        .bind(user.role == Role::Admin)
        .fetch_all(&s.store.pool)
        .await?;
    Ok(Json(
        json!({"groups":groups,"canManage":user.role == Role::Admin}),
    ))
}

async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    let group: Group = sqlx::query_as(&format!("SELECT * FROM ({MINE}) g WHERE g.id = $3"))
        .bind(user.id)
        .bind(user.role == Role::Admin)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (members, more_members) = members_of(pool, id, MEMBERS_SHOWN).await?;
    let projects: Vec<GroupProject> = sqlx::query_as(
        "SELECT p.id, p.name, p.description, p.color, u.display_name AS owner_name, p.owner_id = $2 AS mine,
                (SELECT COUNT(*) FROM saved_items s WHERE s.project_id = p.id)::bigint AS saved
         FROM projects p JOIN users u ON u.id = p.owner_id WHERE p.scope = 'group' AND p.group_id = $1 ORDER BY p.updated_at DESC",
    )
    .bind(id)
    .bind(user.id)
    .fetch_all(pool)
    .await?;
    let collections: Vec<GroupCollection> = sqlx::query_as(
        "SELECT c.id, c.name, u.display_name AS owner_name, (SELECT COUNT(*) FROM knowledge_documents d WHERE d.collection_id = c.id)::bigint AS documents
         FROM knowledge_collections c JOIN users u ON u.id = c.owner_id WHERE c.scope = 'group' AND c.group_id = $1 ORDER BY c.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let (chats, more_chats) = chats_of(pool, id, user.id, CHATS_SHOWN).await?;
    Ok(Json(
        json!({"group":group,"members":members,"moreMembers":more_members,"projects":projects,"collections":collections,"chats":chats,"moreChats":more_chats,"canManage":user.role == Role::Admin}),
    ))
}

/// Members a group page shows; `moreMembers` says when the group is larger.
const MEMBERS_SHOWN: i64 = 500;
/// Shared chats a group page shows, newest first; `moreChats` says when there are older ones.
const CHATS_SHOWN: i64 = 50;

async fn members_of(
    pool: &PgPool,
    group: Uuid,
    shown: i64,
) -> Result<(Vec<Member>, bool), sqlx::Error> {
    let page = Page {
        offset: 0,
        limit: shown,
    };
    let rows = sqlx::query_as(
        "SELECT u.id, u.display_name, u.username FROM user_group_members m JOIN users u ON u.id = m.user_id
         WHERE m.group_id = $1 AND u.enabled ORDER BY u.display_name, u.id LIMIT $2",
    )
    .bind(group)
    .bind(page.fetch())
    .fetch_all(pool)
    .await?;
    Ok(page.cut(rows))
}

async fn chats_of(
    pool: &PgPool,
    group: Uuid,
    viewer: Uuid,
    shown: i64,
) -> Result<(Vec<GroupChat>, bool), sqlx::Error> {
    let page = Page {
        offset: 0,
        limit: shown,
    };
    let rows = sqlx::query_as(
        "SELECT v.id, v.title, v.updated_at, u.display_name AS owner_name, v.owner_id = $2 AS mine
         FROM conversations v JOIN users u ON u.id = v.owner_id AND u.enabled
         WHERE v.share_scope = 'group' AND v.share_group_id = $1 ORDER BY v.updated_at DESC, v.id LIMIT $3",
    )
    .bind(group)
    .bind(viewer)
    .bind(page.fetch())
    .fetch_all(pool)
    .await?;
    Ok(page.cut(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Database;

    #[tokio::test]
    async fn a_group_page_says_when_members_and_chats_go_on() {
        let Some(db) = Database::new().await else {
            return;
        };
        let group = Uuid::new_v4();
        sqlx::query("INSERT INTO user_groups (id, name) VALUES ($1, 'Lab')")
            .bind(group)
            .execute(&db.pool)
            .await
            .unwrap();
        let mut viewer = Uuid::nil();
        for n in 0..3 {
            let user = Uuid::new_v4();
            viewer = user;
            sqlx::query("INSERT INTO users (id, username, display_name, role, password_hash) VALUES ($1, $2, $2, 'user', 'x')")
                .bind(user)
                .bind(format!("person{n}"))
                .execute(&db.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO user_group_members (group_id, user_id) VALUES ($1, $2)")
                .bind(group)
                .bind(user)
                .execute(&db.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO conversations (id, owner_id, title, model, share_scope, share_group_id) VALUES ($1, $2, $3, 'm', 'group', $4)")
                .bind(Uuid::new_v4())
                .bind(user)
                .bind(format!("chat {n}"))
                .bind(group)
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let (members, more) = members_of(&db.pool, group, 2).await.unwrap();
        assert_eq!((members.len(), more), (2, true));
        let (members, more) = members_of(&db.pool, group, 3).await.unwrap();
        assert_eq!((members.len(), more), (3, false));
        let (chats, more) = chats_of(&db.pool, group, viewer, 2).await.unwrap();
        assert_eq!((chats.len(), more, chats[0].mine), (2, true, true));
        let (chats, more) = chats_of(&db.pool, group, viewer, 50).await.unwrap();
        assert_eq!((chats.len(), more), (3, false));
        db.close().await;
    }
}
