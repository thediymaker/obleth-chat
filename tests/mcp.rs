//! Personal API tokens and the MCP endpoint end to end, against a stand-in gateway.
//! Requires a disposable DATABASE_URL.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{sse::Event, Sse},
    routing::{get, post},
    Json, Router,
};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    gateway::Gateway,
    router, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{convert::Infallible, time::Duration};
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";

async fn fixture(pool: PgPool) -> Router {
    let pem = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(Default::default())
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let gateway = Gateway::new(base, pem.as_bytes(), ORIGIN.into(), "obleth".into()).unwrap();
    let mocks = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"fixture-model"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                let last = body["messages"].as_array().unwrap().last().unwrap()["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                Sse::new(async_stream::stream! {
                    if last == "take your time" { tokio::time::sleep(Duration::from_secs(30)).await; }
                    yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"content":format!("You said: {last}")}}]}).to_string()));
                    yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}).to_string()));
                    yield Ok(Event::default().data("[DONE]"));
                })
            }),
        );
    tokio::spawn(async move { axum::serve(listener, mocks).await.unwrap() });
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

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn browser(method: &str, path: &str, body: Value, cookie: &str, csrf: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("origin", ORIGIN)
        .header("cookie", cookie)
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn login(app: &Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(browser(
            "POST",
            "/api/auth/login",
            json!({"username":"test-admin","password":PASSWORD}),
            "",
            "",
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
        .to_string();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let session: Value = serde_json::from_slice(&bytes).unwrap();
    (cookie, session["csrfToken"].as_str().unwrap().to_string())
}

fn rpc(token: &str, method: &str, params: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        ))
        .unwrap()
}

/// A tool's result: its structured content, or its error text.
async fn tool(app: &Router, token: &str, name: &str, arguments: Value) -> Result<Value, String> {
    let (status, _, body) = send(
        app,
        rpc(
            token,
            "tools/call",
            json!({"name":name,"arguments":arguments}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["result"];
    if result["isError"] == true {
        Err(result["content"][0]["text"].as_str().unwrap().to_owned())
    } else {
        Ok(result["structuredContent"].clone())
    }
}

async fn settled(app: &Router, token: &str, id: &str) -> Value {
    for _ in 0..100 {
        let chat = tool(app, token, "get_chat", json!({"id":id}))
            .await
            .unwrap();
        if chat["state"] != "running" {
            return chat;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("chat {id} kept running");
}

async fn create_token(app: &Router, cookie: &str, csrf: &str, body: Value) -> (StatusCode, Value) {
    let (status, _, body) =
        send(app, browser("POST", "/api/auth/tokens", body, cookie, csrf)).await;
    (status, body)
}

#[sqlx::test(migrator = "obleth_chat::MIGRATOR")]
async fn a_token_runs_chats_over_mcp_and_reaches_nothing_else(pool: PgPool) {
    let app = fixture(pool.clone()).await;
    let (cookie, csrf) = login(&app).await;

    // Tokens are made from the browser, with checked names and lifetimes.
    let (status, _) = create_token(&app, &cookie, &csrf, json!({"name":"x","days":3})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = create_token(&app, &cookie, "wrong", json!({"name":"x","days":7})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, created) =
        create_token(&app, &cookie, &csrf, json!({"name":"Claude Code","days":7})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("obc_") && token.len() == 47);
    assert_eq!(
        created["apiToken"]["scopes"],
        json!(["chats:read", "chats:write"])
    );
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM api_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(stored, token.as_bytes(), "only a hash is kept");

    // The token is not a browser session: the rest of the API refuses it.
    let (status, _, _) = send(
        &app,
        Request::builder()
            .uri("/api/conversations")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // And the MCP endpoint refuses anything but a live token, or another site's page.
    let (status, headers, _) = send(&app, rpc("obc_nope", "ping", json!({}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers["www-authenticate"]
        .to_str()
        .unwrap()
        .starts_with("Bearer"));
    let mut cross = rpc(&token, "ping", json!({}));
    cross
        .headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    assert_eq!(send(&app, cross).await.0, StatusCode::FORBIDDEN);

    let (_, _, init) = send(
        &app,
        rpc(
            &token,
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}),
        ),
    )
    .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let notice = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        ))
        .unwrap();
    assert_eq!(send(&app, notice).await.0, StatusCode::ACCEPTED);
    let (_, _, listed) = send(&app, rpc(&token, "tools/list", json!({}))).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "list_chats",
            "get_chat",
            "list_models",
            "start_chat",
            "send_message",
            "stop_chat"
        ]
    );

    // A chat started over MCP runs on its own and is the person's like any other.
    let models = tool(&app, &token, "list_models", json!({})).await.unwrap();
    assert!(models["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == "fixture-model"));
    let started = tool(
        &app,
        &token,
        "start_chat",
        json!({"message":"Plan my week","model":"fixture-model"}),
    )
    .await
    .unwrap();
    let id = started["id"].as_str().unwrap().to_owned();
    assert_eq!(started["title"], "Plan my week");
    assert_eq!(started["url"], format!("{ORIGIN}/#chat/{id}"));
    let chat = settled(&app, &token, &id).await;
    assert_eq!(chat["state"], "completed", "{chat}");
    assert_eq!(chat["messages"][1]["content"], "You said: Plan my week");
    let (status, _, page) = send(
        &app,
        browser(
            "GET",
            &format!("/api/conversations/{id}"),
            json!({}),
            &cookie,
            &csrf,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        page["conversation"]["messages"].as_array().unwrap().len(),
        2
    );

    tool(
        &app,
        &token,
        "send_message",
        json!({"id":id,"message":"take your time"}),
    )
    .await
    .unwrap();
    // One reply at a time per chat.
    let busy = tool(
        &app,
        &token,
        "send_message",
        json!({"id":id,"message":"again"}),
    )
    .await;
    assert!(busy.is_err());
    let stop = tool(&app, &token, "stop_chat", json!({"id":id}))
        .await
        .unwrap();
    assert_eq!(stop["stopped"], true);
    let chat = settled(&app, &token, &id).await;
    assert_ne!(chat["state"], "completed");
    let listed = tool(&app, &token, "list_chats", json!({})).await.unwrap();
    assert_eq!(listed["chats"][0]["id"], id.as_str());
    assert!(
        tool(&app, &token, "get_chat", json!({"id":uuid::Uuid::new_v4()}))
            .await
            .is_err()
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action IN ('mcp_chat_started','mcp_message_sent','mcp_chat_stopped','api_token_created')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 4);

    // A read-only token sees chats but cannot run them.
    let (_, reader) = create_token(
        &app,
        &cookie,
        &csrf,
        json!({"name":"Reader","days":1,"readOnly":true}),
    )
    .await;
    let reader = reader["token"].as_str().unwrap().to_owned();
    let (_, _, listed) = send(&app, rpc(&reader, "tools/list", json!({}))).await;
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 3);
    assert!(tool(&app, &reader, "start_chat", json!({"message":"hi"}))
        .await
        .is_err());
    assert!(tool(&app, &reader, "get_chat", json!({"id":id}))
        .await
        .is_ok());

    // Each token has a per-minute allowance.
    sqlx::query("UPDATE api_tokens SET window_calls=120, window_start=NOW() WHERE name='Reader'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, rpc(&reader, "ping", json!({}))).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );

    // Listing never shows the secret, and revoking ends the token at once.
    let (_, _, tokens) = send(
        &app,
        browser("GET", "/api/auth/tokens", json!({}), &cookie, &csrf),
    )
    .await;
    assert_eq!(tokens["tokens"].as_array().unwrap().len(), 2);
    assert!(!tokens.to_string().contains(&token[4..]));
    let first = created["apiToken"]["id"].as_str().unwrap();
    let (status, _, _) = send(
        &app,
        browser(
            "DELETE",
            &format!("/api/auth/tokens/{first}"),
            json!({}),
            &cookie,
            &csrf,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        send(&app, rpc(&token, "ping", json!({}))).await.0,
        StatusCode::UNAUTHORIZED
    );

    // Expired tokens stop, and an old sign-in cannot make new ones.
    sqlx::query("UPDATE api_tokens SET expires_at=NOW() - INTERVAL '1 second', window_calls=0")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, rpc(&reader, "ping", json!({}))).await.0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE auth_sessions SET created_at=NOW() - INTERVAL '13 hours'")
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = create_token(&app, &cookie, &csrf, json!({"name":"Later","days":7})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["message"], "Sign in again to create a token");
}

#[sqlx::test(migrator = "obleth_chat::MIGRATOR")]
async fn changing_the_password_ends_the_tokens(pool: PgPool) {
    let app = fixture(pool.clone()).await;
    let (cookie, csrf) = login(&app).await;
    let (_, created) = create_token(&app, &cookie, &csrf, json!({"name":"Laptop","days":30})).await;
    let token = created["token"].as_str().unwrap().to_owned();
    assert_eq!(
        send(&app, rpc(&token, "ping", json!({}))).await.0,
        StatusCode::OK
    );
    let (status, _, body) = send(
        &app,
        browser(
            "POST",
            "/api/auth/password",
            json!({"currentPassword":PASSWORD,"newPassword":"another-fixture-password-43"}),
            &cookie,
            &csrf,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        send(&app, rpc(&token, "ping", json!({}))).await.0,
        StatusCode::UNAUTHORIZED
    );
}
