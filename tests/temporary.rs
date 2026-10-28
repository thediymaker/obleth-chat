//! Temporary chats: left out of history and search, never shared, exported, branched or
//! moved into a project, deleted when their owner leaves them or after a quiet day.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    router,
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
        "INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Temporary test',$3)",
    )
    .bind(id)
    .bind(format!("temporary-{}", id.simple()))
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
            ..AuthConfig::default()
        },
    )
    .await
    .unwrap();
    router(AppState::new(auth, pool.clone(), None, 4), "web".into())
}
async fn call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Option<String>, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("origin", ORIGIN)
                .header("cookie", &user.cookie)
                .header("x-csrf-token", &user.csrf)
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, kind, String::from_utf8(bytes.to_vec()).unwrap())
}
async fn json_call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, text) = call(app, user, method, path, body).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}
async fn chat(app: &Router, user: &Session, title: &str, temporary: bool) -> Uuid {
    let (status, body) = json_call(
        app,
        user,
        "POST",
        "/api/conversations",
        Some(json!({"title":title,"model":"fixture","temporary":temporary})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["conversation"]["temporary"], temporary);
    body["conversation"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}
/// Adds a finished exchange that mentions `word`.
async fn exchange(pool: &PgPool, owner: Uuid, chat: Uuid, word: &str) {
    let s = Store::new(pool.clone());
    let turn = s
        .start_turn(
            owner,
            chat,
            &format!("Tell me about {word}"),
            "fixture",
            "",
            0.7,
            2048,
        )
        .await
        .unwrap();
    s.finish_turn_detailed(
        owner,
        chat,
        turn.generation_id,
        &format!("All about {word}"),
        "",
        "completed",
        None,
        None,
        Some("stop"),
        None,
    )
    .await
    .unwrap();
}
async fn exists(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn age(pool: &PgPool, id: Uuid, hours: i32) {
    sqlx::query("UPDATE conversations SET updated_at=NOW()-make_interval(hours=>$2) WHERE id=$1")
        .bind(id)
        .bind(hours)
        .execute(pool)
        .await
        .unwrap();
}
fn ids(body: &Value) -> Vec<String> {
    body["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn temporary_chats_are_not_listed_or_searchable(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let kept = chat(&app, &alice, "Kept chat", false).await;
    let temporary = chat(&app, &alice, "Temporary chat", true).await;
    exchange(&pool, alice.id, kept, "otters").await;
    exchange(&pool, alice.id, temporary, "otters").await;

    for path in [
        "/api/conversations",
        "/api/conversations?archived=all",
        "/api/conversations?query=otters",
        "/api/conversations?query=Temporary",
    ] {
        let (status, body) = json_call(&app, &alice, "GET", path, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !ids(&body).contains(&temporary.to_string()),
            "{path} lists it"
        );
    }
    let (_, body) = json_call(&app, &alice, "GET", "/api/conversations?query=otters", None).await;
    assert_eq!(ids(&body), vec![kept.to_string()]);
    assert_eq!(body["total"], 1);
    let listed = Store::new(pool.clone()).list(alice.id).await.unwrap();
    assert_eq!(listed.iter().map(|c| c.id).collect::<Vec<_>>(), vec![kept]);

    // The owner still opens it and keeps talking in it.
    let (status, body) = json_call(
        &app,
        &alice,
        "GET",
        &format!("/api/conversations/{temporary}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["conversation"]["temporary"], true);
    assert_eq!(
        body["conversation"]["messages"].as_array().unwrap().len(),
        2
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn temporary_chats_cannot_be_shared_exported_branched_or_moved(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let temporary = chat(&app, &alice, "Temporary chat", true).await;
    exchange(&pool, alice.id, temporary, "otters").await;
    let reply: Uuid =
        sqlx::query_scalar("SELECT id FROM messages WHERE conversation_id=$1 AND role='assistant'")
            .bind(temporary)
            .fetch_one(&pool)
            .await
            .unwrap();
    let question: Uuid =
        sqlx::query_scalar("SELECT id FROM messages WHERE conversation_id=$1 AND role='user'")
            .bind(temporary)
            .fetch_one(&pool)
            .await
            .unwrap();
    let (_, project) = json_call(
        &app,
        &alice,
        "POST",
        "/api/projects",
        Some(json!({"name":"Somewhere"})),
    )
    .await;
    let project = project["project"]["id"].as_str().unwrap().to_owned();

    let refused = [
        (
            "PUT",
            format!("/api/conversations/{temporary}/share"),
            json!({"scope":"signed_in"}),
        ),
        (
            "POST",
            "/api/conversations/export".into(),
            json!({"ids":[temporary]}),
        ),
        (
            "PUT",
            format!("/api/conversations/{temporary}/project"),
            json!({"projectId":project}),
        ),
        (
            "POST",
            format!("/api/conversations/{temporary}/branch"),
            json!({"throughMessageId":reply}),
        ),
        (
            "POST",
            format!("/api/conversations/{temporary}/fork"),
            json!({"messageId":question,"mode":"edit","content":"Again"}),
        ),
    ];
    for (method, path, body) in refused {
        let (status, error) = json_call(&app, &alice, method, &path, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {path}: {error}");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("temporary chat"),
            "{error}"
        );
    }
    let (scope, project_id, chats): (String, Option<Uuid>, i64) = sqlx::query_as(
        "SELECT share_scope,project_id,(SELECT COUNT(*) FROM conversations) FROM conversations WHERE id=$1",
    )
    .bind(temporary).fetch_one(&pool).await.unwrap();
    assert_eq!((scope.as_str(), project_id, chats), ("none", None, 1));
    // Even a direct write can't share it.
    assert!(
        sqlx::query("UPDATE conversations SET share_scope='signed_in' WHERE id=$1")
            .bind(temporary)
            .execute(&pool)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn leaving_deletes_a_temporary_chat_and_nothing_else(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let bob = session(&pool, "user").await;
    let kept = chat(&app, &alice, "Kept chat", false).await;
    let temporary = chat(&app, &alice, "Temporary chat", true).await;
    exchange(&pool, alice.id, temporary, "otters").await;
    let reply: Uuid =
        sqlx::query_scalar("SELECT id FROM messages WHERE conversation_id=$1 AND role='assistant'")
            .bind(temporary)
            .fetch_one(&pool)
            .await
            .unwrap();
    json_call(
        &app,
        &alice,
        "PUT",
        &format!("/api/conversations/{temporary}/messages/{reply}/feedback"),
        Some(json!({"rating":1})),
    )
    .await;

    let (status, _) = json_call(
        &app,
        &alice,
        "POST",
        &format!("/api/conversations/{kept}/leave"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = json_call(
        &app,
        &bob,
        "POST",
        &format!("/api/conversations/{temporary}/leave"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(exists(&pool, kept).await && exists(&pool, temporary).await);

    let (status, _) = json_call(
        &app,
        &alice,
        "POST",
        &format!("/api/conversations/{temporary}/leave"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!exists(&pool, temporary).await);
    assert!(exists(&pool, kept).await);
    let (messages, ratings): (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM messages WHERE conversation_id=$1),(SELECT COUNT(*) FROM message_feedback)")
        .bind(temporary).fetch_one(&pool).await.unwrap();
    assert_eq!((messages, ratings), (0, 0));
    let (status, _) = json_call(
        &app,
        &alice,
        "GET",
        &format!("/api/conversations/{temporary}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn leaving_while_a_reply_runs_stops_it_and_the_sweep_deletes_the_chat(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let temporary = chat(&app, &alice, "Temporary chat", true).await;
    let s = Store::new(pool.clone());
    let turn = s
        .start_turn(alice.id, temporary, "Still going", "fixture", "", 0.7, 2048)
        .await
        .unwrap();

    let (status, _) = json_call(
        &app,
        &alice,
        "POST",
        &format!("/api/conversations/{temporary}/leave"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (left, cancel): (bool, bool) = sqlx::query_as(
        "SELECT left_at IS NOT NULL,generation_cancel_requested FROM conversations WHERE id=$1",
    )
    .bind(temporary)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(left && cancel);
    // The running reply is not cut off by the sweep; it ends as stopped replies do.
    assert_eq!(s.sweep_temporary().await.unwrap(), 0);
    assert!(exists(&pool, temporary).await);
    s.finish_turn_detailed(
        alice.id,
        temporary,
        turn.generation_id,
        "Partial",
        "",
        "interrupted",
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.sweep_temporary().await.unwrap(), 1);
    assert!(!exists(&pool, temporary).await);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_sweep_deletes_quiet_temporary_chats_and_leaves_kept_chats_alone(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let quiet = chat(&app, &alice, "Quiet temporary", true).await;
    let recent = chat(&app, &alice, "Recent temporary", true).await;
    let running = chat(&app, &alice, "Running temporary", true).await;
    let old_kept = chat(&app, &alice, "Old kept chat", false).await;
    exchange(&pool, alice.id, quiet, "otters").await;
    let s = Store::new(pool.clone());
    s.start_turn(alice.id, running, "Still going", "fixture", "", 0.7, 2048)
        .await
        .unwrap();
    age(&pool, quiet, 25).await;
    age(&pool, recent, 1).await;
    age(&pool, running, 25).await;
    age(&pool, old_kept, 24 * 400).await;
    // A disabled owner's temporary chats go too.
    sqlx::query("UPDATE users SET enabled=FALSE WHERE id=$1")
        .bind(alice.id)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(s.sweep_temporary().await.unwrap(), 1);
    assert!(!exists(&pool, quiet).await);
    assert!(exists(&pool, recent).await);
    assert!(exists(&pool, running).await);
    assert!(exists(&pool, old_kept).await);
    assert_eq!(s.sweep_temporary().await.unwrap(), 0);
}
