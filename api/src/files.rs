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

use crate::{error::ApiError, knowledge::ObjectStore, worker};

/// How long an uploaded object may wait for the row that names it.
const PENDING_GRACE: &str = "15 minutes";
/// Objects read at once for one request.
const PARALLEL_READS: usize = 8;
/// How long the worker waits after a pass that moved nothing, doubling per idle
/// pass up to FILES_IDLE_MAX: once every file is in the bucket there is nothing to
/// do, and the trash grace is long enough that a five-minute check keeps up.
fn idle_pause(idle_passes: u32) -> Duration {
    worker::FILES_IDLE
        .saturating_mul(1 << idle_passes.saturating_sub(1).min(4))
        .min(worker::FILES_IDLE_MAX)
}

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
        self.store_under(key, bytes, content_type, None).await
    }

    /// `place` for a caller that already has the bytes' SHA-256, so the bucket's
    /// request is signed with it instead of hashing the bytes again.
    pub async fn place_hashed(
        &self,
        key: String,
        bytes: Vec<u8>,
        content_type: &str,
        sha256: &[u8],
    ) -> Result<Placed, ApiError> {
        self.store_under(key, bytes, content_type, Some(sha256))
            .await
    }

    async fn store_under(
        &self,
        key: String,
        bytes: Vec<u8>,
        content_type: &str,
        sha256: Option<&[u8]>,
    ) -> Result<Placed, ApiError> {
        let Some(store) = &self.store else {
            return Ok(Placed {
                data: Some(bytes),
                key: None,
            });
        };
        put(&self.pool, store, &key, bytes, content_type, sha256)
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

    /// A row's bytes as a response body. From the bucket they are streamed, so a large
    /// download is never held in memory.
    pub async fn body(
        &self,
        data: Option<Vec<u8>>,
        key: Option<String>,
        limit: usize,
    ) -> Result<axum::body::Body, ApiError> {
        match (data, key, &self.store) {
            (None, Some(key), Some(store)) => {
                let stream = store.get_stream(&key, limit).await.map_err(unavailable)?;
                Ok(axum::body::Body::from_stream(stream))
            }
            (data, key, _) => Ok(self.read(data, key, limit).await?.into()),
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
        let mut idle_passes = 0u32;
        loop {
            expire_job_output(&self.pool).await;
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
            idle_passes = if moved { 0 } else { idle_passes + 1 };
            let pause = if moved {
                Duration::ZERO
            } else {
                idle_pause(idle_passes)
            };
            if !worker::pause(&shutdown, pause, None).await {
                return;
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
                None,
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
    sha256: Option<&[u8]>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO object_trash (object_key, pending) VALUES ($1, TRUE) ON CONFLICT DO NOTHING",
    )
    .bind(key)
    .execute(pool)
    .await?;
    match sha256 {
        Some(sha256) => store.put_hashed(key, bytes, content_type, sha256).await,
        None => store.put(key, bytes, content_type).await,
    }
}

pub fn attachment_key(owner: Uuid, id: Uuid) -> String {
    format!("attachments/{owner}/{id}")
}

pub fn image_key(owner: Uuid, id: Uuid) -> String {
    format!("images/{owner}/{id}")
}

/// How a stored file is served: the type to send, and whether a browser may show it in
/// place. Only PDFs and common raster images open inline. Text of any kind goes out as
/// plain text and everything else as opaque bytes, both as downloads, so an uploaded
/// HTML, XML or SVG file never runs as a page. The stored type is only a hint.
pub fn served_type(mime: &str) -> (&'static str, bool) {
    let essence = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "application/pdf" => ("application/pdf", true),
        "image/png" => ("image/png", true),
        "image/jpeg" => ("image/jpeg", true),
        "image/gif" => ("image/gif", true),
        "image/webp" => ("image/webp", true),
        e if e.starts_with("text/")
            || e.ends_with("+json")
            || e.ends_with("+xml")
            || [
                "application/json",
                "application/xml",
                "application/javascript",
                "application/ecmascript",
                "application/typescript",
                "application/x-yaml",
                "application/yaml",
                "application/toml",
                "application/x-sh",
                "application/sql",
                "application/x-ndjson",
            ]
            .contains(&e) =>
        {
            ("text/plain; charset=utf-8", false)
        }
        _ => ("application/octet-stream", false),
    }
}

/// Job output that Slurm clusters bring back is kept 30 days; deleting the row trashes
/// its object.
async fn expire_job_output(pool: &PgPool) {
    if let Err(error) =
        sqlx::query("DELETE FROM slurm_job_outputs WHERE created_at < NOW() - INTERVAL '30 days'")
            .execute(pool)
            .await
    {
        tracing::warn!("could not expire job output: {error}");
    }
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

#[cfg(test)]
mod tests {
    use super::{empty_trash, Files};
    use super::{idle_pause, served_type};
    use crate::knowledge::storage::stub;
    use crate::test_support::Database;
    use axum::http::Method;
    use uuid::Uuid;

    /// A knowledge object whose document row never commits (the server died in
    /// between) is swept after the grace period; one whose row is written is claimed
    /// by the row's trigger and kept.
    #[tokio::test]
    async fn an_object_whose_row_never_commits_is_swept_and_a_recorded_one_is_kept() {
        let Some(db) = Database::new().await else {
            return;
        };
        let (store, seen) = stub::bucket().await;
        let files = Files::new(db.pool.clone(), Some(store.clone()));
        let owner = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, username, display_name, role, password_hash) VALUES ($1, 'ana', 'Ana', 'user', 'x')")
            .bind(owner).execute(&db.pool).await.unwrap();
        let collection = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO knowledge_collections (id, owner_id, name) VALUES ($1, $2, 'Papers')",
        )
        .bind(collection)
        .bind(owner)
        .execute(&db.pool)
        .await
        .unwrap();

        let lost = format!("knowledge/{owner}/lost");
        let kept = format!("knowledge/{owner}/kept");
        for key in [&lost, &kept] {
            files
                .place(key.clone(), b"hello".to_vec(), "text/plain")
                .await
                .unwrap();
        }
        let pending: Vec<String> = sqlx::query_scalar(
            "SELECT object_key FROM object_trash WHERE pending ORDER BY object_key",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(pending, vec![kept.clone(), lost.clone()]);

        // The kept one's row is written: the trigger claims the object.
        sqlx::query("INSERT INTO knowledge_documents (id, collection_id, owner_id, name, mime_type, size, object_key) VALUES ($1, $2, $3, 'kept.txt', 'text/plain', 5, $4)")
            .bind(Uuid::new_v4()).bind(collection).bind(owner).bind(&kept).execute(&db.pool).await.unwrap();
        // The lost one's row never comes; the grace period passes.
        sqlx::query(
            "UPDATE object_trash SET created_at = NOW() - INTERVAL '1 hour' WHERE object_key = $1",
        )
        .bind(&lost)
        .execute(&db.pool)
        .await
        .unwrap();
        empty_trash(&db.pool, &store).await;

        let deleted: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| *m == Method::DELETE)
            .map(|(_, k)| k.clone())
            .collect();
        assert_eq!(deleted, vec![lost.clone()]);
        let left: Vec<String> = sqlx::query_scalar("SELECT object_key FROM object_trash")
            .fetch_all(&db.pool)
            .await
            .unwrap();
        assert!(left.is_empty(), "{left:?}");
        db.close().await;
    }

    #[test]
    fn idle_pause_backs_off_to_five_minutes() {
        let seconds: Vec<u64> = (1..=6).map(|n| idle_pause(n).as_secs()).collect();
        assert_eq!(seconds, [30, 60, 120, 240, 300, 300]);
    }

    #[test]
    fn only_documents_and_images_are_served_inline() {
        for (stored, served, inline) in [
            ("application/pdf", "application/pdf", true),
            ("IMAGE/PNG", "image/png", true),
            ("image/webp; q=1", "image/webp", true),
            (
                "text/html; charset=utf-8",
                "text/plain; charset=utf-8",
                false,
            ),
            ("application/xhtml+xml", "text/plain; charset=utf-8", false),
            ("image/svg+xml", "text/plain; charset=utf-8", false),
            ("image/svg+xml; x=json", "text/plain; charset=utf-8", false),
            ("application/json", "text/plain; charset=utf-8", false),
            ("text/markdown", "text/plain; charset=utf-8", false),
            (
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "application/octet-stream",
                false,
            ),
            (
                "application/x-shockwave-flash",
                "application/octet-stream",
                false,
            ),
            ("", "application/octet-stream", false),
        ] {
            assert_eq!(served_type(stored), (served, inline), "{stored}");
        }
    }
}
