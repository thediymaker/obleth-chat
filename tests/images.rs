//! Generated images: the gateway sends them inline as data URLs in one large content
//! event; Chat stores them, links them from the saved reply and serves them only to
//! their owner. Requires a disposable DATABASE_URL.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{sse::Event, Sse},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    gateway::Gateway,
    router, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::convert::Infallible;
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";

/// A PNG signature followed by enough bytes to make the event about 2 MiB.
fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..1_500_000u32).map(|i| (i % 251) as u8));
    bytes
}

async fn app(pool: PgPool) -> Router {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mocks = Router::new()
        .route("/v1/models", get(|| async { Json(json!({"data":[{"id":"fixture-model"}]})) }))
        .route(
            "/v1/chat/completions",
            post(|| async {
                let image = format!("\n\n![A moth and a crescent moon](data:image/png;base64,{})", STANDARD.encode(png()));
                let stream = async_stream::stream! {
                    yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"content":"Here is the badge."}}]}).to_string()));
                    yield Ok(Event::default().data(json!({"choices":[{"delta":{"content":image}}]}).to_string()));
                    yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string()));
                    yield Ok(Event::default().data("[DONE]"));
                };
                Sse::new(stream)
            }),
        );
    tokio::spawn(async move { axum::serve(listener, mocks).await.unwrap() });
    let pem = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(Default::default())
        .unwrap();
    let gateway = Gateway::new(
        format!("http://{address}").parse().unwrap(),
        pem.as_bytes(),
        ORIGIN.into(),
        "obleth".into(),
    )
    .unwrap();
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
    let mut state = AppState::new(auth, pool, Some(gateway), 4);
    state.followups_enabled = false;
    router(state, "web".into())
}

type Session = (String, String);

fn req(method: &str, path: &str, body: Value, session: &Session) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("origin", ORIGIN)
        .header("cookie", &session.0)
        .header("x-csrf-token", &session.1)
        .header("content-type", "application/json")
        .body(Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    session: &Session,
) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(req(method, path, body, session))
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

async fn login(app: &Router, name: &str) -> Session {
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/auth/login",
            json!({"username":name,"password":PASSWORD}),
            &(String::new(), String::new()),
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

#[sqlx::test(migrations = "./migrations")]
async fn generated_images_are_stored_linked_and_private(pool: PgPool) {
    let app = app(pool).await;
    let admin = login(&app, "test-admin").await;
    let (_, chat) = call(
        &app,
        "POST",
        "/api/conversations",
        json!({"title":"Brand","model":"fixture-model"}),
        &admin,
    )
    .await;
    let chat: Value = serde_json::from_slice(&chat).unwrap();
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let (status, streamed) = call(
        &app,
        "POST",
        &format!("/api/conversations/{id}/messages"),
        json!({"content":"Make me a logo","model":"fixture-model"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let streamed = String::from_utf8(streamed).unwrap();
    assert!(
        !streamed.contains("data:image/"),
        "the browser gets a link, not the bytes"
    );

    let (_, detail) = call(
        &app,
        "GET",
        &format!("/api/conversations/{id}"),
        Value::Null,
        &admin,
    )
    .await;
    let detail: Value = serde_json::from_slice(&detail).unwrap();
    let reply = detail["conversation"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|m| m["role"] == "assistant")
        .unwrap()
        .clone();
    assert_eq!(reply["status"], "completed", "{reply}");
    let content = reply["content"].as_str().unwrap();
    let start = content.find("](/api/images/").expect(content) + 2;
    assert!(
        content.starts_with("Here is the badge.\n\n![A moth and a crescent moon]("),
        "{content}"
    );
    let link = &content[start..content.len() - 1];

    let response = app
        .clone()
        .oneshot(req("GET", link, Value::Null, &admin))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
        png()
    );

    let (status, _) = call(
        &app,
        "POST",
        "/api/admin/users",
        json!({"username":"other","password":PASSWORD,"displayName":"Other","role":"user"}),
        &admin,
    )
    .await;
    assert!(status.is_success());
    let other = login(&app, "other").await;
    assert_eq!(
        call(&app, "GET", link, Value::Null, &other).await.0,
        StatusCode::NOT_FOUND
    );
}
