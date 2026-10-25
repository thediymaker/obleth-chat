//! Optional retention must never delete user data until an administrator opts in.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    retention,
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
    sqlx::query(
        "INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Retention test',$3)",
    )
    .bind(id)
    .bind(format!("retention-{}", id.simple()))
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()));
    sqlx::query("INSERT INTO auth_sessions(token_hash,user_id,expires_at)VALUES($1,$2,NOW()+INTERVAL '1 hour')")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(pool).await.unwrap();
    Session {
        id,
        cookie: format!("obleth_chat_session={token}"),
        csrf: URL_SAFE_NO_PAD.encode(Sha256::digest(
            format!("obleth-chat-csrf:{token}").as_bytes(),
        )),
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
    retention::router(AppState::new(auth, pool.clone(), None, 4))
}
async fn call(app: &Router, user: &Session, method: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri("/api/admin/retention")
                .header("origin", ORIGIN)
                .header("cookie", &user.cookie)
                .header("x-csrf-token", &user.csrf)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn configure(pool: &PgPool, value: Value) {
    sqlx::query("UPDATE retention_policy SET policy=$1,last_run=NULL")
        .bind(value)
        .execute(pool)
        .await
        .unwrap();
}
async fn conversation(pool: &PgPool, owner: Uuid, days: i32) -> Uuid {
    let id = Store::new(pool.clone())
        .create(owner, "Retention fixture", "fixture")
        .await
        .unwrap()
        .id;
    sqlx::query("UPDATE conversations SET updated_at=NOW()-make_interval(days=>$2) WHERE id=$1")
        .bind(id)
        .bind(days)
        .execute(pool)
        .await
        .unwrap();
    id
}
async fn file(pool: &PgPool, owner: Uuid, days: i32) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO attachments(id,owner_id,name,mime_type,kind,size,data,created_at)VALUES($1,$2,'file.txt','text/plain','text',1,$3,NOW()-make_interval(days=>$4))")
        .bind(id).bind(owner).bind(b"x".to_vec()).bind(days).execute(pool).await.unwrap();
    id
}

#[sqlx::test(migrations = "./migrations")]
async fn defaults_keep_user_data_forever_but_expired_authentication_is_pruned(pool: PgPool) {
    let app = fixture(&pool).await;
    let admin = session(&pool, "admin").await;
    let chat = conversation(&pool, admin.id, 90).await;
    let attachment = file(&pool, admin.id, 90).await;
    let old_event = Uuid::new_v4();
    sqlx::query("INSERT INTO audit_events(id,action,target_type,created_at)VALUES($1,'old.event','fixture',NOW()-INTERVAL '90 days')").bind(old_event).execute(&pool).await.unwrap();
    for table in ["auth_sessions", "account_access_tokens"] {
        let sql = if table == "auth_sessions" {
            "INSERT INTO auth_sessions(token_hash,user_id,expires_at)VALUES($1,$2,NOW()-INTERVAL '1 hour')"
        } else {
            "INSERT INTO account_access_tokens(token_hash,user_id,kind,expires_at)VALUES($1,$2,'reset',NOW()-INTERVAL '1 hour')"
        };
        sqlx::query(sql)
            .bind(Uuid::new_v4().as_bytes().to_vec())
            .bind(admin.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let (status, policy) = call(&app, &admin, "GET", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        policy,
        json!({"conversationDays":null,"unusedFileDays":null,"auditDays":null,"enabled":false,"lastRun":null})
    );
    let report = retention::run_once(&pool).await.unwrap().unwrap();
    assert_eq!(report.conversations, 0);
    assert_eq!(report.unused_files, 0);
    assert_eq!(report.audit_events, 0);
    assert_eq!(report.expired_sessions, 1);
    assert_eq!(report.expired_access_tokens, 1);
    for (table, id) in [
        ("conversations", chat),
        ("attachments", attachment),
        ("audit_events", old_event),
    ] {
        assert!(sqlx::query_scalar::<_, bool>(&format!(
            "SELECT EXISTS(SELECT 1 FROM {table} WHERE id=$1)"
        ))
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap());
    }
    let (status, policy) = call(&app, &admin, "GET", Value::Null).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpired login must survive cleanup"
    );
    assert!(policy["lastRun"]["at"].is_string());
    assert_eq!(policy["lastRun"]["expiredSessions"], 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_explicitly_eligible_inactive_unreferenced_old_data_is_removed(pool: PgPool) {
    let owner = session(&pool, "user").await.id;
    let old = conversation(&pool, owner, 90).await;
    let recent = conversation(&pool, owner, 1).await;
    let active = conversation(&pool, owner, 90).await;
    let expired = conversation(&pool, owner, 90).await;
    sqlx::query("UPDATE conversations SET active_generation=$2,generation_expires=NOW()+INTERVAL '1 hour' WHERE id=$1").bind(active).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE conversations SET active_generation=$2,generation_expires=NOW()-INTERVAL '1 hour' WHERE id=$1").bind(expired).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    let unused = file(&pool, owner, 90).await;
    let new_file = file(&pool, owner, 1).await;
    let referenced = file(&pool, owner, 90).await;
    let message = Uuid::new_v4();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status)VALUES($1,$2,0,'user','Keep reference','completed')").bind(message).bind(recent).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO message_attachments(message_id,attachment_id,position)VALUES($1,$2,0)",
    )
    .bind(message)
    .bind(referenced)
    .execute(&pool)
    .await
    .unwrap();
    let old_audit = Uuid::new_v4();
    sqlx::query("INSERT INTO audit_events(id,action,target_type,created_at)VALUES($1,'old','fixture',NOW()-INTERVAL '90 days')").bind(old_audit).execute(&pool).await.unwrap();
    configure(
        &pool,
        json!({"conversationDays":30,"unusedFileDays":30,"auditDays":30}),
    )
    .await;
    let report = retention::run_once(&pool).await.unwrap().unwrap();
    assert_eq!(
        (
            report.conversations,
            report.unused_files,
            report.audit_events
        ),
        (2, 1, 1)
    );
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1)"
    )
    .bind(old)
    .fetch_one(&pool)
    .await
    .unwrap());
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM attachments WHERE id=$1)")
            .bind(unused)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    for id in [recent, active] {
        assert!(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1)"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap());
        assert!(!sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1)"
        )
        .bind(expired)
        .fetch_one(&pool)
        .await
        .unwrap());
    }
    for id in [referenced, new_file] {
        assert!(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM attachments WHERE id=$1)"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap());
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM audit_events WHERE action='retention.cleanup'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn cleanup_is_bounded_single_replica_and_skips_busy_account_writes(pool: PgPool) {
    let busy = session(&pool, "user").await.id;
    let available = session(&pool, "user").await.id;
    let protected = conversation(&pool, busy, 90).await;
    sqlx::query("INSERT INTO conversations(id,owner_id,title,model,updated_at)SELECT gen_random_uuid(),$1,'Old','fixture',NOW()-INTERVAL '90 days' FROM generate_series(1,150)").bind(available).execute(&pool).await.unwrap();
    configure(
        &pool,
        json!({"conversationDays":30,"unusedFileDays":null,"auditDays":null}),
    )
    .await;
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(busy)
        .execute(&mut *writer)
        .await
        .unwrap();
    let mut jobs = Vec::new();
    for _ in 0..8 {
        let clone = pool.clone();
        jobs.push(tokio::spawn(async move {
            retention::run_once(&clone).await.unwrap()
        }));
    }
    let mut completed = 0;
    for job in jobs {
        if let Some(report) = job.await.unwrap() {
            completed += 1;
            assert_eq!(report.conversations, 100);
        }
    }
    assert_eq!(
        completed, 1,
        "replicas must share the persisted hourly schedule"
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1)"
    )
    .bind(protected)
    .fetch_one(&pool)
    .await
    .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversations WHERE owner_id=$1")
            .bind(available)
            .fetch_one(&pool)
            .await
            .unwrap(),
        50
    );
    writer.rollback().await.unwrap();
    assert!(retention::run_once(&pool).await.unwrap().is_none());
    sqlx::query("UPDATE retention_policy SET last_run=NOW()-INTERVAL '2 hours'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        retention::run_once(&pool)
            .await
            .unwrap()
            .unwrap()
            .conversations,
        51
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn policy_requires_admin_validates_days_and_audits_changes(pool: PgPool) {
    let app = fixture(&pool).await;
    let admin = session(&pool, "admin").await;
    let user = session(&pool, "user").await;
    assert_eq!(
        call(&app, &user, "GET", Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, &user, "PATCH", json!({"conversationDays":1}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for invalid in [
        json!({"conversationDays":0}),
        json!({"unusedFileDays":3651}),
        json!({"auditDays":29}),
        json!({"conversationDays":1.5}),
        json!({"enabled":true}),
    ] {
        assert_eq!(
            call(&app, &admin, "PATCH", invalid).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, saved) = call(
        &app,
        &admin,
        "PATCH",
        json!({"conversationDays":30,"auditDays":365}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["unusedFileDays"], Value::Null);
    assert_eq!(call(&app, &admin, "GET", Value::Null).await.1, saved);
    let (status, disabled) = call(
        &app,
        &admin,
        "PATCH",
        json!({"conversationDays":null,"auditDays":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(disabled["enabled"], false);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM audit_events WHERE action='retention.update' AND actor_id=$1"
        )
        .bind(admin.id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
}
