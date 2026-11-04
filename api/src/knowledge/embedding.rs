//! Which embedding model makes Knowledge's vectors, and the prefixes it expects before
//! searches and passages. Administrators choose them in Settings; the environment gives
//! the default model, and each known model brings its own prefixes. Vectors from
//! different models or prefixes can't be compared, so when the settings in use change,
//! every document and saved item is indexed again.

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{bad, knowledge, Knowledge};
use crate::{error::ApiError, gateway::valid_model, AppState};

const MAX_PREFIX: usize = 500;

const RETRIEVAL: &str = "Represent this sentence for searching relevant passages: ";
/// Prefixes the makers of common embedding models publish for retrieval, by a part of
/// the model's name. Other models get none unless they are set.
const KNOWN: &[(&[&str], &str, &str)] = &[
    (
        &["qwen3-embedding"],
        "Instruct: Given a question, retrieve passages from the person's documents that answer it\nQuery: ",
        "",
    ),
    (
        &["embeddinggemma"],
        "task: search result | query: ",
        "title: none | text: ",
    ),
    (&["nomic-embed-text"], "search_query: ", "search_document: "),
    (
        &["multilingual-e5", "e5-large", "e5-base", "e5-small"],
        "query: ",
        "passage: ",
    ),
    (
        &["bge-large-en", "bge-base-en", "bge-small-en", "mxbai-embed-large"],
        RETRIEVAL,
        "",
    ),
];

/// Text put before searches and before stored passages, in the form the embedding
/// model was trained on.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompts {
    pub query: String,
    pub document: String,
}

impl Prompts {
    /// The published prefixes for a model, or none for a model Chat doesn't know.
    pub fn for_model(model: &str) -> Self {
        let name = model.to_ascii_lowercase();
        let (query, document) = KNOWN
            .iter()
            .find(|(names, _, _)| names.iter().any(|n| name.contains(n)))
            .map_or(("", ""), |(_, query, document)| (*query, *document));
        Self {
            query: query.into(),
            document: document.into(),
        }
    }
}

/// What applies where an administrator hasn't chosen: the environment's model, its
/// prefixes when the environment sets them, and otherwise each model's own.
struct Defaults {
    model: String,
    query: Option<String>,
    document: Option<String>,
}

impl Defaults {
    fn prompts(&self, model: &str) -> Prompts {
        let known = Prompts::for_model(model);
        // The environment's prefixes describe the environment's model only.
        let own = model == self.model;
        Prompts {
            query: self.query.clone().filter(|_| own).unwrap_or(known.query),
            document: self
                .document
                .clone()
                .filter(|_| own)
                .unwrap_or(known.document),
        }
    }

    fn resolve(&self, (model, query, document): Chosen) -> Embedding {
        let model = model.unwrap_or_else(|| self.model.clone());
        let prompts = self.prompts(&model);
        Embedding {
            query_prefix: query.unwrap_or(prompts.query),
            document_prefix: document.unwrap_or(prompts.document),
            model,
        }
    }
}

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

impl Knowledge {
    fn defaults(&self) -> Defaults {
        Defaults {
            model: self.model.clone(),
            query: self.env_prompts.0.clone(),
            document: self.env_prompts.1.clone(),
        }
    }

    /// The settings used where no administrator has chosen.
    pub fn default_embedding(&self) -> Embedding {
        self.defaults().resolve((None, None, None))
    }

    /// The settings in use now. Read on each use, so every replica follows a change.
    pub async fn embedding(&self) -> Result<Embedding, sqlx::Error> {
        Ok(self.defaults().resolve(chosen(&self.pool).await?))
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
        let reindexed = reconcile_in(&mut tx, &self.defaults()).await?;
        tx.commit().await?;
        Ok(reindexed)
    }
}

/// The first check after an upgrade only records the settings: the stored vectors were
/// made with them.
async fn reconcile_in(
    tx: &mut Transaction<'_, Postgres>,
    defaults: &Defaults,
) -> Result<bool, sqlx::Error> {
    let indexed: Option<Value> =
        sqlx::query_scalar("SELECT indexed_with FROM knowledge_embedding WHERE id FOR UPDATE")
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
    let current = json!(defaults.resolve(chosen(&mut **tx).await?));
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
    // Each model's own prefixes, which apply unless an administrator sets others.
    let defaults = k.defaults();
    let mut names: Vec<&String> = models.iter().flatten().collect();
    names.extend([&effective.model, &defaults.model]);
    let prefixes: serde_json::Map<String, Value> = names
        .into_iter()
        .map(|name| {
            let p = defaults.prompts(name);
            (
                name.clone(),
                json!({"queryPrefix": p.query, "documentPrefix": p.document}),
            )
        })
        .collect();
    Ok(json!({
        "chosen": {"model": model, "queryPrefix": query, "documentPrefix": document},
        "defaults": k.default_embedding(),
        "effective": effective,
        "models": models,
        "prefixes": prefixes,
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
    reconcile_in(&mut tx, &k.defaults()).await?;
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
    let reindexed = reconcile_in(&mut tx, &k.defaults()).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(query: Option<&str>) -> Defaults {
        Defaults {
            model: "qwen3-embedding-8b".into(),
            query: query.map(Into::into),
            document: None,
        }
    }

    #[test]
    fn known_models_bring_their_own_prefixes() {
        let gemma = Prompts::for_model("EmbeddingGemma-2");
        assert_eq!(gemma.query, "task: search result | query: ");
        assert_eq!(gemma.document, "title: none | text: ");
        assert!(Prompts::for_model("qwen3-embedding-4b")
            .query
            .starts_with("Instruct: "));
        assert_eq!(
            Prompts::for_model("multilingual-e5-large").document,
            "passage: "
        );
        assert_eq!(
            Prompts::for_model("bge-m3"),
            Prompts {
                query: "".into(),
                document: "".into()
            }
        );
        assert_eq!(Prompts::for_model("qwen3-vl-embedding-8b").query, "");
    }

    #[test]
    fn choices_then_environment_then_the_models_own() {
        let d = defaults(Some("env: "));
        let env_model = d.resolve((None, None, None));
        assert_eq!(env_model.query_prefix, "env: ");
        assert!(env_model.document_prefix.is_empty());
        // Another model takes its own prefixes, not the environment's.
        let gemma = d.resolve((Some("embeddinggemma-2".into()), None, None));
        assert_eq!(gemma.query_prefix, "task: search result | query: ");
        assert_eq!(gemma.document_prefix, "title: none | text: ");
        let chosen = d.resolve((Some("embeddinggemma-2".into()), Some(String::new()), None));
        assert_eq!(chosen.query_prefix, "");
        assert_eq!(chosen.document_prefix, "title: none | text: ");
        assert!(defaults(None)
            .resolve((None, None, None))
            .query_prefix
            .starts_with("Instruct: "));
    }
}
