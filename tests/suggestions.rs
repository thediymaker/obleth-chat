//! Real PostgreSQL and local HTTP checks for the optional auxiliary request.
use axum::{
    extract::State,
    response::{sse::Event, Sse},
    routing::{get, post},
    Json, Router,
};
use obleth_chat::{
    auth::{Auth, AuthConfig, Identity, Role},
    gateway::Gateway,
    operations, suggestions, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{
    convert::Infallible,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Value>>>,
    reply: Arc<Mutex<String>>,
    delay: Duration,
}
async fn model_list() -> Json<Value> {
    Json(json!({"data":[{"id":"test-model"}]}))
}
async fn chat(
    State(fake): State<Fake>,
    Json(body): Json<Value>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    fake.seen.lock().unwrap().push(body);
    let reply = fake.reply.lock().unwrap().clone();
    Sse::new(async_stream::stream! {
        tokio::time::sleep(fake.delay).await;
        yield Ok(Event::default().data(json!({"choices":[{"delta":{"content":reply}}]}).to_string()));
        yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":90,"completion_tokens":25,"total_tokens":115}}).to_string()));
        yield Ok(Event::default().data("[DONE]"));
    })
}
async fn fixture(pool: &PgPool, delay: Duration) -> (AppState, Fake, tokio::task::JoinHandle<()>) {
    static KEY: OnceLock<String> = OnceLock::new();
    let pem = KEY.get_or_init(|| {
        RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
            .unwrap()
            .to_pkcs8_pem(Default::default())
            .unwrap()
            .to_string()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway = Gateway::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
        pem.as_bytes(),
        "http://localhost".into(),
        "obleth".into(),
    )
    .unwrap();
    let fake = Fake {
        seen: Arc::new(Mutex::new(vec![])),
        reply: Arc::new(Mutex::new(
            json!([
                "How does this work?",
                "Can you show an example?",
                "What should I try next?"
            ])
            .to_string(),
        )),
        delay,
    };
    let router = Router::new()
        .route("/v1/models", get(model_list))
        .route("/v1/chat/completions", post(chat))
        .with_state(fake.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: "http://localhost".into(),
            session_hours: 12,
            bootstrap_username: None,
            bootstrap_password: None,
            oidc: None,
            login_rate_per_minute: 100,
        },
    )
    .await
    .unwrap();
    (
        AppState::new(auth, pool.clone(), Some(gateway), 4),
        fake,
        task,
    )
}
async fn seed(pool: &PgPool) -> (Identity, Uuid, Uuid) {
    let owner = Uuid::new_v4();
    let conversation = Uuid::new_v4();
    let question = Uuid::new_v4();
    let answer = Uuid::new_v4();
    let name = format!("suggestions_{owner}");
    sqlx::query("INSERT INTO users(id,username,display_name)VALUES($1,$2,'Suggestions test')")
        .bind(owner)
        .bind(&name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO conversations(id,owner_id,title,model)VALUES($1,$2,'Suggestions','test-model')").bind(conversation).bind(owner).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status)VALUES($1,$2,0,'user','Explain loops','completed')").bind(question).bind(conversation).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status,settings,parent_message_id,generation_id)VALUES($1,$2,1,'assistant','Loops repeat work.','completed','{\"model\":\"test-model\"}',$3,$4)").bind(answer).bind(conversation).bind(question).bind(Uuid::new_v4()).execute(pool).await.unwrap();
    sqlx::query("UPDATE conversations SET selected_leaf_id=$2 WHERE id=$1")
        .bind(conversation)
        .bind(answer)
        .execute(pool)
        .await
        .unwrap();
    (
        Identity {
            id: owner,
            username: name,
            display_name: "Suggestions test".into(),
            role: Role::User,
        },
        conversation,
        answer,
    )
}
async fn result(pool: &PgPool, assistant: Uuid) -> (String, Value) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let value: (String, Value) =
                sqlx::query_as("SELECT follow_ups_status,follow_ups FROM messages WHERE id=$1")
                    .bind(assistant)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if value.0 != "pending" {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("follow-up task finished")
}

#[sqlx::test(migrations = "./migrations")]
async fn generated_once_with_same_model_private_context_and_actual_usage(pool: PgPool) {
    let (state, fake, server) = fixture(&pool, Duration::from_millis(50)).await;
    let (user, conversation, assistant) = seed(&pool).await;
    let busy = state.slots.clone().try_acquire_many_owned(3).unwrap();
    assert!(
        !suggestions::schedule(
            state.clone(),
            user.clone(),
            conversation,
            assistant,
            "test-model".into()
        )
        .await,
        "leave the last global slot for an interactive request"
    );
    drop(busy);
    let (a, b) = tokio::join!(
        suggestions::schedule(
            state.clone(),
            user.clone(),
            conversation,
            assistant,
            "test-model".into()
        ),
        suggestions::schedule(
            state.clone(),
            user.clone(),
            conversation,
            assistant,
            "test-model".into()
        )
    );
    assert_ne!(a, b, "only one atomic claim succeeds");
    let (status, questions) = result(&pool, assistant).await;
    assert_eq!(status, "completed");
    assert_eq!(questions.as_array().unwrap().len(), 3);
    {
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0]["model"], "test-model");
        assert_eq!(seen[0]["max_tokens"], 256);
        assert_eq!(seen[0]["reasoning_effort"], "none");
        assert_eq!(seen[0]["stream_options"]["include_usage"], true);
        assert!(seen[0].get("tools").is_none());
    }
    let usage:(String,i64,i64,bool) = sqlx::query_as("SELECT purpose,prompt_tokens,total_tokens,usage_complete FROM generation_usage WHERE generation_id=$1").bind(assistant).fetch_one(&pool).await.unwrap();
    assert_eq!(usage, ("follow_up".into(), 90, 115, true));
    assert!(
        !suggestions::schedule(state, user, conversation, assistant, "test-model".into()).await
    );
    server.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn ownership_latest_selection_and_current_model_policy_are_enforced(pool: PgPool) {
    let (state, fake, server) = fixture(&pool, Duration::from_millis(200)).await;
    let (user, conversation, assistant) = seed(&pool).await;
    let (other, _, _) = seed(&pool).await;
    assert!(
        !suggestions::schedule(
            state.clone(),
            other,
            conversation,
            assistant,
            "test-model".into()
        )
        .await
    );
    assert!(
        !suggestions::schedule(
            state.clone(),
            user.clone(),
            conversation,
            assistant,
            "other-model".into()
        )
        .await
    );
    assert!(
        suggestions::schedule(
            state.clone(),
            user.clone(),
            conversation,
            assistant,
            "test-model".into()
        )
        .await
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while fake.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sqlx::query("UPDATE conversations SET selected_leaf_id=NULL WHERE id=$1")
        .bind(conversation)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        result(&pool, assistant).await.1,
        json!([]),
        "never show old branch suggestions"
    );
    let (restricted, chat_id, message_id) = seed(&pool).await;
    sqlx::query(
        "INSERT INTO model_profiles(model_id,name,enabled)VALUES('test-model','Restricted',FALSE)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        suggestions::schedule(state, restricted, chat_id, message_id, "test-model".into()).await
    );
    assert_eq!(result(&pool, message_id).await.0, "error");
    assert_eq!(
        fake.seen.lock().unwrap().len(),
        1,
        "restricted model never receives another request"
    );
    server.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn failures_and_quota_denials_never_modify_the_primary_answer(pool: PgPool) {
    let (state, fake, server) = fixture(&pool, Duration::ZERO).await;
    let (user, conversation, assistant) = seed(&pool).await;
    *fake.reply.lock().unwrap() = "not valid JSON".into();
    assert!(
        suggestions::schedule(
            state.clone(),
            user,
            conversation,
            assistant,
            "test-model".into()
        )
        .await
    );
    assert_eq!(result(&pool, assistant).await.0, "error");
    let answer: (String, String) =
        sqlx::query_as("SELECT status,content FROM messages WHERE id=$1")
            .bind(assistant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(answer, ("completed".into(), "Loops repeat work.".into()));
    let total: i64 =
        sqlx::query_scalar("SELECT total_tokens FROM generation_usage WHERE generation_id=$1")
            .bind(assistant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        total, 115,
        "usage counted even if optional output is unusable"
    );
    let (limited, chat_id, message_id) = seed(&pool).await;
    sqlx::query("UPDATE site_limits SET limits='{\"dailyTokens\":1}' WHERE id=TRUE")
        .execute(&pool)
        .await
        .unwrap();
    assert!(suggestions::schedule(state, limited, chat_id, message_id, "test-model".into()).await);
    assert_eq!(result(&pool, message_id).await.0, "error");
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
    server.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn auxiliary_reservations_compete_with_each_other_and_expire(pool: PgPool) {
    let (user, _, _) = seed(&pool).await;
    sqlx::query("UPDATE site_limits SET limits='{\"concurrentRequests\":1,\"dailyTokens\":540}' WHERE id=TRUE").execute(&pool).await.unwrap();
    let generation = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    operations::reserve_follow_up_in(&mut tx, user.id, generation, 256, 100)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    operations::recover_in(&mut tx, user.id).await.unwrap();
    assert!(
        operations::reserve_follow_up_in(&mut tx, user.id, Uuid::new_v4(), 1, 10)
            .await
            .is_err(),
        "live auxiliary lease occupies an optional request slot"
    );
    tx.rollback().await.unwrap();
    sqlx::query("UPDATE generation_usage SET reservation_expires=NOW()-INTERVAL '1 second' WHERE generation_id=$1").bind(generation).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    operations::reserve_follow_up_in(&mut tx, user.id, Uuid::new_v4(), 256, 10)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM generation_usage WHERE generation_id=$1")
            .bind(generation)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "interrupted");
    // The abandoned lease's reservation becomes an admission charge (25 context
    // tokens plus its whole 256-token budget), leaving room for exactly one more.
    let charged: Option<i64> =
        sqlx::query_scalar("SELECT charged_tokens FROM generation_usage WHERE generation_id=$1")
            .bind(generation)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(charged, Some(281));
}

#[sqlx::test(migrations = "./migrations")]
async fn interactive_turn_preempts_optional_work_without_losing_usage(pool: PgPool) {
    let (state, fake, server) = fixture(&pool, Duration::from_secs(10)).await;
    let (user, conversation, assistant) = seed(&pool).await;
    sqlx::query("UPDATE site_limits SET limits='{\"concurrentRequests\":1}' WHERE id=TRUE")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        suggestions::schedule(
            state.clone(),
            user.clone(),
            conversation,
            assistant,
            "test-model".into()
        )
        .await
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while fake.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let next = state
        .store
        .create(user.id, "Next", "test-model")
        .await
        .unwrap();
    state
        .store
        .start_turn(
            user.id,
            next.id,
            "Next question",
            "test-model",
            "",
            0.7,
            100,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.metrics.streams.get() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("preempted worker cancelled promptly");
    assert_eq!(result(&pool, assistant).await.0, "error");
    let status: String =
        sqlx::query_scalar("SELECT status FROM generation_usage WHERE generation_id=$1")
            .bind(assistant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "interrupted");
    // A completion racing with preemption must still charge its reported tokens.
    let mut tx = pool.begin().await.unwrap();
    operations::finish_in(
        &mut tx,
        assistant,
        Some(&json!({"prompt_tokens":10,"completion_tokens":5,"total_tokens":15})),
        "completed",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let usage: (String, i64, bool) = sqlx::query_as(
        "SELECT status,total_tokens,usage_complete FROM generation_usage WHERE generation_id=$1",
    )
    .bind(assistant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(usage, ("interrupted".into(), 15, true));
    server.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn both_leases_outlast_the_task_and_remain_pending_beyond_the_old_deadline(pool: PgPool) {
    let (state, fake, server) = fixture(&pool, Duration::from_secs(10)).await;
    let (user, conversation, assistant) = seed(&pool).await;
    assert!(
        suggestions::schedule(
            state.clone(),
            user.clone(),
            conversation,
            assistant,
            "test-model".into()
        )
        .await
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while fake.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (pending_seconds,usage_seconds):(f64,f64)=sqlx::query_as("SELECT EXTRACT(EPOCH FROM m.follow_ups_expires_at-clock_timestamp())::double precision,EXTRACT(EPOCH FROM g.reservation_expires-clock_timestamp())::double precision FROM messages m JOIN generation_usage g ON g.generation_id=m.id WHERE m.id=$1").bind(assistant).fetch_one(&pool).await.unwrap();
    for remaining in [pending_seconds, usage_seconds] {
        assert!(
            remaining > operations::FOLLOW_UP_TIMEOUT.as_secs_f64(),
            "lease cannot expire while bounded work is still permitted"
        );
        assert!(remaining <= operations::FOLLOW_UP_LEASE_SECONDS as f64);
    }
    // Advance both durable lease ages past the former 45-second expiry without
    // sleeping; readers and quota recovery must still treat this task as live.
    sqlx::query("UPDATE messages SET follow_ups_expires_at=follow_ups_expires_at-INTERVAL '46 seconds' WHERE id=$1").bind(assistant).execute(&pool).await.unwrap();
    sqlx::query("UPDATE generation_usage SET reservation_expires=reservation_expires-INTERVAL '46 seconds' WHERE generation_id=$1").bind(assistant).execute(&pool).await.unwrap();
    let saved = state.store.get(user.id, conversation).await.unwrap();
    assert_eq!(
        saved
            .messages
            .iter()
            .find(|m| m.id == assistant)
            .unwrap()
            .follow_ups_status,
        "pending"
    );
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    operations::recover_in(&mut tx, user.id).await.unwrap();
    tx.commit().await.unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM generation_usage WHERE generation_id=$1")
            .bind(assistant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "reserved");
    state.shutdown.cancel();
    assert_eq!(result(&pool, assistant).await.0, "error");
    server.abort();
}
