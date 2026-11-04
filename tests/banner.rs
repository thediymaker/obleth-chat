//! The banner: administrators set it, every signed-in person gets it with their
//! session, and signed-out visitors only when it is meant for the sign-in page.
//! Requires a disposable DATABASE_URL.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    banner, AppState,
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
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Banner test',$3)")
        .bind(id)
        .bind(format!("banner-{}", id.simple()))
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
    banner::router(state).merge(sessions)
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
async fn administrators_set_the_banner_and_everyone_gets_it_with_their_session(pool: PgPool) {
    let app = fixture(&pool).await;
    let admin = session(&pool, "admin").await;
    let person = session(&pool, "user").await;

    let (status, saved) = call(&app, Some(&admin), "GET", "/api/admin/banner", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["text"], "");
    assert_eq!(saved["link"], Value::Null);
    assert_eq!(saved["tone"], "info");
    assert_eq!(saved["signedOut"], false);
    assert_eq!(saved["dismissible"], false);
    let (_, signed_in) = call(&app, Some(&person), "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_in["authenticated"], true);
    assert_eq!(signed_in["banner"], Value::Null);

    let change = json!({"text": " In development:\n features may change. ", "link": {"label": "Send feedback", "url": "https://example.edu/feedback"}, "tone": "warning", "signedOut": false, "dismissible": true});
    let (status, _) = call(
        &app,
        Some(&person),
        "PATCH",
        "/api/admin/banner",
        change.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, Some(&person), "GET", "/api/admin/banner", Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, saved) = call(&app, Some(&admin), "PATCH", "/api/admin/banner", change).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["text"], "In development: features may change.");
    assert_eq!(
        saved["link"],
        json!({"label": "Send feedback", "url": "https://example.edu/feedback"})
    );
    assert_eq!(saved["tone"], "warning");
    assert_eq!(saved["dismissible"], true);
    let audit: Value =
        sqlx::query_scalar("SELECT metadata FROM audit_events WHERE action='banner.update'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audit["text"], "In development: features may change.");

    let shown = json!({"text": "In development: features may change.", "link": {"label": "Send feedback", "url": "https://example.edu/feedback"}, "tone": "warning", "dismissible": true});
    let (_, signed_in) = call(&app, Some(&person), "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_in["banner"], shown);
    // Only for people who are signed in until it is meant for the sign-in page too.
    let (_, signed_out) = call(&app, None, "GET", "/api/session", Value::Null).await;
    assert_eq!(signed_out["authenticated"], false);
    assert_eq!(signed_out["banner"], Value::Null);
    let change =
        json!({"text": "In development", "tone": "info", "signedOut": true, "dismissible": false});
    let (status, saved) = call(&app, Some(&admin), "PATCH", "/api/admin/banner", change).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["link"], Value::Null);
    let (_, signed_out) = call(&app, None, "GET", "/api/session", Value::Null).await;
    assert_eq!(
        signed_out["banner"],
        json!({"text": "In development", "link": null, "tone": "info", "dismissible": false})
    );

    // Blank text turns it off everywhere and keeps the rest for next time.
    let off = json!({"text": "  ", "tone": "info", "signedOut": true, "dismissible": false});
    let (status, saved) = call(&app, Some(&admin), "PATCH", "/api/admin/banner", off).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["text"], "");
    assert_eq!(saved["signedOut"], true);
    for user in [None, Some(&person)] {
        let (_, session) = call(&app, user, "GET", "/api/session", Value::Null).await;
        assert_eq!(session["banner"], Value::Null);
    }

    // A bad change is refused and saves nothing.
    for bad in [
        json!({"text": "x".repeat(201), "tone": "info", "signedOut": false, "dismissible": false}),
        json!({"text": "Hi", "link": {"label": "Go", "url": "javascript:alert(1)"}, "tone": "info", "signedOut": false, "dismissible": false}),
        json!({"text": "Hi", "tone": "red", "signedOut": false, "dismissible": false}),
    ] {
        let (status, _) = call(&app, Some(&admin), "PATCH", "/api/admin/banner", bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (_, saved) = call(&app, Some(&admin), "GET", "/api/admin/banner", Value::Null).await;
    assert_eq!(saved["text"], "");
}
