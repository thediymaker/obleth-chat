//! Workspaces with and without a preview domain: without one, workspaces still run,
//! Chat says previews are off and leaves preview-like hosts alone. (The frame policy
//! is set once per process, so it isn't compared across these routers.)
//! Requires a disposable DATABASE_URL; no cluster is contacted.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    router,
    sandbox::{SandboxConfig, Sandboxes},
    AppState,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";

async fn app(pool: PgPool, preview_domain: Option<&str>) -> Router {
    // The Kubernetes client points nowhere, so nothing here starts a pod.
    let ca = std::env::temp_dir().join("obleth-workspaces-test-ca.pem");
    std::fs::write(&ca, "").unwrap();
    std::env::set_var("OBLETH_CHAT_KUBE_API", "https://127.0.0.1:9");
    std::env::set_var("OBLETH_CHAT_KUBE_CA_FILE", &ca);
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: ORIGIN.into(),
            session_hours: 12,
            bootstrap_username: Some("test-admin".into()),
            bootstrap_password: Some(PASSWORD.into()),
            oidc: None,
            login_rate_per_minute: 1000,
        },
    )
    .await
    .unwrap();
    let config = SandboxConfig {
        namespace: "nowhere".into(),
        image: "example.invalid/none".into(),
        runtime_class: None,
        storage_class: None,
        storage: "1Gi".into(),
        disk_limit: 1 << 30,
        cpu_limit: "1".into(),
        memory_limit: "1Gi".into(),
        everyone: false,
        preview_domain: preview_domain.map(Into::into),
        preview_scheme: "http".into(),
        chat_origin: ORIGIN.into(),
        idle: Duration::from_secs(3600),
        retention: Duration::from_secs(86400),
    };
    let mut state = AppState::new(auth, pool.clone(), None, 4);
    state.sandboxes = Some(Sandboxes::new(config, pool).unwrap());
    router(state, "web".into())
}

fn req(path: &str, host: &str, session: &(String, String), body: Option<Value>) -> Request<Body> {
    Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("host", host)
        .header("origin", ORIGIN)
        .header("cookie", &session.0)
        .header("x-csrf-token", &session.1)
        .header("content-type", "application/json")
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap()
}

async fn login(app: &Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(req(
            "/api/auth/login",
            "localhost:8085",
            &(String::new(), String::new()),
            Some(json!({"username":"test-admin","password":PASSWORD})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (cookie, body["csrfToken"].as_str().unwrap().to_owned())
}

async fn capabilities(app: &Router, host: &str, session: &(String, String)) -> Value {
    let response = app
        .clone()
        .oneshot(req("/api/capabilities", host, session, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{host}");
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn workspaces_run_without_a_preview_domain(pool: PgPool) {
    let app = app(pool, None).await;
    let admin = login(&app).await;
    let caps = capabilities(&app, "localhost:8085", &admin).await;
    assert_eq!(caps["features"]["workspaces"], true, "{caps}");
    assert_eq!(caps["features"]["previews"], false, "{caps}");
    // A host that looks like a preview is just Chat.
    let caps = capabilities(&app, "abcdef.sbx.example.com", &admin).await;
    assert_eq!(caps["features"]["previews"], false);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_preview_domain_turns_previews_on(pool: PgPool) {
    let app = app(pool, Some("sbx.example.com")).await;
    let admin = login(&app).await;
    let caps = capabilities(&app, "localhost:8085", &admin).await;
    assert_eq!(caps["features"]["previews"], true, "{caps}");
    // Preview hosts are answered by the preview proxy, never by Chat's routes.
    let response = app
        .clone()
        .oneshot(req(
            "/api/capabilities",
            "abcdef.sbx.example.com",
            &admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
