//! These integration tests require DATABASE_URL pointing to a disposable PostgreSQL
//! database. sqlx creates an isolated database for each test and runs all migrations.

use axum::http::StatusCode;
use obleth_chat::store::{
    ReasoningEffort, Store, MAX_CONTEXT_BYTES, MAX_CONVERSATIONS_PER_USER, MAX_CONVERSATION_BYTES,
    MAX_HISTORY, MAX_RESPONSE_BYTES,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, display_name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("user_{id}"))
        .bind("Test User")
        .execute(pool)
        .await
        .unwrap();
    id
}

#[sqlx::test(migrations = "./migrations")]
async fn every_conversation_operation_enforces_owner(pool: PgPool) {
    let alice = user(&pool).await;
    let bob = user(&pool).await;
    let store = Store::new(pool);
    let conversation = store
        .create(alice, "Alice's private chat", "test-model")
        .await
        .unwrap();
    let turn = store
        .start_turn(
            alice,
            conversation.id,
            "A private question",
            "test-model",
            "",
            0.7,
            1024,
        )
        .await
        .unwrap();
    assert!(store.list(bob).await.unwrap().is_empty());
    assert_eq!(store.list(alice).await.unwrap().len(), 1);
    assert_eq!(
        store.get(bob, conversation.id).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .rename(bob, conversation.id, "stolen")
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .update(bob, conversation.id, None, Some(ReasoningEffort::High))
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .update(alice, conversation.id, None, Some(ReasoningEffort::High))
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        store.delete(bob, conversation.id).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .branch(bob, conversation.id, turn.messages[0].id)
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .start_turn(bob, conversation.id, "attack", "test-model", "", 0.7, 1024)
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .finish_turn(
                bob,
                conversation.id,
                turn.generation_id,
                "attack",
                "completed",
                None,
                None
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .interrupt_turn(bob, conversation.id, turn.generation_id, "attack")
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store.get(alice, conversation.id).await.unwrap().messages[1].status,
        "pending"
    );
    store
        .finish_turn(
            alice,
            conversation.id,
            turn.generation_id,
            "Private answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    store
        .rename(alice, conversation.id, "Renamed")
        .await
        .unwrap();
    assert_eq!(
        store.get(alice, conversation.id).await.unwrap().title,
        "Renamed"
    );
    store.delete(alice, conversation.id).await.unwrap();
    assert!(store.list(alice).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn reasoning_counts_toward_combined_response_and_history_storage_bounds(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store
        .create(owner, "Reasoning budget", "model")
        .await
        .unwrap();
    assert_eq!(chat.reasoning_effort, "medium");
    for status in ["completed", "interrupted", "error"] {
        let turn = store
            .start_turn_with_effort(
                owner,
                chat.id,
                "Question",
                "model",
                "",
                1.0,
                512,
                Some(ReasoningEffort::High),
            )
            .await
            .unwrap();
        // Both text channels share a single response allowance, not one each.
        let error = store
            .finish_turn_with_reasoning(
                owner,
                chat.id,
                turn.generation_id,
                "Answer",
                &"r".repeat(MAX_RESPONSE_BYTES),
                status,
                None,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(error.1.contains("1 MiB"));
        assert_eq!(
            store
                .get(owner, chat.id)
                .await
                .unwrap()
                .messages
                .last()
                .unwrap()
                .status,
            "pending"
        );
        store
            .finish_turn_with_reasoning(
                owner,
                chat.id,
                turn.generation_id,
                "",
                &"r".repeat(MAX_RESPONSE_BYTES),
                status,
                None,
                None,
            )
            .await
            .unwrap();
    }
    let saved = store.get(owner, chat.id).await.unwrap();
    assert_eq!(saved.reasoning_effort, "high");
    assert_eq!(saved.messages.len(), 6);
    assert_eq!(
        saved.messages[1].reasoning_content.len(),
        MAX_RESPONSE_BYTES
    );
    let error = store
        .start_turn(owner, chat.id, "Next", "model", "", 1.0, 512)
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("4 MiB"));
    assert_eq!(store.get(owner, chat.id).await.unwrap().messages.len(), 6);
    let branched = store
        .branch(owner, chat.id, saved.messages[1].id)
        .await
        .unwrap();
    assert_eq!(branched.reasoning_effort, "high");
    assert_eq!(
        branched.messages[1].reasoning_content,
        saved.messages[1].reasoning_content
    );
    // Reasoning isn't model context: a large trace still allows a small follow-up.
    let turn = store
        .start_turn(owner, branched.id, "Follow up", "model", "", 1.0, 512)
        .await
        .unwrap();
    assert_eq!(turn.reasoning_effort, "high");
    // A reasoning-only answer is retained for display, but is not an empty
    // assistant turn in the next provider request.
    assert_eq!(turn.messages.len(), 2);
    assert_eq!(turn.messages[1].content, "Follow up");
    assert!(turn
        .messages
        .iter()
        .all(|message| message.reasoning_content.is_empty()));
}

#[sqlx::test(migrations = "./migrations")]
async fn reasoning_migration_defaults_and_database_constraints_protect_legacy_writers(
    pool: PgPool,
) {
    let owner = user(&pool).await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversations (id, owner_id, title, model) VALUES ($1, $2, 'Legacy', 'model')",
    )
    .bind(id)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    let message_id = Uuid::new_v4();
    sqlx::query("INSERT INTO messages (id, conversation_id, ordinal, role, content, status) VALUES ($1, $2, 0, 'user', 'Legacy question', 'completed')").bind(message_id).bind(id).execute(&pool).await.unwrap();
    let store = Store::new(pool.clone());
    let saved = store.get(owner, id).await.unwrap();
    assert_eq!(saved.reasoning_effort, "medium");
    assert_eq!(saved.messages[0].reasoning_content, "");
    assert!(
        sqlx::query("UPDATE conversations SET reasoning_effort = 'invalid' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "UPDATE messages SET reasoning_content = 'Not assistant reasoning' WHERE id = $1"
    )
    .bind(message_id)
    .execute(&pool)
    .await
    .is_err());
    let turn = store
        .start_turn(owner, id, "Next", "model", "", 1.0, 512)
        .await
        .unwrap();
    assert!(sqlx::query(
        "UPDATE messages SET content = 'Answer', reasoning_content = $2 WHERE id = $1"
    )
    .bind(turn.assistant_id)
    .bind("r".repeat(MAX_RESPONSE_BYTES))
    .execute(&pool)
    .await
    .is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn only_one_replica_can_start_a_conversation_generation(pool: PgPool) {
    let owner = user(&pool).await;
    let first_replica = Store::new(pool.clone());
    let second_replica = Store::new(pool);
    let chat = first_replica
        .create(owner, "Concurrent", "model")
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        first_replica.start_turn(owner, chat.id, "First", "model", "", 1.0, 512),
        second_replica.start_turn(owner, chat.id, "Second", "model", "", 1.0, 512),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let rejected = if first.is_err() {
        first.unwrap_err()
    } else {
        second.unwrap_err()
    };
    assert_eq!(rejected.0, StatusCode::CONFLICT);
    let saved = first_replica.get(owner, chat.id).await.unwrap();
    assert_eq!(
        saved.messages.len(),
        2,
        "Rejected start must not append a user message"
    );
    assert_eq!(saved.messages[1].status, "pending");
}

#[sqlx::test(migrations = "./migrations")]
async fn active_generation_cap_is_atomic_across_replicas(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let mut chats = Vec::new();
    for number in 0..8 {
        chats.push(
            store
                .create(owner, &format!("Chat {number}"), "model")
                .await
                .unwrap(),
        );
    }
    let mut tasks = tokio::task::JoinSet::new();
    for chat in &chats {
        let replica = Store::new(pool.clone());
        let id = chat.id;
        tasks.spawn(async move {
            replica
                .start_turn(owner, id, "Hello", "model", "", 1.0, 512)
                .await
        });
    }
    let mut allowed = 0;
    let mut rejected = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            Ok(_) => allowed += 1,
            Err(error) => {
                assert_eq!(error.0, StatusCode::TOO_MANY_REQUESTS);
                rejected += 1;
            }
        }
    }
    assert_eq!((allowed, rejected), (3, 5));
    let other_user = user(&pool).await;
    let other_chat = store
        .create(other_user, "Other user", "model")
        .await
        .unwrap();
    store
        .start_turn(other_user, other_chat.id, "Hello", "model", "", 1.0, 512)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn completed_conversations_survive_database_reconnect(pool: PgPool) {
    let owner = user(&pool).await;
    let options = (*pool.connect_options()).clone();
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Persistent", "model").await.unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "model", "Be helpful", 0.5, 512)
        .await
        .unwrap();
    store
        .finish_turn(
            owner,
            chat.id,
            turn.generation_id,
            "Answer",
            "completed",
            Some(json!({"total_tokens": 9})),
            Some("gateway-request-123"),
        )
        .await
        .unwrap();
    drop(store);
    pool.close().await;
    let reopened = PgPool::connect_with(options).await.unwrap();
    let store = Store::new(reopened.clone());
    let saved = store.get(owner, chat.id).await.unwrap();
    assert_eq!(saved.title, "Persistent");
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[1].id, turn.assistant_id);
    assert_eq!(saved.messages[1].content, "Answer");
    assert_eq!(saved.messages[1].status, "completed");
    assert_eq!(saved.messages[1].usage, Some(json!({"total_tokens": 9})));
    assert_eq!(
        saved.messages[1].request_id.as_deref(),
        Some("gateway-request-123")
    );
    let serialized = serde_json::to_value(saved).unwrap();
    assert!(serialized.get("updatedAt").is_some());
    assert_eq!(
        serialized["messages"][1]["requestId"],
        "gateway-request-123"
    );
    reopened.close().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_generation_is_interrupted_and_late_writer_cannot_overwrite_new_turn(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Expired", "model").await.unwrap();
    let expired = store
        .start_turn(owner, chat.id, "First question", "model", "", 1.0, 512)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE conversations SET generation_expires = NOW() - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(chat.id)
    .execute(&pool)
    .await
    .unwrap();
    let replacement = store
        .start_turn(owner, chat.id, "Second question", "model", "", 1.0, 512)
        .await
        .unwrap();
    assert_eq!(replacement.messages.len(), 2);
    assert!(replacement
        .messages
        .iter()
        .all(|message| message.role == "user"));
    let late = store
        .finish_turn(
            owner,
            chat.id,
            expired.generation_id,
            "Late answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(late.0, StatusCode::CONFLICT);
    let saved = store.get(owner, chat.id).await.unwrap();
    assert_eq!(saved.messages[1].status, "interrupted");
    assert_eq!(saved.messages[1].content, "");
    assert_eq!(saved.messages[3].status, "pending");
    store
        .finish_turn(
            owner,
            chat.id,
            replacement.generation_id,
            "Current answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store.get(owner, chat.id).await.unwrap().messages[3].content,
        "Current answer"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn late_completion_commits_expiry_recovery(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Expired", "model").await.unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "model", "", 1.0, 512)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE conversations SET generation_expires = NOW() - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(chat.id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .finish_turn(
                owner,
                chat.id,
                turn.generation_id,
                "Too late",
                "completed",
                None,
                None
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    let status: String = sqlx::query_scalar("SELECT status FROM messages WHERE id = $1")
        .bind(turn.assistant_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "interrupted");
}

#[sqlx::test(migrations = "./migrations")]
async fn branching_copies_only_selected_prefix_with_independent_ids(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool);
    let chat = store.create(owner, "Original", "model").await.unwrap();
    let first = store
        .start_turn(owner, chat.id, "First", "model", "", 1.0, 512)
        .await
        .unwrap();
    store
        .finish_turn(
            owner,
            chat.id,
            first.generation_id,
            "First answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let second = store
        .start_turn(owner, chat.id, "Second", "model", "", 1.0, 512)
        .await
        .unwrap();
    assert_eq!(
        store
            .branch(owner, chat.id, second.assistant_id)
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    let branch = store
        .branch(owner, chat.id, first.assistant_id)
        .await
        .unwrap();
    assert_ne!(branch.id, chat.id);
    assert_eq!(branch.messages.len(), 2);
    assert_ne!(branch.messages[1].id, first.assistant_id);
    assert_eq!(branch.messages[1].content, "First answer");
    assert_eq!(store.get(owner, chat.id).await.unwrap().messages.len(), 4);
    store
        .start_turn(owner, branch.id, "Alternative", "model", "", 1.0, 512)
        .await
        .unwrap();
    assert_eq!(
        store.delete(owner, chat.id).await.unwrap_err().0,
        StatusCode::CONFLICT
    );
    store
        .interrupt_turn(owner, chat.id, second.generation_id, "")
        .await
        .unwrap();
    store.delete(owner, chat.id).await.unwrap();
    assert_eq!(store.get(owner, branch.id).await.unwrap().messages.len(), 4);
}

#[sqlx::test(migrations = "./migrations")]
async fn interrupted_partial_content_is_saved_but_never_used_for_inference(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool);
    let chat = store.create(owner, "Interrupted", "model").await.unwrap();
    let first = store
        .start_turn(owner, chat.id, "Question", "model", "", 1.0, 512)
        .await
        .unwrap();
    store
        .interrupt_turn(owner, chat.id, first.generation_id, "Half an ans")
        .await
        .unwrap();
    let saved = store.get(owner, chat.id).await.unwrap();
    assert_eq!(saved.messages[1].content, "Half an ans");
    assert_eq!(saved.messages[1].status, "interrupted");
    let second = store
        .start_turn(owner, chat.id, "Continue", "model", "", 1.0, 512)
        .await
        .unwrap();
    assert!(second
        .messages
        .iter()
        .all(|message| message.content != "Half an ans"));
    assert_eq!(second.messages.last().unwrap().content, "Continue");
}

#[sqlx::test(migrations = "./migrations")]
async fn context_and_history_limits_reject_without_appending_messages(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Large context", "model").await.unwrap();
    let first = store
        .start_turn(owner, chat.id, "Question", "model", "", 1.0, 512)
        .await
        .unwrap();
    store
        .finish_turn(
            owner,
            chat.id,
            first.generation_id,
            &"x".repeat(MAX_CONTEXT_BYTES),
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let error = store
        .start_turn(owner, chat.id, "Next", "model", "", 1.0, 512)
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("100 KiB"));
    assert_eq!(store.get(owner, chat.id).await.unwrap().messages.len(), 2);
    let full = store.create(owner, "Full history", "model").await.unwrap();
    for ordinal in 0..MAX_HISTORY {
        sqlx::query("INSERT INTO messages (id, conversation_id, ordinal, role, content, status) VALUES ($1, $2, $3, 'user', 'x', 'completed')")
            .bind(Uuid::new_v4()).bind(full.id).bind(ordinal as i32).execute(&pool).await.unwrap();
    }
    let error = store
        .start_turn(owner, full.id, "Next", "model", "", 1.0, 512)
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("200 messages"));
    assert_eq!(
        store.get(owner, full.id).await.unwrap().messages.len(),
        MAX_HISTORY
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn conversation_cap_is_explicit_and_applies_to_create_and_branch(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Original", "model").await.unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "model", "", 1.0, 512)
        .await
        .unwrap();
    store
        .finish_turn(
            owner,
            chat.id,
            turn.generation_id,
            "Answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO conversations (id, owner_id, title, model)
         SELECT gen_random_uuid(), $1, 'History', 'model' FROM generate_series(1, $2)",
    )
    .bind(owner)
    .bind(MAX_CONVERSATIONS_PER_USER - 1)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store.list(owner).await.unwrap().len(),
        MAX_CONVERSATIONS_PER_USER as usize
    );
    let create_error = store.create(owner, "Over cap", "model").await.unwrap_err();
    assert_eq!(create_error.0, StatusCode::BAD_REQUEST);
    assert!(create_error.1.contains("1,000 conversations"));
    let branch_error = store
        .branch(owner, chat.id, turn.assistant_id)
        .await
        .unwrap_err();
    assert_eq!(branch_error.0, StatusCode::BAD_REQUEST);
    assert!(branch_error.1.contains("1,000 conversations"));
    let other = user(&pool).await;
    store
        .create(other, "Other user's conversation", "model")
        .await
        .unwrap();
    store.delete(owner, chat.id).await.unwrap();
    store.create(owner, "Slot released", "model").await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn disabling_an_account_blocks_access_but_allows_generation_cleanup(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store
        .create(owner, "Disable while generating", "model")
        .await
        .unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "model", "", 1.0, 512)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET enabled = FALSE WHERE id = $1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.get(owner, chat.id).await.unwrap_err().0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        store
            .start_turn(owner, chat.id, "Question", "model", "", 1.0, 512)
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
    store
        .interrupt_turn(owner, chat.id, turn.generation_id, "Partial reply")
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM messages WHERE id = $1")
        .bind(turn.assistant_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "interrupted");
    let active: Option<Uuid> =
        sqlx::query_scalar("SELECT active_generation FROM conversations WHERE id = $1")
            .bind(chat.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(active, None);
}

#[sqlx::test(migrations = "./migrations")]
async fn failed_history_counts_toward_storage_limit_and_reserves_the_next_response(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool);
    let chat = store
        .create(owner, "Repeated partial replies", "model")
        .await
        .unwrap();
    for status in ["error", "interrupted", "error"] {
        let turn = store
            .start_turn(owner, chat.id, "Question", "model", "", 1.0, 512)
            .await
            .unwrap();
        store
            .finish_turn(
                owner,
                chat.id,
                turn.generation_id,
                &"x".repeat(MAX_RESPONSE_BYTES),
                status,
                None,
                None,
            )
            .await
            .unwrap();
    }
    let before = store.get(owner, chat.id).await.unwrap();
    let stored_bytes = before
        .messages
        .iter()
        .map(|message| message.content.len())
        .sum::<usize>();
    assert!(stored_bytes < MAX_CONVERSATION_BYTES);
    assert!(stored_bytes + "Another question".len() + MAX_RESPONSE_BYTES > MAX_CONVERSATION_BYTES);
    // All assistant replies are excluded from inference, so this limit must be
    // checked against persisted history before completed-context filtering.
    let error = store
        .start_turn(owner, chat.id, "Another question", "model", "", 1.0, 512)
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("4 MiB"));
    let after = store.get(owner, chat.id).await.unwrap();
    assert_eq!(
        after.messages.len(),
        6,
        "A rejected turn must not append a user or assistant message"
    );
    assert_eq!(after.messages[1].status, "error");
    assert_eq!(after.messages[3].status, "interrupted");
    assert_eq!(after.messages[5].content.len(), MAX_RESPONSE_BYTES);
}

#[sqlx::test(migrations = "./migrations")]
async fn stopped_workspace_replies_are_replayed_with_their_steps(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Workspace", "model").await.unwrap();
    let first = store
        .start_turn(
            owner,
            chat.id,
            "Build a pi calculator",
            "model",
            "",
            1.0,
            512,
        )
        .await
        .unwrap();
    obleth_chat::lifecycle::save_activity(
        &pool,
        owner,
        chat.id,
        first.generation_id,
        &[json!({"summary":"Ran cargo build","status":"error","detail":"exit 101"})],
    )
    .await
    .unwrap();
    store
        .finish_turn_detailed(
            owner,
            chat.id,
            first.generation_id,
            "Building it now.",
            "",
            "error",
            None,
            None,
            None,
            Some("Generation reached the 15-minute time limit"),
        )
        .await
        .unwrap();
    let second = store
        .start_turn(owner, chat.id, "continue", "model", "", 1.0, 512)
        .await
        .unwrap();
    let replayed = second
        .messages
        .iter()
        .find(|m| m.role == "assistant")
        .map(obleth_chat::store::replay_content)
        .expect("the stopped workspace reply is replayed");
    assert!(replayed.contains("- Ran cargo build (error): exit 101"));
    assert!(replayed.contains("stopped before finishing: Generation reached"));
    assert_eq!(second.messages.last().unwrap().content, "continue");
}
