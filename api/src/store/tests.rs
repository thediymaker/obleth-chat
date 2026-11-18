//! Postgres-backed store tests; they skip without TEST_DATABASE_URL.

use crate::test_support::Database;
use uuid::Uuid;

/// Every ON DELETE SET NULL column that points at messages, and the file-moving
/// worker's "still holds its bytes" check, are answered from an index, and a chat
/// with a long message chain deletes within the retention worker's statement
/// timeout.
#[tokio::test]
async fn deleting_a_chat_finds_message_links_through_indexes() {
    let Some(db) = Database::new().await else {
        return;
    };
    let owner = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, role) VALUES ($1, 'ana', 'Ana', 'user')",
    )
    .bind(owner)
    .execute(&db.pool)
    .await
    .unwrap();
    let chat = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversations (id, owner_id, title, model) VALUES ($1, $2, 'Links', 'm')",
    )
    .bind(chat)
    .bind(owner)
    .execute(&db.pool)
    .await
    .unwrap();
    // A chain of messages: each points back at its parent, and forward at the
    // child that is selected, once that child exists.
    let ids: Vec<Uuid> = (0..150).map(|_| Uuid::new_v4()).collect();
    for (ordinal, id) in ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, ordinal, role, content, status, parent_message_id) \
             VALUES ($1, $2, $3, 'user', 'm', 'completed', $4)",
        )
        .bind(id)
        .bind(chat)
        .bind(ordinal as i32)
        .bind(ordinal.checked_sub(1).map(|i| ids[i]))
        .execute(&db.pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE messages m SET selected_child_id = c.id FROM messages c WHERE c.parent_message_id = m.id AND m.conversation_id = $1")
        .bind(chat)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE conversations SET selected_leaf_id = $2 WHERE id = $1")
        .bind(chat)
        .bind(ids[ids.len() - 1])
        .execute(&db.pool)
        .await
        .unwrap();

    // With sequential scans priced out, a table without a usable index still plans
    // one, which is how the missing indexes were found in the first place.
    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .unwrap();
    for (table, column) in [
        ("messages", "parent_message_id"),
        ("messages", "selected_child_id"),
        ("conversations", "selected_leaf_id"),
        ("saved_items", "source_message_id"),
        ("memories", "source_message_id"),
    ] {
        let plan: Vec<String> = sqlx::query_scalar(&format!(
            "EXPLAIN SELECT 1 FROM {table} WHERE {column} = $1"
        ))
        .bind(ids[0])
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert!(
            plan.iter().any(|line| line.contains("Index")),
            "{table}.{column} is checked by {plan:?}"
        );
    }
    for table in ["attachments", "generated_images"] {
        let plan: Vec<String> = sqlx::query_scalar(&format!(
            "EXPLAIN SELECT id FROM {table} WHERE data IS NOT NULL LIMIT 1"
        ))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert!(
            plan.iter().any(|line| line.contains("Index")),
            "{table} unmoved rows are found by {plan:?}"
        );
    }
    drop(conn);

    sqlx::query("SET statement_timeout = '10s'")
        .execute(&db.pool)
        .await
        .unwrap();
    let deleted = sqlx::query("DELETE FROM conversations WHERE id = $1 AND owner_id = $2")
        .bind(chat)
        .bind(owner)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(deleted.rows_affected(), 1);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE conversation_id = $1")
        .bind(chat)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    db.close().await;
}
