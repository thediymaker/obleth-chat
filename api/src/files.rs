//! Where people's files keep their bytes: chat attachments and generated images.
//! With object storage configured (OBLETH_CHAT_S3_*), new files go to the bucket and
//! their rows keep only the object's key; without it the bytes
//! stay in Postgres. Rows of either kind stay readable, and the worker moves rows still
//! holding bytes into the bucket, so turning storage on later needs nothing else.
//!
//! An object is uploaded before the row naming it is committed. It is first recorded
//! in `object_trash` as pending, and a trigger removes that record when a row claims
//! the key, so an object whose row never commits is deleted after `PENDING_GRACE`.

use std::time::Duration;

use axum::http::StatusCode;
use futures_util::{stream, StreamExt, TryStreamExt};
use sqlx::{PgPool, Row};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{error::ApiError, knowledge::ObjectStore};

/// How long an uploaded object may wait for the row that names it.
const PENDING_GRACE: &str = "15 minutes";
/// Objects read at once for one request.
const PARALLEL_READS: usize = 8;

#[derive(Clone)]
pub struct Files {
    pool: PgPool,
    store: Option<ObjectStore>,
}

/// A file's bytes as its row stores them: in `data`, or in the bucket under `key`.
pub struct Placed {
    pub data: Option<Vec<u8>>,
    pub key: Option<String>,
}

impl Files {
    pub fn new(pool: PgPool, store: Option<ObjectStore>) -> Self {
        Self { pool, store }
    }

    /// Whether new files go to object storage.
    pub fn in_bucket(&self) -> bool {
        self.store.is_some()
    }

    /// Uploads `bytes` under `key` when there is a bucket, or hands them back for the
    /// row's `data` column when there isn't.
    pub async fn place(
        &self,
        key: String,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<Placed, ApiError> {
        let Some(store) = &self.store else {
            return Ok(Placed {
                data: Some(bytes),
                key: None,
            });
        };
        put(&self.pool, store, &key, bytes, content_type)
            .await
            .map_err(unavailable)?;
        Ok(Placed {
            data: None,
            key: Some(key),
        })
    }

    /// Removes an object whose row was not written. Best effort: the trash deletes it
    /// after the grace period anyway.
    pub async fn discard(&self, placed: &Placed) {
        if let (Some(store), Some(key)) = (&self.store, &placed.key) {
            if let Err(error) = store.delete(key).await {
                tracing::warn!("could not remove an unused stored file: {error:#}");
            }
        }
    }

    /// A row's bytes, from its `data` column or from the bucket.
    pub async fn read(
        &self,
        data: Option<Vec<u8>>,
        key: Option<String>,
        limit: usize,
    ) -> Result<Vec<u8>, ApiError> {
        match (data, key) {
            (Some(data), _) => Ok(data),
            (None, Some(key)) => {
                let store = self.store.as_ref().ok_or_else(|| {
                    ApiError(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "This file is kept in object storage, which is not configured".into(),
                    )
                })?;
                store.get(&key, limit).await.map_err(unavailable)
            }
            (None, None) => Err(ApiError::internal()),
        }
    }

    /// Stores a copy of a row's bytes under `key`, for a new row. If that row is never
    /// written, the copy is deleted after the grace period.
    pub async fn copy(
        &self,
        data: Option<Vec<u8>>,
        from: Option<String>,
        key: String,
        content_type: &str,
        limit: usize,
    ) -> Result<Placed, ApiError> {
        let bytes = self.read(data, from, limit).await?;
        self.place(key, bytes, content_type).await
    }

    /// Several rows' bytes in the order given, a few fetched at a time.
    pub async fn read_all(
        &self,
        rows: Vec<(Option<Vec<u8>>, Option<String>)>,
        limit: usize,
    ) -> Result<Vec<Vec<u8>>, ApiError> {
        stream::iter(rows)
            .map(|(data, key)| self.read(data, key, limit))
            .buffered(PARALLEL_READS)
            .try_collect()
            .await
    }

    /// Deletes trashed objects and moves files still held in Postgres into the bucket,
    /// until shutdown. Does nothing without a bucket.
    pub async fn worker(self, shutdown: CancellationToken) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let mut reported = false;
        loop {
            empty_trash(&self.pool, &store).await;
            let moved = match self.move_one(&store).await {
                Ok(moved) => moved,
                Err(error) => {
                    tracing::warn!("could not move a file into object storage: {error:#}");
                    false
                }
            };
            if moved && !reported {
                tracing::info!("moving files kept in Postgres into object storage");
                reported = true;
            }
            let pause = if moved {
                Duration::ZERO
            } else {
                Duration::from_secs(30)
            };
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(pause) => {}
            }
        }
    }

    /// Moves one file kept in Postgres into the bucket. False when none is left.
    async fn move_one(&self, store: &ObjectStore) -> anyhow::Result<bool> {
        for (table, key) in [
            ("attachments", attachment_key as fn(Uuid, Uuid) -> String),
            ("generated_images", image_key),
        ] {
            let mut tx = self.pool.begin().await?;
            let Some(row) = sqlx::query(&format!(
                "SELECT id, owner_id, mime_type, data FROM {table} WHERE data IS NOT NULL LIMIT 1 FOR UPDATE SKIP LOCKED"
            ))
            .fetch_optional(&mut *tx)
            .await?
            else {
                continue;
            };
            let id: Uuid = row.get("id");
            let key = key(row.get("owner_id"), id);
            put(
                &self.pool,
                store,
                &key,
                row.get("data"),
                row.get("mime_type"),
            )
            .await?;
            sqlx::query(&format!(
                "UPDATE {table} SET data = NULL, object_key = $2 WHERE id = $1"
            ))
            .bind(id)
            .bind(&key)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(true);
        }
        Ok(false)
    }
}

/// Records `key` as pending, then uploads it.
async fn put(
    pool: &PgPool,
    store: &ObjectStore,
    key: &str,
    bytes: Vec<u8>,
    content_type: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO object_trash (object_key, pending) VALUES ($1, TRUE) ON CONFLICT DO NOTHING",
    )
    .bind(key)
    .execute(pool)
    .await?;
    store.put(key, bytes, content_type).await
}

pub fn attachment_key(owner: Uuid, id: Uuid) -> String {
    format!("attachments/{owner}/{id}")
}

pub fn image_key(owner: Uuid, id: Uuid) -> String {
    format!("images/{owner}/{id}")
}

/// Deletes up to 20 trashed objects: those whose rows are gone, and pending uploads
/// no row claimed within the grace period. Rows a running transaction is claiming are
/// skipped.
pub async fn empty_trash(pool: &PgPool, store: &ObjectStore) {
    let Ok(mut tx) = pool.begin().await else {
        return;
    };
    let keys: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT object_key FROM object_trash WHERE NOT pending OR created_at < NOW() - INTERVAL '{PENDING_GRACE}'
         ORDER BY created_at LIMIT 20 FOR UPDATE SKIP LOCKED"
    ))
    .fetch_all(&mut *tx)
    .await
    .unwrap_or_default();
    for key in keys {
        if let Err(error) = store.delete(&key).await {
            tracing::warn!("could not delete a stored file: {error:#}");
            break;
        }
        let _ = sqlx::query("DELETE FROM object_trash WHERE object_key = $1")
            .bind(&key)
            .execute(&mut *tx)
            .await;
    }
    let _ = tx.commit().await;
}

fn unavailable(error: anyhow::Error) -> ApiError {
    tracing::warn!("object storage failed: {error:#}");
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "File storage is unavailable. Try again shortly.".into(),
    )
}
