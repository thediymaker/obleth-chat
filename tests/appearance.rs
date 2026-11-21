//! The site's look: administrators set it, and everyone gets what is set with their
//! session, signed in or not. A setting left unset stays out of the answer.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    appearance,
    auth::{Auth, AuthConfig},
    AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
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
        "INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Appearance test',$3)",
    )
    .bind(id)
    .bind(format!("appearance-{}", id.simple()))
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

async fn fixture(pool: &PgPool) -> Router {
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
    let state = AppState::new(auth, pool.clone(), None, 4);
    let sessions = state.auth.router();
    appearance::router(state).merge(sessions)
}

async fn call(
    app: &Router,
    user: Option<&Session>,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", ORIGIN)
        .header("content-type", "application/json");
    if let Some(user) = user {
        request = request
            .header("cookie", &user.cookie)
            .header("x-csrf-token", &user.csrf);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn administrators_set_the_look_and_every_page_gets_it_with_the_session(pool: PgPool) {
    let app = fixture(&pool).await;
    let admin = session(&pool, "admin").await;
    let person = session(&pool, "user").await;

    // Nothing is set at first: the installation's own file decides everything.
    let (status, saved) = call(
        &app,
        Some(&admin),
        "GET",
        "/api/admin/appearance",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["accent"], Value::Null);
    let (_, signed_out) = call(&app, None, "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_out["appearance"], json!({}));

    // Only administrators read and change it.
    let change = json!({"siteName": "Research Computing", "accent": "#5FD3BC", "defaultTheme": "dark", "personalAccent": false, "corners": null});
    let (status, _) = call(
        &app,
        Some(&person),
        "PATCH",
        "/api/admin/appearance",
        change.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        Some(&person),
        "GET",
        "/api/admin/appearance",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, None, "PATCH", "/api/admin/appearance", change.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, saved) = call(&app, Some(&admin), "PATCH", "/api/admin/appearance", change).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["accent"], "#5fd3bc");
    assert_eq!(saved["corners"], Value::Null);
    let shown = json!({"siteName": "Research Computing", "accent": "#5fd3bc", "defaultTheme": "dark", "personalAccent": false});
    let (_, signed_out) = call(&app, None, "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_out["appearance"], shown);
    let (_, signed_in) = call(&app, Some(&person), "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_in["appearance"], shown);
    let logged: Value =
        sqlx::query_scalar("SELECT metadata FROM audit_events WHERE action='appearance.update'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(logged, shown);

    // A bad color changes nothing; nulls put everything back.
    let (status, _) = call(
        &app,
        Some(&admin),
        "PATCH",
        "/api/admin/appearance",
        json!({"accent": "teal"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, signed_out) = call(&app, None, "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_out["appearance"], shown);
    let (status, _) = call(
        &app,
        Some(&admin),
        "PATCH",
        "/api/admin/appearance",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, signed_out) = call(&app, None, "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_out["appearance"], json!({}));
}
