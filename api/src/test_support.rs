//! Helpers for tests that need Postgres. Each test gets its own schema in the
//! database TEST_DATABASE_URL names, with every migration applied, and drops it
//! when done, so tests run side by side and leave nothing behind.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::env;
use uuid::Uuid;

static VECTOR: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

pub struct Database {
    pub pool: PgPool,
    admin: PgPool,
    schema: String,
}
impl Database {
    /// A pool on its own schema with every migration applied, or None without
    /// TEST_DATABASE_URL (the test then skips).
    pub async fn new() -> Option<Self> {
        let Ok(url) = env::var("TEST_DATABASE_URL") else {
            eprintln!("Skipping Postgres test: set TEST_DATABASE_URL to run");
            return None;
        };
        let schema = format!("test_{}", Uuid::new_v4().simple());
        let admin = PgPool::connect(&url).await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        // pgvector's types live in public; the extension is made once per database.
        VECTOR
            .get_or_init(|| async {
                sqlx::query("CREATE EXTENSION IF NOT EXISTS vector SCHEMA public")
                    .execute(&admin)
                    .await
                    .unwrap();
            })
            .await;
        let search_path = format!("SET search_path TO {schema}, public");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |conn, _| {
                let statement = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&statement).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        // Every migration, so handlers that touch other features' tables (deleting a
        // group clears its shares, for one) work as they do in production.
        crate::MIGRATOR.run(&pool).await.unwrap();
        Some(Self {
            pool,
            admin,
            schema,
        })
    }
    /// Drops the schema; call it at the end of a test.
    pub async fn close(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}
