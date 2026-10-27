//! Decisions settings: administrators choose where yes/no questions go, a key for
//! another endpoint is kept sealed and never shown, and Check asks with the form's
//! values before they are saved. Requires a disposable DATABASE_URL.
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    decisions::{self, Decisions, Endpoint, Key},
    secrets::SecretKey,
    AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";

struct Session {
    cookie: String,
    csrf: String,
}

async fn session(pool: &PgPool, role: &str) -> Session {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Decisions test',$3)",
    )
    .bind(id)
    .bind(format!("decisions-{}", id.simple()))
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()));
    sqlx::query("INSERT INTO auth_sessions(token_hash,user_id,expires_at)VALUES($1,$2,NOW()+INTERVAL '1 hour')")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(pool).await.unwrap();
    Session {
        cookie: format!("obleth_chat_session={token}"),
        csrf: URL_SAFE_NO_PAD.encode(Sha256::digest(
            format!("obleth-chat-csrf:{token}").as_bytes(),
        )),
    }
}

async fn fixture(pool: &PgPool, secret: Option<SecretKey>) -> Router {
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: ORIGIN.into(),
            session_hours: 12,
            login_rate_per_minute: 1000,
            ..AuthConfig::default()
        },
    )
    .await
    .unwrap();
    let mut state = AppState::new(auth, pool.clone(), None, 4);
    state.decisions = Decisions::new(pool.clone(), secret);
    decisions::router(state)
}

async fn call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
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

/// A decisions service: keeps each request and answers with `answer`.
/// The Authorization header and the body of one request.
type Asked = (Option<String>, Value);

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Asked>>>);

async fn service(answer: fn() -> Response) -> (String, Seen) {
    let seen = Seen::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock = Router::new()
        .route(
            "/v1/verdicts",
            post(
                move |State(seen): State<Seen>, headers: HeaderMap, Json(body): Json<Value>| async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    seen.0.lock().unwrap().push((auth, body));
                    answer()
                },
            ),
        )
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    (format!("http://{address}"), seen)
}

fn true_answer() -> Response {
    Json(json!({"model": "judge-31b", "verdicts": {"true_statement": {"type": "boolean", "value": true, "probabilities": {"true": 0.97, "false": 0.03}, "confidence": 0.9}}})).into_response()
}

#[sqlx::test(migrations = "./migrations")]
async fn administrators_point_decisions_at_another_endpoint_with_a_sealed_key(pool: PgPool) {
    let app = fixture(&pool, Some(SecretKey::from_bytes([9; 32]))).await;
    let admin = session(&pool, "admin").await;
    let person = session(&pool, "user").await;

    let (status, saved) = call(&app, &admin, "GET", "/api/admin/decisions", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["endpoint"], "gateway");
    assert_eq!(saved["checkRuns"], true);
    assert_eq!(saved["hasKey"], false);
    assert_eq!(saved["canStoreKey"], true);
    assert_eq!(saved["gateway"], false);

    let change = json!({"endpoint": "custom", "url": "https://d.example.com", "key": "sk-test-1234", "model": "judge-31b"});
    let (status, _) = call(
        &app,
        &person,
        "PATCH",
        "/api/admin/decisions",
        change.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, saved) = call(&app, &admin, "PATCH", "/api/admin/decisions", change).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["endpoint"], "custom");
    assert_eq!(saved["url"], "https://d.example.com/v1/verdicts");
    assert_eq!(saved["model"], "judge-31b");
    assert_eq!(saved["hasKey"], true);
    assert!(!saved.to_string().contains("sk-test-1234"));
    let sealed: Vec<u8> = sqlx::query_scalar("SELECT api_key FROM decisions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!sealed.windows(12).any(|w| w == b"sk-test-1234"));
    let audit: Value =
        sqlx::query_scalar("SELECT metadata FROM audit_events WHERE action='decisions.update'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!audit.to_string().contains("sk-test-1234"));
    assert_eq!(audit["keyChanged"], true);

    // Every replica reads the same settings, key included.
    let settings = Decisions::new(pool.clone(), Some(SecretKey::from_bytes([9; 32])))
        .settings()
        .await
        .unwrap();
    assert!(
        matches!(&settings.endpoint, Endpoint::Custom { key: Key::Set(k), .. } if k == "sk-test-1234")
    );
    assert_eq!(settings.model.as_deref(), Some("judge-31b"));
    // Another secret key can't open it, and says so rather than sending no key.
    let other = Decisions::new(pool.clone(), Some(SecretKey::from_bytes([8; 32])))
        .settings()
        .await
        .unwrap();
    assert!(matches!(
        &other.endpoint,
        Endpoint::Custom {
            key: Key::Unreadable,
            ..
        }
    ));

    // Leaving the key out keeps it; null removes it.
    let (_, saved) = call(
        &app,
        &admin,
        "PATCH",
        "/api/admin/decisions",
        json!({"checkRuns": false}),
    )
    .await;
    assert_eq!(
        (saved["hasKey"].clone(), saved["checkRuns"].clone()),
        (json!(true), json!(false))
    );
    let (_, saved) = call(
        &app,
        &admin,
        "PATCH",
        "/api/admin/decisions",
        json!({"key": null}),
    )
    .await;
    assert_eq!(saved["hasKey"], false);

    for wrong in [
        json!({"endpoint": "elsewhere"}),
        json!({"url": "https://user:pw@d.example.com"}),
        json!({"apiKey": "x"}),
    ] {
        let (status, _) = call(&app, &admin, "PATCH", "/api/admin/decisions", wrong.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{wrong}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_key_is_refused_without_the_secret_key_that_seals_it(pool: PgPool) {
    let app = fixture(&pool, None).await;
    let admin = session(&pool, "admin").await;
    let (status, body) = call(
        &app,
        &admin,
        "PATCH",
        "/api/admin/decisions",
        json!({"endpoint": "custom", "url": "https://d.example.com", "key": "sk-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string().contains("OBLETH_CHAT_SECRET_KEY_FILE"),
        "{body}"
    );
    // An endpoint that needs no key is fine.
    let (status, saved) = call(
        &app,
        &admin,
        "PATCH",
        "/api/admin/decisions",
        json!({"endpoint": "custom", "url": "https://d.example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["canStoreKey"], false);
}

#[sqlx::test(migrations = "./migrations")]
async fn check_asks_with_the_form_before_it_is_saved(pool: PgPool) {
    let app = fixture(&pool, Some(SecretKey::from_bytes([9; 32]))).await;
    let admin = session(&pool, "admin").await;
    let (url, seen) = service(true_answer).await;

    // Unsaved values are used as given, the new key included.
    let form = json!({"endpoint": "custom", "url": url, "key": "sk-unsaved", "model": "judge-31b"});
    let (status, checked) = call(&app, &admin, "POST", "/api/admin/decisions/check", form).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(checked["ok"], true, "{checked}");
    assert_eq!(checked["model"], "judge-31b");
    assert_eq!(checked["yes"], 0.97);
    let (auth, body) = seen.0.lock().unwrap()[0].clone();
    assert_eq!(auth.as_deref(), Some("Bearer sk-unsaved"));
    assert_eq!(body["model"], "judge-31b");
    assert_eq!(body["questions"]["true_statement"]["type"], "boolean");
    // ...and nothing was saved.
    let endpoint: String = sqlx::query_scalar("SELECT endpoint FROM decisions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(endpoint, "gateway");

    // The gateway can't answer when Chat has none.
    let (_, checked) = call(
        &app,
        &admin,
        "POST",
        "/api/admin/decisions/check",
        json!({}),
    )
    .await;
    assert_eq!(
        checked,
        json!({"ok": false, "error": "Chat has no gateway to ask", "ms": checked["ms"]})
    );
    let (_, checked) = call(
        &app,
        &admin,
        "POST",
        "/api/admin/decisions/check",
        json!({"endpoint": "off"}),
    )
    .await;
    assert_eq!(checked["error"], "Decisions are off");
}

#[sqlx::test(migrations = "./migrations")]
async fn check_says_why_an_endpoint_gave_no_answer(pool: PgPool) {
    let app = fixture(&pool, None).await;
    let admin = session(&pool, "admin").await;
    let ask = |url: String| json!({"endpoint": "custom", "url": url});

    let (url, _) = service(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "no such route"}})),
        )
            .into_response()
    })
    .await;
    let (_, checked) = call(&app, &admin, "POST", "/api/admin/decisions/check", ask(url)).await;
    assert_eq!(checked["error"], "The endpoint answered 404: no such route");

    // A redirect is not followed, so a key never goes somewhere unchosen.
    let (url, _) = service(|| {
        Redirect::temporary("https://elsewhere.example.com/v1/verdicts").into_response()
    })
    .await;
    let (_, checked) = call(&app, &admin, "POST", "/api/admin/decisions/check", ask(url)).await;
    assert!(
        checked["error"].as_str().unwrap().contains("a redirect"),
        "{checked}"
    );

    let (url, _) = service(|| Json(json!({"verdicts": {"true_statement": {"value": true, "probabilities": {"true": 0.5, "false": 0.5}, "confidence": 0.0, "answered": false}}})).into_response()).await;
    let (_, checked) = call(&app, &admin, "POST", "/api/admin/decisions/check", ask(url)).await;
    assert_eq!(checked["error"], "The model gave no answer");
}
