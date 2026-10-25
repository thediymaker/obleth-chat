//! Saved items: functions, notes, links and excerpts a person keeps while working.
//! Each item is one searchable unit with its own embedding, found by `search_knowledge`
//! alongside collection passages, and listed in Library.

use super::{bad, knowledge, vector_literal, words, Hit, Knowledge};
use crate::{auth::Identity, error::ApiError, gateway::Gateway, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

pub const KINDS: [&str; 4] = ["code", "note", "link", "excerpt"];
const MAX_ITEMS: i64 = 5000;
const MAX_BODY: usize = 65536;
const EMBED_CHARS: usize = 6000;
const COLUMNS: &str = "s.id, s.owner_id, s.kind, s.title, s.body, s.language, s.url, s.tags, s.searchable, s.embedding IS NOT NULL AS indexed,
    s.source_conversation_id, s.source_message_id, c.title AS source_title, s.project_id, s.pinned, s.created_at, s.updated_at,
    (SELECT u.display_name FROM users u WHERE u.id = s.owner_id) AS owner_name";
/// Saved items person $1 may see: their own, and those in projects shared with them.
const SEES: &str = "(s.owner_id = $1 OR s.project_id IN (SELECT p.id FROM projects p WHERE p.scope = 'group' AND p.group_id IN (SELECT group_id FROM user_group_members WHERE user_id = $1)))";

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub kind: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub language: Option<String>,
    pub url: Option<String>,
    pub tags: Option<Vec<String>>,
    pub searchable: Option<bool>,
    pub source_conversation_id: Option<Uuid>,
    pub source_message_id: Option<Uuid>,
    /// A project id, or null to take it out of its project.
    #[serde(default, deserialize_with = "some")]
    pub project_id: Option<Option<Uuid>>,
    pub pinned: Option<bool>,
}

/// Keeps "absent" (None) apart from "null" (Some(None)).
fn some<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<Uuid>>, D::Error> {
    Option::<Uuid>::deserialize(d).map(Some)
}

/// Lowercase tags without '#', letters, digits and - _ . + only, at most 12.
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for tag in tags {
        let tag: String = tag
            .trim()
            .trim_start_matches('#')
            .to_lowercase()
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
            .take(32)
            .collect();
        if !tag.is_empty() && !out.contains(&tag) && out.len() < 12 {
            out.push(tag);
        }
    }
    out
}

pub(crate) fn item_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({"id":r.get::<Uuid,_>("id"),"ownerId":r.get::<Uuid,_>("owner_id"),"ownerName":r.get::<Option<String>,_>("owner_name"),"projectId":r.get::<Option<Uuid>,_>("project_id"),"pinned":r.get::<bool,_>("pinned"),"kind":r.get::<String,_>("kind"),"title":r.get::<String,_>("title"),"body":r.get::<String,_>("body"),
        "language":r.get::<String,_>("language"),"url":r.get::<Option<String>,_>("url"),"tags":r.get::<Vec<String>,_>("tags"),
        "searchable":r.get::<bool,_>("searchable"),"indexed":r.get::<bool,_>("indexed"),
        "source":r.get::<Option<Uuid>,_>("source_conversation_id").map(|id| json!({"conversationId":id,"messageId":r.get::<Option<Uuid>,_>("source_message_id"),"title":r.get::<Option<String>,_>("source_title")})),
        "createdAt":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),"updatedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")})
}

fn embed_text(title: &str, tags: &[String], body: &str) -> String {
    let mut text = format!("{title}\n{}\n{body}", tags.join(" "));
    if text.chars().count() > EMBED_CHARS {
        text = text.chars().take(EMBED_CHARS).collect();
    }
    text
}

impl Knowledge {
    /// Saves an item for `user`. The embedding is made now when the gateway answers,
    /// otherwise the worker makes it later.
    pub async fn save(
        &self,
        gateway: Option<&Gateway>,
        user: &Identity,
        input: Input,
    ) -> Result<Value, ApiError> {
        let kind = input.kind.unwrap_or_else(|| "note".into());
        if !KINDS.contains(&kind.as_str()) {
            return Err(bad("Kind must be code, note, link or excerpt"));
        }
        let title: String = input
            .title
            .unwrap_or_default()
            .trim()
            .chars()
            .take(200)
            .collect();
        let body = input.body.unwrap_or_default();
        let url = input
            .url
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty());
        if let Some(url) = &url {
            if url.len() > 2048 || !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(bad("Links must be http(s) addresses"));
            }
        }
        let title = if title.is_empty() {
            url.clone()
                .or_else(|| {
                    body.lines()
                        .find(|l| !l.trim().is_empty())
                        .map(|l| l.trim().chars().take(80).collect())
                })
                .ok_or_else(|| bad("Give it a title"))?
        } else {
            title
        };
        if body.len() > MAX_BODY {
            return Err(bad("Saved items are up to 64 KB"));
        }
        let tags = clean_tags(&input.tags.unwrap_or_default());
        let language: String = input
            .language
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .chars()
            .take(40)
            .collect();
        // A source must be one of the person's own conversations.
        let source: Option<(Uuid, Option<Uuid>)> = match input.source_conversation_id {
            Some(conversation) => sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM conversations WHERE id=$1 AND owner_id=$2",
            )
            .bind(conversation)
            .bind(user.id)
            .fetch_optional(&self.pool)
            .await?
            .map(|c| (c, input.source_message_id)),
            None => None,
        };
        let message = match source {
            Some((conversation, Some(message))) => {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM messages WHERE id=$1 AND conversation_id=$2",
                )
                .bind(message)
                .bind(conversation)
                .fetch_optional(&self.pool)
                .await?
            }
            _ => None,
        };
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM saved_items WHERE owner_id=$1")
            .bind(user.id)
            .fetch_one(&self.pool)
            .await?;
        if count >= MAX_ITEMS {
            return Err(bad(
                "You have 5,000 saved items, the most there can be. Delete some first.",
            ));
        }
        let project = match input.project_id.flatten() {
            Some(project) => Some(usable_project(&self.pool, user.id, project).await?),
            None => None,
        };
        let searchable = input.searchable.unwrap_or(true);
        let embedding = match gateway.filter(|_| searchable) {
            Some(gateway) => {
                self.embed_one(gateway, user, &embed_text(&title, &tags, &body))
                    .await
            }
            None => None,
        };
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO saved_items (id, owner_id, kind, title, body, language, url, tags, searchable, source_conversation_id, source_message_id, embedding, project_id, pinned)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::halfvec,$13,$14)",
        )
        .bind(id)
        .bind(user.id)
        .bind(&kind)
        .bind(&title)
        .bind(&body)
        .bind(&language)
        .bind(&url)
        .bind(&tags)
        .bind(searchable)
        .bind(source.map(|s| s.0))
        .bind(message)
        .bind(embedding)
        .bind(project)
        .bind(input.pinned.unwrap_or(false))
        .execute(&self.pool)
        .await?;
        self.item(user.id, id).await
    }

    async fn embed_one(&self, gateway: &Gateway, user: &Identity, text: &str) -> Option<String> {
        match gateway
            .embeddings(user, &self.model, &[self.document_input(text)])
            .await
        {
            Ok(mut v) => v.pop().map(vector_literal),
            Err(e) => {
                tracing::warn!("saved item embedding deferred: {}", e.1);
                None
            }
        }
    }

    pub async fn item(&self, owner: Uuid, id: Uuid) -> Result<Value, ApiError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM saved_items s LEFT JOIN conversations c ON c.id = s.source_conversation_id AND c.owner_id = $1 WHERE s.id=$2 AND {SEES}"
        ))
        .bind(owner)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
        Ok(item_json(&row))
    }

    /// Makes missing embeddings, a few at a time. Returns whether there was work.
    pub(super) async fn embed_saved(&self, gateway: &Gateway) -> bool {
        let rows = match sqlx::query(
            "UPDATE saved_items SET embedding_attempts = embedding_attempts + 1 WHERE id IN (
                SELECT id FROM saved_items WHERE embedding IS NULL AND searchable AND embedding_attempts < 3
                ORDER BY created_at LIMIT 8 FOR UPDATE SKIP LOCKED)
             RETURNING id, owner_id, title, tags, body",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(_) => return false,
        };
        let worked = !rows.is_empty();
        for row in rows {
            let Ok(owner) = super::identity(&self.pool, row.get("owner_id")).await else {
                continue;
            };
            let text = embed_text(
                row.get::<&str, _>("title"),
                &row.get::<Vec<String>, _>("tags"),
                row.get::<&str, _>("body"),
            );
            if let Some(vector) = self.embed_one(gateway, &owner, &text).await {
                let _ = sqlx::query("UPDATE saved_items SET embedding=$2::halfvec WHERE id=$1")
                    .bind(row.get::<Uuid, _>("id"))
                    .bind(vector)
                    .execute(&self.pool)
                    .await;
            }
        }
        worked
    }

    pub async fn has_saved(&self, owner: Uuid, project: Option<Uuid>) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM saved_items WHERE (owner_id=$1 OR ($2::uuid IS NOT NULL AND project_id=$2)) AND searchable)",
        )
        .bind(owner)
        .bind(project)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }

    /// Saved items that best match the query, by meaning and words, as ranked ids.
    async fn rank_saved(
        &self,
        gateway: &Gateway,
        user: &Identity,
        query: &str,
        only_searchable: bool,
        project: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<(Uuid, bool, bool)>, ApiError> {
        let text: String = query.trim().chars().take(2000).collect();
        if text.is_empty() {
            return Ok(vec![]);
        }
        let vector = gateway
            .embeddings(user, &self.model, &[self.query_input(&text)])
            .await?
            .pop()
            .map(vector_literal)
            .ok_or_else(ApiError::internal)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            "WITH mine AS (SELECT id, embedding, tsv FROM saved_items WHERE (owner_id = $1 OR ($6::uuid IS NOT NULL AND project_id = $6)) AND (searchable OR NOT $5)),
             vec AS (
                SELECT id, ROW_NUMBER() OVER (ORDER BY embedding <=> $2::halfvec) AS r
                FROM (SELECT id, embedding FROM mine WHERE embedding IS NOT NULL ORDER BY embedding <=> $2::halfvec LIMIT 30) v
             ), kw AS (
                SELECT id, ROW_NUMBER() OVER (ORDER BY rank DESC) AS r FROM (
                    SELECT m.id, ts_rank_cd(m.tsv, q) AS rank FROM mine m, to_tsquery('english', $3) q
                    WHERE $3 <> '' AND m.tsv @@ q ORDER BY rank DESC LIMIT 30) k
             )
             SELECT id, SUM(1.0 / (60 + r)) AS score, BOOL_OR(src = 'v') AS by_meaning, BOOL_OR(src = 'k') AS by_words
             FROM (SELECT id, r, 'v' AS src FROM vec UNION ALL SELECT id, r, 'k' FROM kw) u
             GROUP BY id ORDER BY score DESC LIMIT $4",
        )
        .bind(user.id)
        .bind(&vector)
        .bind(words(&text))
        .bind(limit as i64)
        .bind(only_searchable)
        .bind(project)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .iter()
            .map(|r| (r.get("id"), r.get("by_meaning"), r.get("by_words")))
            .collect())
    }

    /// Saved items as passages for a chat.
    pub async fn search_saved(
        &self,
        gateway: &Gateway,
        user: &Identity,
        query: &str,
        project: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<Hit>, ApiError> {
        let ranked = self
            .rank_saved(gateway, user, query, true, project, limit)
            .await?;
        let ids: Vec<Uuid> = ranked.iter().map(|r| r.0).collect();
        let rows = sqlx::query(
            "SELECT id, kind, title, body, language, url FROM saved_items WHERE id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(ranked
            .iter()
            .filter_map(|(id, meaning, by_words)| {
                let r = rows.iter().find(|r| r.get::<Uuid, _>("id") == *id)?;
                let (kind, body, url) = (
                    r.get::<String, _>("kind"),
                    r.get::<String, _>("body"),
                    r.get::<Option<String>, _>("url"),
                );
                let language = r.get::<String, _>("language");
                let mut content = String::new();
                if let Some(url) = url {
                    content.push_str(&format!("{url}\n"));
                }
                content.push_str(&if kind == "code" {
                    format!(
                        "```{language}\n{}\n```",
                        body.chars().take(4000).collect::<String>()
                    )
                } else {
                    body.chars().take(4000).collect()
                });
                Some(Hit {
                    document: *id,
                    collection: Uuid::nil(),
                    name: r.get("title"),
                    page: None,
                    content,
                    matched: match (meaning, by_words) {
                        (true, true) => "meaning + words",
                        (false, true) => "words",
                        _ => "meaning",
                    },
                    saved: true,
                })
            })
            .collect())
    }
}

/// A project the person may use, or not found.
async fn usable_project(pool: &sqlx::PgPool, user: Uuid, project: Uuid) -> Result<Uuid, ApiError> {
    sqlx::query_scalar::<_, Uuid>(&format!(
        "SELECT p.id FROM projects p WHERE p.id = $2 AND {}",
        crate::projects::VISIBLE
    ))
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)
}

/// The newest items in a project (pinned first), for its page. The caller checked
/// that the person may use the project.
pub async fn in_project(
    pool: &sqlx::PgPool,
    user: Uuid,
    project: Uuid,
    limit: i64,
) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query(&format!(
        "SELECT {COLUMNS} FROM saved_items s LEFT JOIN conversations c ON c.id = s.source_conversation_id AND c.owner_id = $1
         WHERE s.project_id = $2 ORDER BY s.pinned DESC, s.created_at DESC LIMIT $3"
    ))
    .bind(user)
    .bind(project)
    .bind(limit)
    .fetch_all(pool)
    .await?
    .iter()
    .map(item_json)
    .collect())
}

// ---- HTTP ----

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/saved", get(list).post(create))
        .route("/api/saved/{id}", get(one).patch(update).delete(remove))
        .with_state(state)
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    project: Option<Uuid>,
    offset: Option<i64>,
}

async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    let kind = q.kind.filter(|k| KINDS.contains(&k.as_str()));
    let tag = q
        .tag
        .map(|t| clean_tags(&[t]))
        .and_then(|t| t.into_iter().next());
    let query = q.q.unwrap_or_default();
    let offset = q.offset.unwrap_or(0).clamp(0, 100_000);
    // One project's items (anyone's, when it is shared), or all of the person's own.
    let project = match q.project {
        Some(p) => Some(usable_project(&k.pool, user.id, p).await?),
        None => None,
    };
    let scope = match project {
        Some(p) => format!("s.project_id = '{p}'"),
        None => "s.owner_id = $1".into(),
    };
    let base = format!(
        "SELECT {COLUMNS} FROM saved_items s LEFT JOIN conversations c ON c.id = s.source_conversation_id AND c.owner_id = $1
         WHERE {scope} AND ($2::text IS NULL OR s.kind=$2) AND ($3::text IS NULL OR $3 = ANY(s.tags))"
    );
    let (items, more) = if query.trim().is_empty() {
        let rows = sqlx::query(&format!(
            "{base} ORDER BY s.pinned DESC, s.created_at DESC LIMIT 61 OFFSET $4"
        ))
        .bind(user.id)
        .bind(&kind)
        .bind(&tag)
        .bind(offset)
        .fetch_all(&k.pool)
        .await?;
        let more = rows.len() > 60;
        (
            rows.iter().take(60).map(item_json).collect::<Vec<_>>(),
            more,
        )
    } else {
        let gateway = s.gateway.as_ref().ok_or_else(crate::gateway::unavailable)?;
        let ranked = k
            .rank_saved(gateway, &user, &query, false, project, 60)
            .await?;
        let ids: Vec<Uuid> = ranked.iter().map(|r| r.0).collect();
        let rows = sqlx::query(&format!("{base} AND s.id = ANY($4)"))
            .bind(user.id)
            .bind(&kind)
            .bind(&tag)
            .bind(&ids)
            .fetch_all(&k.pool)
            .await?;
        let items = ids
            .iter()
            .filter_map(|id| {
                rows.iter()
                    .find(|r| r.get::<Uuid, _>("id") == *id)
                    .map(item_json)
            })
            .collect();
        (items, false)
    };
    let tags: Vec<Value> = sqlx::query("SELECT t, COUNT(*)::bigint AS n FROM saved_items, UNNEST(tags) t WHERE owner_id=$1 GROUP BY t ORDER BY n DESC, t LIMIT 60")
        .bind(user.id)
        .fetch_all(&k.pool)
        .await?
        .iter()
        .map(|r| json!({"tag":r.get::<String,_>("t"),"count":r.get::<i64,_>("n")}))
        .collect();
    let kinds: Vec<Value> = sqlx::query(
        "SELECT kind, COUNT(*)::bigint AS n FROM saved_items WHERE owner_id=$1 GROUP BY kind",
    )
    .bind(user.id)
    .fetch_all(&k.pool)
    .await?
    .iter()
    .map(|r| json!({"kind":r.get::<String,_>("kind"),"count":r.get::<i64,_>("n")}))
    .collect();
    Ok(Json(
        json!({"items":items,"more":more,"tags":tags,"kinds":kinds}),
    ))
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    let item = k.save(s.gateway.as_ref(), &user, body).await?;
    Ok((StatusCode::CREATED, Json(json!({"item":item}))))
}

async fn one(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let k = knowledge(&s)?;
    Ok(Json(json!({"item":k.item(user.id, id).await?})))
}

async fn update(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Input>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    let current = k.item(user.id, id).await?;
    if current["ownerId"] != json!(user.id) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the person who saved it can change it".into(),
        ));
    }
    let project: Option<Uuid> = match body.project_id {
        Some(Some(p)) => Some(usable_project(&k.pool, user.id, p).await?),
        Some(None) => None,
        None => serde_json::from_value(current["projectId"].clone()).unwrap_or(None),
    };
    let pinned = body
        .pinned
        .unwrap_or(current["pinned"].as_bool().unwrap_or(false));
    let text = |key: &str| current[key].as_str().unwrap_or("").to_owned();
    let title: String = body
        .title
        .map(|t| t.trim().chars().take(200).collect())
        .unwrap_or_else(|| text("title"));
    if title.is_empty() {
        return Err(bad("Give it a title"));
    }
    let item_body = body.body.unwrap_or_else(|| text("body"));
    if item_body.len() > MAX_BODY {
        return Err(bad("Saved items are up to 64 KB"));
    }
    let kind = body.kind.unwrap_or_else(|| text("kind"));
    if !KINDS.contains(&kind.as_str()) {
        return Err(bad("Kind must be code, note, link or excerpt"));
    }
    let tags = match body.tags {
        Some(tags) => clean_tags(&tags),
        None => serde_json::from_value(current["tags"].clone()).unwrap_or_default(),
    };
    let searchable = body
        .searchable
        .unwrap_or(current["searchable"].as_bool().unwrap_or(true));
    let language: String = body
        .language
        .unwrap_or_else(|| text("language"))
        .trim()
        .to_lowercase()
        .chars()
        .take(40)
        .collect();
    let changed =
        title != text("title") || item_body != text("body") || current["tags"] != json!(tags);
    sqlx::query(
        "UPDATE saved_items SET title=$3, body=$4, kind=$5, tags=$6, searchable=$7, language=$8, project_id=$10, pinned=$11, updated_at=NOW(),
             embedding = CASE WHEN $9 THEN NULL ELSE embedding END,
             embedding_attempts = CASE WHEN $9 THEN 0 ELSE embedding_attempts END
         WHERE id=$1 AND owner_id=$2",
    )
    .bind(id)
    .bind(user.id)
    .bind(&title)
    .bind(&item_body)
    .bind(&kind)
    .bind(&tags)
    .bind(searchable)
    .bind(&language)
    .bind(changed)
    .bind(project)
    .bind(pinned)
    .execute(&k.pool)
    .await?;
    Ok(Json(json!({"item":k.item(user.id, id).await?})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let k = knowledge(&s)?;
    let done = sqlx::query("DELETE FROM saved_items WHERE id=$1 AND owner_id=$2")
        .bind(id)
        .bind(user.id)
        .execute(&k.pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_cleaned() {
        let tags = clean_tags(&[
            "#Research".into(),
            "research".into(),
            " vLLM tuning ".into(),
            "!!".into(),
            "c++".into(),
        ]);
        assert_eq!(tags, vec!["research", "vllm-tuning", "c++"]);
    }
}
