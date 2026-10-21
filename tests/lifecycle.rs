use axum::http::StatusCode;
use obleth_chat::{lifecycle, store::Store};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

async fn owner(pool: &PgPool) -> Uuid {
    let owner = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Lifecycle test','user')",
    )
    .bind(owner)
    .bind(format!("lifecycle-{owner}"))
    .execute(pool)
    .await
    .unwrap();
    owner
}

#[sqlx::test(migrations = "./migrations")]
async fn checkpoints_preserve_partial_output_and_index_only_when_terminal(pool: PgPool) {
    let owner = owner(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Checkpoint", "fixture").await.unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "fixture", "", 0.7, 128)
        .await
        .unwrap();
    lifecycle::checkpoint(
        &pool,
        owner,
        chat.id,
        turn.generation_id,
        "partial searchable answer",
        "private thinking",
        Some("request"),
    )
    .await
    .unwrap();
    let saved = store.get(owner, chat.id).await.unwrap();
    assert_eq!(saved.messages[1].status, "pending");
    assert_eq!(saved.messages[1].content, "partial searchable answer");
    assert_eq!(saved.messages[1].reasoning_content, "private thinking");
    let chunks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM message_search_chunks WHERE message_id=$1")
            .bind(turn.assistant_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(chunks, 0);
    assert!(
        lifecycle::heartbeat(&pool, owner, chat.id, turn.generation_id)
            .await
            .unwrap()
            .is_none()
    );
    store
        .finish_turn_with_reasoning(
            owner,
            chat.id,
            turn.generation_id,
            "partial searchable answer",
            "private thinking",
            "interrupted",
            None,
            Some("request"),
        )
        .await
        .unwrap();
    let chunks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM message_search_chunks WHERE message_id=$1")
            .bind(turn.assistant_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(chunks > 0);
    assert!(lifecycle::checkpoint(
        &pool,
        owner,
        chat.id,
        turn.generation_id,
        "late overwrite",
        "",
        None
    )
    .await
    .is_err());
    assert_eq!(
        store.get(owner, chat.id).await.unwrap().messages[1].content,
        "partial searchable answer"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn abandoned_worker_cannot_renew_or_overwrite_replacement(pool: PgPool) {
    let owner = owner(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store
        .create(owner, "Lease fencing", "fixture")
        .await
        .unwrap();
    let old = store
        .start_turn(owner, chat.id, "First", "fixture", "", 0.7, 128)
        .await
        .unwrap();
    lifecycle::checkpoint(
        &pool,
        owner,
        chat.id,
        old.generation_id,
        "saved before failure",
        "",
        None,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE conversations SET generation_expires=NOW()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(chat.id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        lifecycle::heartbeat(&pool, owner, chat.id, old.generation_id)
            .await
            .unwrap()
            .is_some()
    );
    let recovered = store.get(owner, chat.id).await.unwrap();
    assert!(recovered.active_generation_id.is_none());
    assert_eq!(recovered.messages[1].status, "interrupted");
    assert_eq!(recovered.messages[1].content, "saved before failure");
    let next = store
        .start_turn(owner, chat.id, "Second", "fixture", "", 0.7, 128)
        .await
        .unwrap();
    assert!(lifecycle::checkpoint(
        &pool,
        owner,
        chat.id,
        old.generation_id,
        "old worker",
        "",
        None
    )
    .await
    .is_err());
    assert!(
        lifecycle::heartbeat(&pool, owner, chat.id, old.generation_id)
            .await
            .unwrap()
            .is_some()
    );
    lifecycle::checkpoint(
        &pool,
        owner,
        chat.id,
        next.generation_id,
        "new worker",
        "",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        store.get(owner, chat.id).await.unwrap().messages[3].content,
        "new worker"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn checkpoint_does_not_wait_for_a_long_user_transaction(pool: PgPool) {
    let owner = owner(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Busy owner", "fixture").await.unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "fixture", "", 0.7, 128)
        .await
        .unwrap();
    // Stands in for an export, import or search holding the owner row.
    let mut busy = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *busy)
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        lifecycle::checkpoint(
            &pool,
            owner,
            chat.id,
            turn.generation_id,
            "progress",
            "",
            None,
        ),
    )
    .await
    .expect("checkpoint waited for the user row")
    .unwrap();
    busy.rollback().await.unwrap();
    assert_eq!(
        store.get(owner, chat.id).await.unwrap().messages[1].content,
        "progress"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn checkpoint_lock_wait_is_a_transient_error(pool: PgPool) {
    let owner = owner(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Locked", "fixture").await.unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "fixture", "", 0.7, 128)
        .await
        .unwrap();
    let mut busy = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM messages WHERE id=$1 FOR UPDATE")
        .bind(turn.assistant_id)
        .execute(&mut *busy)
        .await
        .unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(3),
        lifecycle::checkpoint(
            &pool,
            owner,
            chat.id,
            turn.generation_id,
            "progress",
            "",
            None,
        ),
    )
    .await
    .expect("checkpoint lock wait was not bounded")
    .unwrap_err();
    // Not CONFLICT: the worker retries on a later tick instead of aborting.
    assert_eq!(error.0, StatusCode::INTERNAL_SERVER_ERROR);
    busy.rollback().await.unwrap();
    lifecycle::checkpoint(
        &pool,
        owner,
        chat.id,
        turn.generation_id,
        "progress",
        "",
        None,
    )
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn check_reports_stop_and_expiry_without_renewing(pool: PgPool) {
    let owner = owner(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Check", "fixture").await.unwrap();
    let turn = store
        .start_turn(owner, chat.id, "Question", "fixture", "", 0.7, 128)
        .await
        .unwrap();
    let expires = || {
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT generation_expires FROM conversations WHERE id=$1",
        )
        .bind(chat.id)
        .fetch_one(&pool)
    };
    let before = expires().await.unwrap();
    assert!(lifecycle::check(&pool, owner, chat.id, turn.generation_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(expires().await.unwrap(), before);
    assert_eq!(
        lifecycle::check(&pool, owner, chat.id, Uuid::new_v4())
            .await
            .unwrap(),
        Some("Response is no longer active")
    );
    sqlx::query("UPDATE users SET enabled=FALSE WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        lifecycle::check(&pool, owner, chat.id, turn.generation_id)
            .await
            .unwrap(),
        Some("Response is no longer active")
    );
    sqlx::query("UPDATE users SET enabled=TRUE WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE conversations SET generation_cancel_requested=TRUE WHERE id=$1")
        .bind(chat.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        lifecycle::check(&pool, owner, chat.id, turn.generation_id)
            .await
            .unwrap(),
        Some("Response stopped")
    );
    sqlx::query(
        "UPDATE conversations SET generation_cancel_requested=FALSE,generation_expires=NOW()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(chat.id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        lifecycle::check(&pool, owner, chat.id, turn.generation_id)
            .await
            .unwrap(),
        Some("Response is no longer active")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn lease_recovery_uses_the_active_index_and_renewals_avoid_indexes(pool: PgPool) {
    let indexed: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes WHERE tablename='conversations' AND indexdef LIKE '%generation_expires%'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(indexed.is_empty(), "{indexed:?}");
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN UPDATE conversations SET active_generation=NULL,generation_expires=NULL WHERE owner_id=$1 AND active_generation IS NOT NULL AND generation_expires<=clock_timestamp()",
    )
    .bind(Uuid::new_v4())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert!(
        plan.iter()
            .any(|line| line.contains("conversations_active_owner_idx")),
        "{plan:?}"
    );
}

#[test]
fn long_responses_checkpoint_less_often() {
    let tick = Duration::from_millis(750);
    assert!(!lifecycle::checkpoint_due(
        100,
        100,
        Duration::from_secs(60)
    ));
    assert!(lifecycle::checkpoint_due(20_000, 19_999, tick));
    assert!(!lifecycle::checkpoint_due(200_000, 199_000, tick));
    assert!(lifecycle::checkpoint_due(
        200_000,
        199_000,
        Duration::from_secs(3)
    ));
    assert!(lifecycle::checkpoint_due(200_000, 190_000, tick));
}
