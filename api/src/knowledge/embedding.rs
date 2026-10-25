//! Which embedding model makes Knowledge's vectors, and the prefixes it expects before
//! searches and passages. Administrators choose them in Settings; the environment gives
//! the defaults. Vectors from different models or prefixes can't be compared, so when
//! the settings in use change, every document and saved item is indexed again.

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{bad, knowledge, Knowledge};
use crate::{error::ApiError, gateway::valid_model, AppState};

const MAX_PREFIX: usize = 500;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Embedding {
    pub model: String,
    pub query_prefix: String,
    pub document_prefix: String,
}

impl Embedding {
    /// A search as the model expects it.
    pub fn query(&self, text: &str) -> String {
        format!("{}{text}", self.query_prefix)
    }

    /// A stored passage or saved item as the model expects it.
    pub fn document(&self, text: &str) -> String {
        format!("{}{text}", self.document_prefix)
    }
}

/// The administrator's choices; None keeps the default.
type Chosen = (Option<String>, Option<String>, Option<String>);

async fn chosen<'e>(executor: impl PgExecutor<'e>) -> Result<Chosen, sqlx::Error> {
    Ok(sqlx::query_as(
        "SELECT model, query_prefix, document_prefix FROM knowledge_embedding WHERE id",
    )
    .fetch_optional(executor)
    .await?
    .unwrap_or_default())
}

fn resolve((model, query, document): Chosen, defaults: Embedding) -> Embedding {
    Embedding {
        model: model.unwrap_or(defaults.model),
        query_prefix: query.unwrap_or(defaults.query_prefix),
        document_prefix: document.unwrap_or(defaults.document_prefix),
    }
}

impl Knowledge {
    /// The environment's settings, used where no administrator has chosen.
    pub fn default_embedding(&self) -> Embedding {
        Embedding {
            model: self.model.clone(),
            query_prefix: self.prompts.query.clone(),
            document_prefix: self.prompts.document.clone(),
        }
    }

    /// The settings in use now. Read on each use, so every replica follows a change.
    pub async fn embedding(&self) -> Result<Embedding, sqlx::Error> {
        Ok(resolve(chosen(&self.pool).await?, self.default_embedding()))
    }

    /// Indexes everything again when the settings in use differ from those that made
    /// the stored vectors. Returns whether it did.
    pub(super) async fn reconcile(&self) -> Result<bool, sqlx::Error> {
        let indexed: Option<Value> =
            sqlx::query_scalar("SELECT indexed_with FROM knowledge_embedding WHERE id")
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let current = self.embedding().await?;
        if indexed == Some(json!(current)) {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        let reindexed = reconcile_in(&mut tx, self.default_embedding()).await?;
        tx.commit().await?;
        Ok(reindexed)
    }
}

/// The first check after an upgrade only records the settings: the stored vectors were
/// made with them.
async fn reconcile_in(
    tx: &mut Transaction<'_, Postgres>,
    defaults: Embedding,
) -> Result<bool, sqlx::Error> {
    let indexed: Option<Value> =
        sqlx::query_scalar("SELECT indexed_with FROM knowledge_embedding WHERE id FOR UPDATE")
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
    let current = json!(resolve(chosen(&mut **tx).await?, defaults));
    if indexed.as_ref() == Some(&current) {
        return Ok(false);
    }
    let reindex = indexed.is_some();
    if reindex {
        tracing::info!(settings = %current, "embedding settings changed; indexing every document and saved item again");
        sqlx::query(
            "UPDATE knowledge_documents SET status='queued', stage='', error=NULL, attempts=0, progress_done=0, claimed_at=NULL, updated_at=NOW()
             WHERE status IN ('ready', 'processing', 'failed')",
        )
        .execute(&mut **tx)
        .await?;
        sqlx::query("UPDATE saved_items SET embedding=NULL, embedding_attempts=0")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("UPDATE knowledge_embedding SET indexed_with=$1 WHERE id")
        .bind(&current)
        .execute(&mut **tx)
        .await?;
    Ok(reindex)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/knowledge/embedding", get(read).patch(save))
        .with_state(state)
}

async fn document(
    s: &AppState,
    k: &Knowledge,
    user: &crate::auth::Identity,
) -> Result<Value, ApiError> {
    let (model, query, document) = chosen(&s.store.pool).await?;
    let effective = k.embedding().await?;
    // An unreachable gateway leaves the list empty; the current model is still shown.
    let models = match &s.gateway {
        Some(gateway) => gateway.embedding_models(user).await.ok(),
        None => None,
    };
    let progress = progress(&s.store.pool).await?;
    Ok(json!({
        "chosen": {"model": model, "queryPrefix": query, "documentPrefix": document},
        "defaults": k.default_embedding(),
        "effective": effective,
        "models": models,
        "index": progress,
    }))
}

/// How far indexing has come, so an administrator can follow a change.
async fn progress(pool: &PgPool) -> Result<Value, sqlx::Error> {
    let (documents, pending, saved, saved_pending): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM knowledge_documents),
                (SELECT COUNT(*) FROM knowledge_documents WHERE status IN ('queued', 'processing')),
                (SELECT COUNT(*) FROM saved_items WHERE searchable),
                (SELECT COUNT(*) FROM saved_items WHERE searchable AND embedding IS NULL AND embedding_attempts < 3)",
    )
    .fetch_one(pool)
    .await?;
    Ok(json!({
        "documents": documents,
        "pendingDocuments": pending,
        "savedItems": saved,
        "pendingSavedItems": saved_pending,
    }))
}

async fn read(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, false).await?;
    let k = knowledge(&s)?;
    Ok(Json(document(&s, k, &user).await?))
}

/// A prefix may hold line breaks and tabs, but no other control characters.
fn prefix(value: &Value, key: &str) -> Result<Option<String>, ApiError> {
    match value {
        Value::Null => Ok(None),
        Value::String(text)
            if text.chars().count() <= MAX_PREFIX
                && !text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\t') =>
        {
            Ok(Some(text.clone()))
        }
        _ => Err(bad(&format!(
            "{key} must be text of at most {MAX_PREFIX} characters, or null for the default"
        ))),
    }
}

async fn save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let k = knowledge(&s)?;
    let patch = body
        .as_object()
        .ok_or_else(|| bad("Send an object with model, queryPrefix or documentPrefix"))?;
    if let Some(key) = patch
        .keys()
        .find(|key| !["model", "queryPrefix", "documentPrefix"].contains(&key.as_str()))
    {
        return Err(bad(&format!("Unknown setting: {key}")));
    }
    let model = match patch.get("model") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(model)) if valid_model(model) => {
            // A model the gateway doesn't serve would fail every document.
            let gateway = s.gateway.as_ref().ok_or_else(crate::gateway::unavailable)?;
            if !gateway.embedding_models(&user).await?.contains(model) {
                return Err(bad(&format!(
                    "The gateway lists no embedding model named {model}"
                )));
            }
            Some(Some(model.clone()))
        }
        Some(_) => return Err(bad("model must be a model name, or null for the default")),
    };
    let query = patch
        .get("queryPrefix")
        .map(|v| prefix(v, "queryPrefix"))
        .transpose()?;
    let document_prefix = patch
        .get("documentPrefix")
        .map(|v| prefix(v, "documentPrefix"))
        .transpose()?;

    let mut tx = crate::operations::admin_transaction(&s.store.pool, user.id).await?;
    // Record what made the stored vectors first, so this change is seen as one.
    reconcile_in(&mut tx, k.default_embedding()).await?;
    let (mut chosen_model, mut chosen_query, mut chosen_document) = chosen(&mut *tx).await?;
    if let Some(value) = model {
        chosen_model = value;
    }
    if let Some(value) = query {
        chosen_query = value;
    }
    if let Some(value) = document_prefix {
        chosen_document = value;
    }
    sqlx::query(
        "UPDATE knowledge_embedding SET model=$1, query_prefix=$2, document_prefix=$3, updated_at=NOW() WHERE id",
    )
    .bind(&chosen_model)
    .bind(&chosen_query)
    .bind(&chosen_document)
    .execute(&mut *tx)
    .await?;
    let reindexed = reconcile_in(&mut tx, k.default_embedding()).await?;
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,metadata)VALUES($1,$2,'knowledge.embedding.update','site',$3)")
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(json!({"model": chosen_model, "queryPrefix": chosen_query, "documentPrefix": chosen_document, "reindexed": reindexed}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut out = document(&s, k, &user).await?;
    out["reindexed"] = json!(reindexed);
    Ok(Json(out))
}
