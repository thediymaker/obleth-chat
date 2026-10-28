//! Thumbs up and down on replies: only the owner rates, viewers never see a rating, and
//! administrators see models, reasons and comments without chats or people.
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
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Feedback test',$3)")
        .bind(id)
        .bind(format!("feedback-{}", id.simple()))
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
/// A chat with one finished reply from `model`: (chat, reply).
async fn reply(pool: &PgPool, owner: Uuid, model: &str, answer: &str) -> (Uuid, Uuid) {
    let s = Store::new(pool.clone());
    let c = s.create(owner, "Feedback fixture", model).await.unwrap();
    let turn = s
        .start_turn(owner, c.id, "A private question", model, "", 0.7, 2048)
        .await
        .unwrap();
    s.finish_turn_detailed(
        owner,
        c.id,
        turn.generation_id,
        answer,
        "",
        "completed",
        None,
        None,
        Some("stop"),
        None,
    )
    .await
    .unwrap();
    (c.id, turn.assistant_id)
}
fn rating_path(chat: Uuid, message: Uuid) -> String {
    format!("/api/conversations/{chat}/messages/{message}/feedback")
}
async fn rating_of(app: &Router, user: &Session, chat: Uuid, message: Uuid) -> Value {
    let (status, body) = json_call(
        app,
        user,
        "GET",
        &format!("/api/conversations/{chat}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let messages = body["conversation"]["messages"].as_array().unwrap().clone();
    messages
        .into_iter()
        .find(|m| m["id"] == json!(message))
        .unwrap()["feedback"]
        .clone()
}

#[sqlx::test(migrations = "./migrations")]
async fn the_owner_rates_changes_and_clears_a_reply(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let (chat, message) = reply(&pool, alice.id, "model-a", "An answer").await;
    assert_eq!(rating_of(&app, &alice, chat, message).await, Value::Null);

    let (status, body) = json_call(
        &app,
        &alice,
        "PUT",
        &rating_path(chat, message),
        Some(json!({"rating":1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        rating_of(&app, &alice, chat, message).await,
        json!({"rating":1,"reasons":[],"comment":null})
    );

    // Switching to a thumbs down keeps one row, with its reasons and comment.
    let (status, _) = json_call(&app, &alice, "PUT", &rating_path(chat, message),
        Some(json!({"rating":-1,"reasons":["wrong","too_long","wrong"],"comment":"  The date is off.  "}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rating_of(&app, &alice, chat, message).await,
        json!({"rating":-1,"reasons":["wrong","too_long"],"comment":"The date is off."})
    );
    let (rows, model): (i64, String) =
        sqlx::query_as("SELECT COUNT(*),MIN(model) FROM message_feedback")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((rows, model.as_str()), (1, "model-a"));

    for bad in [
        json!({"rating":0}),
        json!({"rating":1,"reasons":["wrong"]}),
        json!({"rating":-1,"reasons":["dull"]}),
        json!({"rating":-1,"comment":"x".repeat(2_001)}),
    ] {
        let (status, _) =
            json_call(&app, &alice, "PUT", &rating_path(chat, message), Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let (status, _) = json_call(&app, &alice, "DELETE", &rating_path(chat, message), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(rating_of(&app, &alice, chat, message).await, Value::Null);

    // A rating goes with its chat.
    json_call(
        &app,
        &alice,
        "PUT",
        &rating_path(chat, message),
        Some(json!({"rating":1})),
    )
    .await;
    let (status, _) = json_call(
        &app,
        &alice,
        "DELETE",
        &format!("/api/conversations/{chat}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_feedback")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_the_owner_rates_and_viewers_never_see_a_rating(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let bob = session(&pool, "user").await;
    let (chat, message) = reply(&pool, alice.id, "model-a", "An answer").await;
    // The user's own message is not a reply.
    let question: Uuid =
        sqlx::query_scalar("SELECT id FROM messages WHERE conversation_id=$1 AND role='user'")
            .bind(chat)
            .fetch_one(&pool)
            .await
            .unwrap();
    let (status, _) = json_call(
        &app,
        &alice,
        "PUT",
        &rating_path(chat, question),
        Some(json!({"rating":1})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    json_call(
        &app,
        &alice,
        "PUT",
        &rating_path(chat, message),
        Some(json!({"rating":-1,"comment":"Mine"})),
    )
    .await;
    let (status, _) = json_call(
        &app,
        &alice,
        "PUT",
        &format!("/api/conversations/{chat}/share"),
        Some(json!({"scope":"signed_in"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Bob can read the shared chat, but can neither rate nor clear, nor see Alice's rating.
    for method in ["PUT", "DELETE"] {
        let (status, _) = json_call(
            &app,
            &bob,
            method,
            &rating_path(chat, message),
            Some(json!({"rating":1})),
        )
        .await;
        assert!(
            matches!(status, StatusCode::NOT_FOUND | StatusCode::NO_CONTENT),
            "{method} {status}"
        );
    }
    let (status, shared) = json_call(&app, &bob, "GET", &format!("/api/shared/{chat}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(shared["conversation"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m.get("feedback").is_none()));
    let (rating, comment): (i16, String) =
        sqlx::query_as("SELECT rating,comment FROM message_feedback")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((rating, comment.as_str()), (-1, "Mine"));
}

#[sqlx::test(migrations = "./migrations")]
async fn administrators_see_models_reasons_and_comments_without_chats_or_people(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool, "user").await;
    let admin = session(&pool, "admin").await;
    let (c1, m1) = reply(&pool, alice.id, "model-a", "A secret answer").await;
    let (c2, m2) = reply(&pool, alice.id, "model-a", "Another answer").await;
    let (c3, m3) = reply(&pool, alice.id, "=model,b", "Third answer").await;
    json_call(
        &app,
        &alice,
        "PUT",
        &rating_path(c1, m1),
        Some(json!({"rating":1})),
    )
    .await;
    json_call(
        &app,
        &alice,
        "PUT",
        &rating_path(c2, m2),
        Some(json!({"rating":-1,"reasons":["wrong","other"],"comment":"Made up a citation"})),
    )
    .await;
    json_call(
        &app,
        &alice,
        "PUT",
        &rating_path(c3, m3),
        Some(json!({"rating":-1,"reasons":["wrong"]})),
    )
    .await;
    // An old rating falls outside a short period.
    sqlx::query(
        "UPDATE message_feedback SET updated_at=NOW()-INTERVAL '20 days' WHERE message_id=$1",
    )
    .bind(m3)
    .execute(&pool)
    .await
    .unwrap();

    let (status, _) = json_call(&app, &alice, "GET", "/api/admin/feedback", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = json_call(&app, &admin, "GET", "/api/admin/feedback?days=0", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = json_call(&app, &admin, "GET", "/api/admin/feedback?days=30", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["days"], 30);
    assert_eq!(
        body["models"],
        json!([{"model":"model-a","up":1,"down":1,"upShare":0.5},{"model":"=model,b","up":0,"down":1,"upShare":0.0}])
    );
    assert_eq!(
        body["reasons"],
        json!([{"reason":"wrong","count":2},{"reason":"other","count":1}])
    );
    let comments = body["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["comment"], "Made up a citation");
    assert_eq!(comments[0]["model"], "model-a");
    assert_eq!(comments[0]["rating"], -1);
    assert!(comments[0]["at"].is_string());
    // Nothing that leads to a chat, a reply or a person.
    let text = body.to_string();
    for private in [c1, c2, c3, m1, m2, m3, alice.id] {
        assert!(!text.contains(&private.to_string()), "{private} leaked");
    }
    for private in [
        "secret answer",
        "private question",
        "Feedback fixture",
        "feedback-",
    ] {
        assert!(!text.contains(private), "{private} leaked");
    }

    let (status, body) = json_call(&app, &admin, "GET", "/api/admin/feedback?days=7", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["models"],
        json!([{"model":"model-a","up":1,"down":1,"upShare":0.5}])
    );

    let (status, kind, csv) = call(
        &app,
        &admin,
        "GET",
        "/api/admin/feedback/models.csv?days=30",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(kind.as_deref(), Some("text/csv; charset=utf-8"));
    assert_eq!(
        csv,
        "model,up,down,up_percent\r\nmodel-a,1,1,50.0\r\n\"'=model,b\",0,1,0.0\r\n"
    );
    let (status, _, _) = call(&app, &alice, "GET", "/api/admin/feedback/models.csv", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
