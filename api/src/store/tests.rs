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

/// Migration 088's length checks keep the longest browser and device-call values
/// the code writes, and refuse longer ones.
#[tokio::test]
async fn text_length_checks_keep_what_the_code_writes() {
    let Some(db) = Database::before(88).await else {
        return;
    };
    let (owner, chat, connection) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO users (id, username, display_name, role) VALUES ($1, 'ana', 'Ana', 'user')",
    )
    .bind(owner)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO conversations (id, owner_id, title, model) VALUES ($1, $2, 'Device', 'm')",
    )
    .bind(chat)
    .bind(owner)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO connections (id, owner_id, name, url, kind) VALUES ($1, $2, 'Board', 'browser://device', 'device')")
        .bind(connection)
        .bind(owner)
        .execute(&db.pool)
        .await
        .unwrap();
    let browser = |slug: &str, ip: String, error: String| {
        sqlx::query("INSERT INTO browsers (id, owner_id, slug, pod_ip, error_message) VALUES ($1, $2, $3, $4, $5)")
            .bind(Uuid::new_v4())
            .bind(owner)
            .bind(slug.to_owned())
            .bind(ip)
            .bind(error)
    };
    let call = |tool: String, output: String, detail: String| {
        sqlx::query("INSERT INTO device_calls (generation_id, call_id, conversation_id, owner_id, connection_id, tool_name, ok, output, detail) VALUES ($1, 'c', $2, $3, $4, $5, TRUE, $6, $7)")
            .bind(Uuid::new_v4())
            .bind(chat)
            .bind(owner)
            .bind(connection)
            .bind(tool)
            .bind(output)
            .bind(detail)
    };
    // Rows at each limit, written before the checks exist, still validate.
    browser("a1b2c3d4e5f6g7h8", "f".repeat(64), "é".repeat(1000))
        .execute(&db.pool)
        .await
        .unwrap();
    call("t".repeat(128), "é".repeat(65536), "x".repeat(8192))
        .execute(&db.pool)
        .await
        .unwrap();
    db.migrate().await.unwrap();

    sqlx::query("DELETE FROM browsers")
        .execute(&db.pool)
        .await
        .unwrap();
    let refused = |error: sqlx::Error| {
        error
            .as_database_error()
            .unwrap()
            .constraint()
            .unwrap()
            .to_owned()
    };
    for (insert, check) in [
        (
            browser("A1B2C3D4E5F6G7H8", "ip".into(), "e".into()),
            "browsers_slug_check",
        ),
        (
            browser("a1b2c3d4e5f6g7h", "ip".into(), "e".into()),
            "browsers_slug_check",
        ),
        (
            browser("a1b2c3d4e5f6g7h8", "f".repeat(65), "e".into()),
            "browsers_pod_ip_check",
        ),
        (
            browser("a1b2c3d4e5f6g7h8", "ip".into(), "e".repeat(1001)),
            "browsers_error_message_check",
        ),
        (
            call(String::new(), "o".into(), "d".into()),
            "device_calls_tool_name_check",
        ),
        (
            call("t".repeat(129), "o".into(), "d".into()),
            "device_calls_tool_name_check",
        ),
        (
            call("read".into(), "o".repeat(65537), "d".into()),
            "device_calls_output_check",
        ),
        (
            call("read".into(), "o".into(), "d".repeat(8193)),
            "device_calls_detail_check",
        ),
    ] {
        assert_eq!(refused(insert.execute(&db.pool).await.unwrap_err()), check);
    }
    db.close().await;
}

#[test]
fn a_reply_over_the_limit_is_cut_at_the_end_of_a_character() {
    use super::{fit_response, MAX_RESPONSE_BYTES};
    // Within the limit: untouched.
    let mut short = "kept as it is".to_owned();
    fit_response(&mut short, 100);
    assert_eq!(short, "kept as it is");
    // A full reply with a note added, whose cut would fall inside a character.
    let mut full = "é".repeat(MAX_RESPONSE_BYTES / 2);
    full.push_str("\n\n_Paused._");
    fit_response(&mut full, 0);
    assert_eq!(full.len(), MAX_RESPONSE_BYTES);
    let mut beside_thinking = "é".repeat(MAX_RESPONSE_BYTES / 2);
    fit_response(&mut beside_thinking, 1001);
    assert_eq!(beside_thinking.len(), MAX_RESPONSE_BYTES - 1002);
    assert!(beside_thinking.chars().all(|c| c == 'é'));
    // Thinking alone at the limit leaves no room.
    let mut none = "text".to_owned();
    fit_response(&mut none, MAX_RESPONSE_BYTES + 5);
    assert_eq!(none, "");
}
