//! Real transaction tests against isolated SQLx databases. Usage comes only from
//! admitted Chat generations, never from client-authored/imported message usage.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    operations,
    store::Store,
    AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
struct Session {
    id: Uuid,
    cookie: String,
    csrf: String,
}
async fn session(pool: &PgPool, role: &str) -> Session {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Test user',$3)")
        .bind(id)
        .bind(format!("test_{}", id.simple()))
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()));
    sqlx::query("INSERT INTO auth_sessions(token_hash,user_id,expires_at)VALUES($1,$2,NOW()+INTERVAL '1 hour')")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(pool).await.unwrap();
    let csrf = URL_SAFE_NO_PAD.encode(Sha256::digest(
        format!("obleth-chat-csrf:{token}").as_bytes(),
    ));
    Session {
        id,
        cookie: format!("obleth_chat_session={token}"),
        csrf,
    }
}
async fn fixture(pool: &PgPool) -> Router {
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: ORIGIN.into(),
            session_hours: 12,
            bootstrap_username: None,
            bootstrap_password: None,
            oidc: None,
            login_rate_per_minute: 1000,
        },
    )
    .await
    .unwrap();
    operations::router(AppState::new(auth, pool.clone(), None, 4))
}
async fn call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let result = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("origin", ORIGIN)
                .header("cookie", &user.cookie)
                .header("x-csrf-token", &user.csrf)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = result.status();
    let bytes = result.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn set_limits(pool: &PgPool, limits: Value) {
    sqlx::query("UPDATE site_limits SET limits=$1 WHERE id=TRUE")
        .bind(limits)
        .execute(pool)
        .await
        .unwrap();
}
async fn ledger_count(pool: &PgPool, owner: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM generation_usage WHERE user_id=$1")
        .bind(owner)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn optional_follow_ups_are_visible_in_personal_usage_and_reservations(pool: PgPool) {
    let app = fixture(&pool).await;
    let user = session(&pool, "user").await;
    let generation = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    operations::reserve_follow_up_in(&mut tx, user.id, generation, 256, 300)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, active) = call(&app, &user, "GET", "/api/usage", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["today"]["requests"], 1);
    assert_eq!(active["today"]["activeGenerations"], 1);
    assert_eq!(active["today"]["reservedTokens"], 256);
    assert_eq!(active["today"]["incompleteUsage"], 0);
    assert!(active["quotaBasis"].as_str().unwrap().contains("follow-up"));
    let mut tx = pool.begin().await.unwrap();
    operations::finish_in(
        &mut tx,
        generation,
        Some(&json!({"prompt_tokens":90,"completion_tokens":25,"total_tokens":115})),
        "completed",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (_, finished) = call(&app, &user, "GET", "/api/usage", Value::Null).await;
    assert_eq!(finished["today"]["totalTokens"], 115);
    assert_eq!(finished["today"]["activeGenerations"], 0);
    assert_eq!(finished["today"]["reservedTokens"], 0);
    assert_eq!(finished["daily"][0]["totalTokens"], 115);
}

#[sqlx::test(migrations = "./migrations")]
async fn store_accounts_actual_generations_once_and_import_cannot_add_usage(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let bob = session(&pool, "user").await;
    let store = Store::new(pool.clone());
    let chat = store
        .create(alice.id, "Original", "test-model")
        .await
        .unwrap();
    let turn = store
        .start_turn(alice.id, chat.id, "Hello", "test-model", "", 0.7, 200)
        .await
        .unwrap();
    let (_, active) = call(&app, &alice, "GET", "/api/usage", Value::Null).await;
    assert_eq!(active["today"]["requests"], 1);
    assert_eq!(active["today"]["activeGenerations"], 1);
    assert_eq!(active["today"]["reservedTokens"], 200);
    assert_eq!(active["daily"][0]["reservedTokens"], 200);
    let usage = json!({"prompt_tokens":30,"completion_tokens":20,"total_tokens":50});
    store
        .finish_turn(
            alice.id,
            chat.id,
            turn.generation_id,
            "Hello back",
            "completed",
            Some(usage.clone()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .finish_turn(
                alice.id,
                chat.id,
                turn.generation_id,
                "Duplicate",
                "completed",
                Some(usage),
                None
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    let archive = store.export(alice.id, &[chat.id]).await.unwrap();
    store.import(bob.id, archive).await.unwrap();
    assert_eq!(ledger_count(&pool, bob.id).await, 0);
    let (_, bob_usage) = call(&app, &bob, "GET", "/api/usage", Value::Null).await;
    assert_eq!(bob_usage["totals"]["totalTokens"], 0);
    // UTC day boundaries apply to accepted request time, not conversation edits.
    sqlx::query("UPDATE generation_usage SET created_at=(date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')-INTERVAL '1 hour' WHERE generation_id=$1").bind(turn.generation_id).execute(&pool).await.unwrap();
    let error = store
        .start_turn(alice.id, chat.id, "Again", "test-model", "", 0.7, 100)
        .await
        .unwrap();
    store
        .finish_turn(
            alice.id,
            chat.id,
            error.generation_id,
            "",
            "error",
            None,
            None,
        )
        .await
        .unwrap();
    let (status, report) = call(&app, &alice, "GET", "/api/usage", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["today"]["requests"], 1);
    assert_eq!(report["today"]["totalTokens"], 0);
    assert_eq!(report["totals"]["requests"], 2);
    assert_eq!(report["totals"]["totalTokens"], 50);
    assert_eq!(report["totals"]["errors"], 1);
    assert_eq!(report["totals"]["incompleteUsage"], 1);
    assert_eq!(report["totals"]["reservedTokens"], 0);
    assert_eq!(report["daily"].as_array().unwrap().len(), 2);
    store.delete(alice.id, chat.id).await.unwrap();
    assert_eq!(
        ledger_count(&pool, alice.id).await,
        2,
        "deleting a chat must not reset its quota"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn admission_is_atomic_across_concurrent_store_requests(pool: PgPool) {
    let owner = session(&pool, "user").await.id;
    let store = Store::new(pool.clone());
    set_limits(
        &pool,
        json!({"dailyRequests":2,"dailyTokens":null,"concurrentRequests":32}),
    )
    .await;
    let mut jobs = Vec::new();
    for i in 0..10 {
        let id = store
            .create(owner, &format!("Concurrent {i}"), "test-model")
            .await
            .unwrap()
            .id;
        let copy = Store::new(pool.clone());
        jobs.push(tokio::spawn(async move {
            (
                id,
                copy.start_turn(owner, id, "Hello", "test-model", "", 0.7, 100)
                    .await,
            )
        }));
    }
    let mut accepted = Vec::new();
    for job in jobs {
        let (id, result) = job.await.unwrap();
        match result {
            Ok(turn) => accepted.push((id, turn)),
            Err(error) => assert_eq!(error.0, StatusCode::TOO_MANY_REQUESTS),
        }
    }
    assert_eq!(accepted.len(), 2);
    assert_eq!(ledger_count(&pool, owner).await, 2);
    let messages:i64=sqlx::query_scalar("SELECT COUNT(*) FROM messages m JOIN conversations c ON c.id=m.conversation_id WHERE c.owner_id=$1").bind(owner).fetch_one(&pool).await.unwrap();
    assert_eq!(
        messages, 4,
        "denied admissions cannot leave messages behind"
    );
    for (id, turn) in accepted {
        store
            .interrupt_turn(owner, id, turn.generation_id, "")
            .await
            .unwrap();
    }
    let next = store.create(owner, "Later", "test-model").await.unwrap();
    assert_eq!(
        store
            .start_turn(owner, next.id, "Hello", "test-model", "", 0.7, 100)
            .await
            .unwrap_err()
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // An explicit per-user override can inherit/reset daily limits and set concurrency.
    sqlx::query("INSERT INTO user_limit_overrides(user_id,limits)VALUES($1,$2)")
        .bind(owner)
        .bind(json!({"dailyRequests":null,"concurrentRequests":1}))
        .execute(&pool)
        .await
        .unwrap();
    store
        .start_turn(owner, next.id, "Hello", "test-model", "", 0.7, 100)
        .await
        .unwrap();
    let another = store
        .create(owner, "One at a time", "test-model")
        .await
        .unwrap();
    assert_eq!(
        store
            .start_turn(owner, another.id, "Hello", "test-model", "", 0.7, 100)
            .await
            .unwrap_err()
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn token_reservations_release_after_finish_and_expiry_without_inventing_usage(pool: PgPool) {
    let app = fixture(&pool).await;
    let user = session(&pool, "user").await;
    let store = Store::new(pool.clone());
    set_limits(&pool, json!({"dailyTokens":500,"concurrentRequests":3})).await;
    let first = store.create(user.id, "First", "test-model").await.unwrap();
    let second = store.create(user.id, "Second", "test-model").await.unwrap();
    let third = store.create(user.id, "Third", "test-model").await.unwrap();
    let a = store
        .start_turn(user.id, first.id, "One", "test-model", "", 0.7, 300)
        .await
        .unwrap();
    assert_eq!(
        store
            .start_turn(user.id, second.id, "Two", "test-model", "", 0.7, 300)
            .await
            .unwrap_err()
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(ledger_count(&pool, user.id).await, 1);
    store
        .finish_turn(
            user.id,
            first.id,
            a.generation_id,
            "Done",
            "completed",
            Some(json!({"prompt_tokens":50,"completion_tokens":100,"total_tokens":150})),
            None,
        )
        .await
        .unwrap();
    let b = store
        .start_turn(user.id, second.id, "Two", "test-model", "", 0.7, 300)
        .await
        .unwrap();
    assert_eq!(
        store
            .start_turn(user.id, third.id, "Three", "test-model", "", 0.7, 100)
            .await
            .unwrap_err()
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    sqlx::query(
        "UPDATE conversations SET generation_expires=NOW()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(second.id)
    .execute(&pool)
    .await
    .unwrap();
    let (_, expired) = call(&app, &user, "GET", "/api/usage", Value::Null).await;
    assert_eq!(
        expired["today"]["reservedTokens"], 0,
        "expired leases never retain reservations even before lazy recovery"
    );
    assert_eq!(expired["today"]["incompleteUsage"], 1);
    let c = store
        .start_turn(user.id, third.id, "Three", "test-model", "", 0.7, 300)
        .await
        .unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM generation_usage WHERE generation_id=$1")
            .bind(b.generation_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "interrupted");
    store
        .finish_turn(
            user.id,
            third.id,
            c.generation_id,
            "Done without usage",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let (_, report) = call(&app, &user, "GET", "/api/usage", Value::Null).await;
    assert_eq!(report["today"]["requests"], 3);
    assert_eq!(report["today"]["totalTokens"], 150);
    assert_eq!(report["today"]["incompleteUsage"], 2);
    assert_eq!(report["today"]["reservedTokens"], 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn unreported_usage_is_charged_an_estimate_for_admission_only(pool: PgPool) {
    let app = fixture(&pool).await;
    let user = session(&pool, "user").await;
    let store = Store::new(pool.clone());
    set_limits(&pool, json!({"dailyTokens":1000,"concurrentRequests":3})).await;
    let charged = |generation: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT charged_tokens FROM generation_usage WHERE generation_id=$1",
            )
            .bind(generation)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let chats = [
        store.create(user.id, "Stop", "test-model").await.unwrap(),
        store.create(user.id, "Error", "test-model").await.unwrap(),
        store.create(user.id, "Expire", "test-model").await.unwrap(),
        store.create(user.id, "Next", "test-model").await.unwrap(),
    ];
    // Stop keeps 400 output bytes: 1 context token plus 100 output tokens.
    let stopped = store
        .start_turn(user.id, chats[0].id, "abcd", "test-model", "", 0.7, 300)
        .await
        .unwrap();
    store
        .finish_turn(
            user.id,
            chats[0].id,
            stopped.generation_id,
            &"x".repeat(400),
            "interrupted",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(charged(stopped.generation_id).await, Some(101));
    // Output beyond the reservation is capped at the admitted budget.
    let failed = store
        .start_turn(user.id, chats[1].id, "abcd", "test-model", "", 0.7, 50)
        .await
        .unwrap();
    store
        .finish_turn(
            user.id,
            chats[1].id,
            failed.generation_id,
            &"x".repeat(4000),
            "error",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(charged(failed.generation_id).await, Some(51));
    // An abandoned lease is charged from its last checkpoint.
    let expired = store
        .start_turn(user.id, chats[2].id, "abcd", "test-model", "", 0.7, 200)
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET content=$2 WHERE generation_id=$1")
        .bind(expired.generation_id)
        .bind("y".repeat(80))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE conversations SET generation_expires=NOW()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(chats[2].id)
    .execute(&pool)
    .await
    .unwrap();
    // Follow-up output is never saved, so its whole reservation is charged.
    let follow_up = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    operations::reserve_follow_up_in(&mut tx, user.id, follow_up, 256, 300)
        .await
        .unwrap();
    operations::finish_in(&mut tx, follow_up, None, "error")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(charged(follow_up).await, Some(75 + 256));
    // Charged so far: 101 + 51 + (1 + 20) + 331 = 504 of 1000.
    assert_eq!(
        store
            .start_turn(user.id, chats[3].id, "abcd", "test-model", "", 0.7, 497)
            .await
            .unwrap_err()
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let next = store
        .start_turn(user.id, chats[3].id, "abcd", "test-model", "", 0.7, 496)
        .await
        .unwrap();
    assert_eq!(charged(expired.generation_id).await, Some(21));
    store
        .finish_turn(
            user.id,
            chats[3].id,
            next.generation_id,
            "Reported",
            "completed",
            Some(json!({"prompt_tokens":5,"completion_tokens":2,"total_tokens":7})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(charged(next.generation_id).await, None);
    let (_, report) = call(&app, &user, "GET", "/api/usage", Value::Null).await;
    assert_eq!(report["today"]["requests"], 5);
    assert_eq!(
        report["today"]["totalTokens"], 7,
        "estimates never appear as reported usage"
    );
    assert_eq!(report["today"]["incompleteUsage"], 4);
}

#[sqlx::test(migrations = "./migrations")]
async fn failure_after_admission_rolls_back_messages_and_ledger_together(pool: PgPool) {
    let owner = session(&pool, "user").await.id;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Rollback", "test-model").await.unwrap();
    sqlx::query("CREATE FUNCTION reject_fixture_assistant() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.role='assistant' THEN RAISE EXCEPTION 'injected persistence failure'; END IF; RETURN NEW; END $$").execute(&pool).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_fixture_assistant BEFORE INSERT ON messages FOR EACH ROW EXECUTE FUNCTION reject_fixture_assistant()").execute(&pool).await.unwrap();
    assert!(store
        .start_turn(owner, chat.id, "Hello", "test-model", "", 0.7, 100)
        .await
        .is_err());
    assert_eq!(ledger_count(&pool, owner).await, 0);
    assert!(store.get(owner, chat.id).await.unwrap().messages.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn administrator_limits_are_validated_audited_and_usage_is_private(pool: PgPool) {
    let app = fixture(&pool).await;
    let admin = session(&pool, "admin").await;
    let alice = session(&pool, "user").await;
    let bob = session(&pool, "user").await;
    for path in ["/api/admin/usage", "/api/admin/limits", "/api/admin/health"] {
        assert_eq!(
            call(&app, &alice, "GET", path, Value::Null).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &app,
            &alice,
            "PATCH",
            "/api/admin/limits",
            json!({"dailyRequests":5})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for invalid in [
        json!({"dailyRequests":0}),
        json!({"dailyTokens":-1}),
        json!({"concurrentRequests":null}),
        json!({"concurrentRequests":33}),
        json!({"extra":1}),
    ] {
        assert_eq!(
            call(&app, &admin, "PATCH", "/api/admin/limits", invalid)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, saved) = call(
        &app,
        &admin,
        "PATCH",
        "/api/admin/limits",
        json!({"dailyRequests":10,"dailyTokens":1000,"concurrentRequests":4}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let path = format!("/api/admin/limits/{}", alice.id);
    assert_eq!(
        call(
            &app,
            &admin,
            "PUT",
            &path,
            json!({"dailyRequests":null,"dailyTokens":2000})
        )
        .await
        .0,
        StatusCode::OK
    );
    let inherited = operations::effective_limits(&pool, bob.id).await.unwrap();
    assert_eq!(inherited.daily_requests, Some(10));
    let overrides = operations::effective_limits(&pool, alice.id).await.unwrap();
    assert_eq!(overrides.daily_requests, None);
    assert_eq!(overrides.daily_tokens, Some(2000));
    assert_eq!(overrides.concurrent_requests, 4);
    let store = Store::new(pool.clone());
    let chat = store.create(alice.id, "Usage", "test-model").await.unwrap();
    let turn = store
        .start_turn(alice.id, chat.id, "Hello", "test-model", "", 0.7, 100)
        .await
        .unwrap();
    store
        .finish_turn(
            alice.id,
            chat.id,
            turn.generation_id,
            "Done",
            "completed",
            Some(json!({"prompt_tokens":10,"completion_tokens":20,"total_tokens":30})),
            None,
        )
        .await
        .unwrap();
    let (status, report) = call(&app, &admin, "GET", "/api/admin/usage", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let rows = report["users"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let alice_row = rows
        .iter()
        .find(|row| row["id"] == alice.id.to_string())
        .unwrap();
    assert_eq!(alice_row["totals"]["totalTokens"], 30);
    assert_eq!(alice_row["limits"]["dailyTokens"], 2000);
    let bob_row = rows
        .iter()
        .find(|row| row["id"] == bob.id.to_string())
        .unwrap();
    assert_eq!(bob_row["totals"]["requests"], 0);
    assert_eq!(
        call(&app, &admin, "DELETE", &path, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        operations::effective_limits(&pool, alice.id)
            .await
            .unwrap()
            .daily_requests,
        Some(10)
    );
    let audit: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action='limits.update'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audit, 3);
    let (status, health) = call(&app, &admin, "GET", "/api/admin/health", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["database"]["ready"], true);
    assert_eq!(health["gateway"]["configured"], false);
    assert!(!health.to_string().contains("postgresql://"));
}
