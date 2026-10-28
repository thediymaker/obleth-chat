//! End-to-end router + real HTTP gateway tests. Requires a disposable DATABASE_URL.
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    response::{sse::Event, IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    gateway::Gateway,
    router, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";
#[derive(Clone)]
struct Fake {
    jwks: Value,
    seen: Arc<Mutex<Vec<Value>>>,
}
fn verify(headers: &HeaderMap, f: &Fake) -> Value {
    let raw = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap();
    let jwk: &Value = &f.jwks["keys"][0];
    let key = jsonwebtoken::DecodingKey::from_rsa_components(
        jwk["n"].as_str().unwrap(),
        jwk["e"].as_str().unwrap(),
    )
    .unwrap();
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_issuer(&[ORIGIN]);
    validation.set_audience(&["obleth"]);
    let decoded = jsonwebtoken::decode::<Value>(raw, &key, &validation).unwrap();
    assert_eq!(decoded.header.kid.as_deref(), jwk["kid"].as_str());
    assert!(
        decoded.claims["exp"].as_u64().unwrap() - decoded.claims["iat"].as_u64().unwrap() <= 300
    );
    decoded.claims
}
async fn fake_models(State(f): State<Fake>, headers: HeaderMap) -> Json<Value> {
    let _ = verify(&headers, &f);
    Json(json!({"data":[{"id":"fixture-model"}]}))
}
async fn fake_chat(
    State(f): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let claims = verify(&headers, &f);
    assert!(headers.get("x-obleth-session-id").is_some());
    assert_eq!(body["stream_options"]["include_usage"], true);
    // No output limit is sent unless someone set one: the model's own applies.
    assert!(body.get("max_tokens").is_none(), "{body}");
    let messages = body["messages"].as_array().unwrap();
    let mode = messages.last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    // Chat's note after a dropped connection: the "model" carries on, or drops again.
    let dropped = mode.starts_with("[Chat: the connection to the model dropped");
    let always = messages.iter().any(|m| m["content"] == "always-broken");
    f.seen
        .lock()
        .unwrap()
        .push(json!({"claims":claims,"body":body,"effort":headers.get("x-obleth-effort").and_then(|v| v.to_str().ok())}));
    // Auto is the gateway's router: the stream names the model it chose.
    let served = (body["model"] == "auto").then_some("routed-model");
    let stream = async_stream::stream! {
        if dropped {
            if !always {
                yield Ok::<_,Infallible>(Event::default().data(json!({"choices":[{"delta":{"content":" and the rest."},"finish_reason":"stop"}]}).to_string()));
                yield Ok(Event::default().data("[DONE]"));
            }
            return;
        }
        if mode == "oversized-reasoning" {
            for _ in 0..2 {
                yield Ok::<_,Infallible>(Event::default().data(json!({"choices":[{"delta":{"reasoning":"r".repeat(512 * 1024)}}]}).to_string()));
            }
        } else {
            // Providers may mirror aliases; a client must display each token once.
            yield Ok::<_,Infallible>(Event::default().data(json!({"choices":[{"delta":{"reasoning_content":"Plan α", "reasoning":"Plan α", "content":null}}]}).to_string()));
            yield Ok(Event::default().data(json!({"choices":[{"delta":{"reasoning":" then answer.", "reasoning_details":[{"type":"reasoning.encrypted","data":"never-display"}]}}]}).to_string()));
        }
        if mode == "thinking-only" {tokio::time::sleep(Duration::from_secs(30)).await;}
        yield Ok::<_,Infallible>(Event::default().data(json!({"model":served,"choices":[{"delta":{"content":if mode=="slow"{"Partial answer"}else{"Hello α"}}}]}).to_string()));
        if mode=="slow" {tokio::time::sleep(Duration::from_secs(30)).await;}
        if mode=="background" {tokio::time::sleep(Duration::from_secs(2)).await;}
        if mode!="broken" && mode!="always-broken" {yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}).to_string()));yield Ok(Event::default().data("[DONE]"));}
    };
    (
        [("x-obleth-request-id", "fixture-request")],
        Sse::new(stream),
    )
}
async fn fixture(pool: PgPool) -> (Router, AppState, Fake, tokio::task::JoinHandle<()>) {
    let pem = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(Default::default())
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let gateway = Gateway::new(base, pem.as_bytes(), ORIGIN.into(), "obleth".into()).unwrap();
    let fake = Fake {
        jwks: gateway.jwks(),
        seen: Arc::new(Mutex::new(vec![])),
    };
    let r = Router::new()
        .route("/v1/models", get(fake_models))
        .route("/v1/chat/completions", post(fake_chat))
        .with_state(fake.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, r).await.unwrap();
    });
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: ORIGIN.into(),
            session_hours: 12,
            bootstrap_username: Some("test-admin".into()),
            bootstrap_password: Some(PASSWORD.into()),
            oidc: None,
            login_rate_per_minute: 1000,
            ..AuthConfig::default()
        },
    )
    .await
    .unwrap();
    let mut state = AppState::new(auth, pool, Some(gateway), 4);
    state.followups_enabled = false;
    (router(state.clone(), "web".into()), state, fake, task)
}
fn req(method: &str, path: &str, body: Value, cookie: &str, csrf: &str) -> Request<Body> {
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
async fn value(r: axum::response::Response) -> Value {
    serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
async fn login(app: &Router, name: &str) -> (String, String, Value) {
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/auth/login",
            json!({"username":name,"password":PASSWORD}),
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
    let session = value(response).await;
    (
        cookie,
        session["csrfToken"].as_str().unwrap().to_string(),
        session,
    )
}
async fn conversation(app: &Router, cookie: &str, csrf: &str) -> String {
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/conversations",
            json!({"title":"Test conversation","model":"fixture-model"}),
            cookie,
            csrf,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    value(response).await["conversation"]["id"]
        .as_str()
        .unwrap()
        .into()
}
async fn fetch(app: &Router, path: &str, headers: &[(&str, &str)]) -> axum::response::Response {
    let mut request = Request::builder().uri(path);
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn static_files_revalidate_and_compress_while_api_stays_no_store(pool: PgPool) {
    let (app, _, _, task) = fixture(pool).await;
    let script = fetch(&app, "/app.js", &[("accept-encoding", "gzip, br")]).await;
    assert_eq!(script.status(), StatusCode::OK);
    let headers = script.headers().clone();
    assert_eq!(headers["cache-control"], "no-cache");
    assert_eq!(headers["content-encoding"], "br");
    assert!(headers["vary"]
        .to_str()
        .unwrap()
        .contains("accept-encoding"));
    assert!(headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("script-src 'self'"));
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let modified = headers["last-modified"].to_str().unwrap();
    let cached = fetch(&app, "/app.js", &[("if-modified-since", modified)]).await;
    assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(cached.headers()["cache-control"], "no-cache");
    let index = fetch(&app, "/", &[("accept-encoding", "gzip")]).await;
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(index.headers()["cache-control"], "no-cache");
    assert_eq!(index.headers()["content-encoding"], "gzip");
    let image = fetch(
        &app,
        "/assets/favicon-32.png",
        &[("accept-encoding", "gzip")],
    )
    .await;
    assert_eq!(image.status(), StatusCode::OK);
    assert!(image.headers().get("content-encoding").is_none());
    let missing = fetch(&app, "/no-such-page.js", &[]).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.headers()["cache-control"], "no-store");
    for path in ["/api/session", "/api/missing", "/healthz"] {
        let response = fetch(&app, path, &[("accept-encoding", "gzip, br")]).await;
        assert_eq!(response.headers()["cache-control"], "no-store", "{path}");
        assert!(
            response.headers().get("content-encoding").is_none(),
            "{path}"
        );
    }
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn real_gateway_identity_stream_persistence_and_user_isolation(pool: PgPool) {
    let (app, state, fake, task) = fixture(pool.clone()).await;
    let (cookie, csrf, session) = login(&app, "test-admin").await;
    assert_eq!(
        app.clone()
            .oneshot(req("GET", "/api/models", json!(null), &cookie, ""))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let id = conversation(&app, &cookie, &csrf).await;
    let mut send = req(
        "POST",
        &format!("/api/conversations/{id}/messages"),
        json!({"content":"hello","model":"fixture-model"}),
        &cookie,
        &csrf,
    );
    send.headers_mut()
        .insert("accept-encoding", "gzip, br".parse().unwrap());
    let response = app.clone().oneshot(send).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers().get("content-encoding").is_none());
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("event: delta"));
    assert!(text.contains("event: reasoning"));
    assert_eq!(text.matches("Plan α").count(), 1);
    assert!(!text.contains("never-display"));
    assert!(text.contains("Hello α"));
    assert!(text.contains("\"status\":\"completed\""));
    let uid = Uuid::parse_str(session["user"]["id"].as_str().unwrap()).unwrap();
    let saved = state
        .store
        .get(uid, Uuid::parse_str(&id).unwrap())
        .await
        .unwrap();
    assert_eq!(saved.messages[1].status, "completed");
    assert_eq!(saved.messages[1].content, "Hello α");
    assert_eq!(saved.messages[1].reasoning_content, "Plan α then answer.");
    assert_eq!(saved.reasoning_effort, "medium");
    assert_eq!(
        saved.messages[1].usage.as_ref().unwrap()["total_tokens"],
        15
    );
    assert_eq!(
        saved.messages[1].request_id.as_deref(),
        Some("fixture-request")
    );
    assert_eq!(
        fake.seen.lock().unwrap()[0]["claims"]["sub"],
        uid.to_string()
    );
    assert_eq!(
        fake.seen.lock().unwrap()[0]["body"]["reasoning_effort"],
        "medium"
    );
    let user=app.clone().oneshot(req("POST","/api/admin/users",json!({"username":"second-user","password":PASSWORD,"displayName":"Second user","role":"user"}),&cookie,&csrf)).await.unwrap();
    assert_eq!(user.status(), StatusCode::CREATED);
    let (other, other_csrf, _) = login(&app, "second-user").await;
    assert_eq!(
        app.clone()
            .oneshot(req(
                "GET",
                &format!("/api/conversations/{id}"),
                json!(null),
                &other,
                ""
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.clone()
            .oneshot(req(
                "DELETE",
                &format!("/api/conversations/{id}"),
                json!(null),
                &other,
                &other_csrf
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    // Another owner's chat is rejected before any gateway request.
    let seen = fake.seen.lock().unwrap().len();
    assert_eq!(
        app.clone()
            .oneshot(req(
                "POST",
                &format!("/api/conversations/{id}/messages"),
                json!({"content":"intrude","model":"fixture-model"}),
                &other,
                &other_csrf
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fake.seen.lock().unwrap().len(), seen);
    let other_list = value(
        app.clone()
            .oneshot(req("GET", "/api/conversations", json!(null), &other, ""))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(other_list["conversations"], json!([]));
    let restarted = router(state.clone(), "web".into());
    let persisted = value(
        restarted
            .oneshot(req(
                "GET",
                &format!("/api/conversations/{id}"),
                json!(null),
                &cookie,
                "",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        persisted["conversation"]["messages"][1]["content"],
        "Hello α"
    );
    assert_eq!(
        persisted["conversation"]["messages"][1]["reasoningContent"],
        "Plan α then answer."
    );
    assert_eq!(persisted["conversation"]["reasoningEffort"], "medium");
    assert_eq!(
        persisted["conversation"]["messages"][1]["finishReason"],
        "stop"
    );
    assert_eq!(
        persisted["conversation"]["messages"][1]["settings"]["model"],
        "fixture-model"
    );
    let metrics = state.metrics.render();
    assert!(!metrics.contains(&id));
    assert!(!metrics.contains("test-admin"));
    assert!(metrics.contains("obleth_chat_generations_total{outcome=\"completed\"} 1"));
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn disconnect_keeps_running_explicit_stop_saves_partial_and_broken_stream_is_carried_on(
    pool: PgPool,
) {
    let (app, state, fake, task) = fixture(pool).await;
    let (cookie, csrf, session) = login(&app, "test-admin").await;
    let owner = Uuid::parse_str(session["user"]["id"].as_str().unwrap()).unwrap();
    let id = conversation(&app, &cookie, &csrf).await;
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"slow","model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let mut seen = String::new();
    while !seen.contains("Partial answer") {
        seen.push_str(&String::from_utf8_lossy(
            &stream.next().await.unwrap().unwrap(),
        ));
    }
    drop(stream);
    // Leaving the browser detaches only. Checkpointed output is readable from
    // another replica while the upstream response is still running.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let progress = state
        .store
        .get(owner, Uuid::parse_str(&id).unwrap())
        .await
        .unwrap();
    assert_eq!(progress.messages[1].status, "pending");
    assert_eq!(progress.messages[1].content, "Partial answer");
    let replica = router(state.clone(), "web".into());
    let stopped = replica
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/stop"),
            json!({"generationId":progress.active_generation_id.unwrap()}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(stopped.status(), StatusCode::OK);
    let saved = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let c = state
                .store
                .get(owner, Uuid::parse_str(&id).unwrap())
                .await
                .unwrap();
            if c.messages[1].status != "pending" {
                break c;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(saved.messages[1].status, "interrupted");
    assert_eq!(saved.messages[1].content, "Partial answer");
    assert_eq!(saved.messages[1].reasoning_content, "Plan α then answer.");
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"broken","model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let _ = response.into_body().collect().await.unwrap();
    let saved = state
        .store
        .get(owner, Uuid::parse_str(&id).unwrap())
        .await
        .unwrap();
    // A stream that drops once is carried on: the model is handed what it wrote.
    assert_eq!(saved.messages[3].status, "completed");
    assert_eq!(saved.messages[3].content, "Hello α and the rest.");
    assert_eq!(saved.messages[3].reasoning_content, "Plan α then answer.");
    let retry = fake.seen.lock().unwrap().last().unwrap()["body"]["messages"].clone();
    let retry = retry.as_array().unwrap();
    assert_eq!(retry[retry.len() - 2]["role"], "assistant");
    assert_eq!(retry[retry.len() - 2]["content"], "Hello α");
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"always-broken","model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let _ = response.into_body().collect().await.unwrap();
    let saved = state
        .store
        .get(owner, Uuid::parse_str(&id).unwrap())
        .await
        .unwrap();
    // One that keeps dropping ends the reply, keeping what arrived.
    assert_eq!(saved.messages[5].status, "error");
    assert_eq!(
        saved.messages[5].error_message.as_deref(),
        Some("The connection to the model kept dropping")
    );
    assert_eq!(saved.messages[5].content, "Hello α");
    assert_eq!(state.metrics.streams.get(), 0);
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn auto_goes_to_the_gateway_router_and_records_the_model_it_chose(pool: PgPool) {
    let (app, state, fake, task) = fixture(pool).await;
    let (cookie, csrf, session) = login(&app, "test-admin").await;
    let owner = Uuid::parse_str(session["user"]["id"].as_str().unwrap()).unwrap();
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/conversations",
            json!({"title":"Routed", "model":"auto"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let id = value(response).await["conversation"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for (effort, header) in [("high", json!("high")), ("medium", Value::Null)] {
        let response = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/api/conversations/{id}/messages"),
                json!({"content":"Question", "model":"auto", "reasoningEffort":effort}),
                &cookie,
                &csrf,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(
            body.contains("event: model") && body.contains("routed-model"),
            "{body}"
        );
        let seen = fake.seen.lock().unwrap().last().unwrap().clone();
        assert_eq!(seen["body"]["model"], "auto");
        assert_eq!(seen["effort"], header);
    }
    let chat = state
        .store
        .get(owner, Uuid::parse_str(&id).unwrap())
        .await
        .unwrap();
    let reply = chat
        .messages
        .iter()
        .rfind(|m| m.role == "assistant")
        .unwrap();
    let settings = reply.settings.clone().unwrap();
    assert_eq!(settings["model"], "routed-model");
    assert_eq!(settings["auto"], true);
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn effort_is_validated_persisted_forwarded_and_reasoning_is_not_replayed(pool: PgPool) {
    let (app, state, fake, task) = fixture(pool).await;
    let (cookie, csrf, session) = login(&app, "test-admin").await;
    let owner = Uuid::parse_str(session["user"]["id"].as_str().unwrap()).unwrap();
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/conversations",
            json!({"title":"Reasoning", "model":"fixture-model", "reasoningEffort":"low"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = value(response).await;
    assert_eq!(created["conversation"]["reasoningEffort"], "low");
    let id = created["conversation"]["id"].as_str().unwrap();
    let chat_id = Uuid::parse_str(id).unwrap();
    let path = format!("/api/conversations/{id}");
    // Invalid effort is rejected before writing user messages or calling upstream.
    for effort in [json!("max"), json!(7), json!({"level":"high"})] {
        let response = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("{path}/messages"),
                json!({"content":"Question", "model":"fixture-model", "reasoningEffort":effort}),
                &cookie,
                &csrf,
            ))
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ));
    }
    assert!(state
        .store
        .get(owner, chat_id)
        .await
        .unwrap()
        .messages
        .is_empty());
    assert!(fake.seen.lock().unwrap().is_empty());
    for effort in ["none", "low", "medium", "high"] {
        let response = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("{path}/messages"),
                json!({"content":"Question", "model":"fixture-model", "reasoningEffort":effort}),
                &cookie,
                &csrf,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.into_body().collect().await.unwrap();
        assert_eq!(
            fake.seen.lock().unwrap().last().unwrap()["body"]["reasoning_effort"],
            effort
        );
        assert_eq!(
            state
                .store
                .get(owner, chat_id)
                .await
                .unwrap()
                .reasoning_effort,
            effort
        );
    }
    let response = app
        .clone()
        .oneshot(req(
            "PATCH",
            &path,
            json!({"reasoningEffort":"low"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        value(response).await["conversation"]["reasoningEffort"],
        "low"
    );
    // Omitted optional field reuses the saved choice, including older clients.
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{path}/messages"),
            json!({"content":"Follow up", "model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let _ = response.into_body().collect().await.unwrap();
    let last = fake.seen.lock().unwrap().last().unwrap().clone();
    assert_eq!(last["body"]["reasoning_effort"], "low");
    for message in last["body"]["messages"].as_array().unwrap() {
        assert_eq!(message.as_object().unwrap().len(), 2);
        assert!(!message["content"].as_str().unwrap().contains("Plan α"));
    }
    let saved = state.store.get(owner, chat_id).await.unwrap();
    let branched = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{path}/branch"),
            json!({"throughMessageId":saved.messages[1].id}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let branched = value(branched).await;
    assert_eq!(branched["conversation"]["reasoningEffort"], "low");
    assert_eq!(
        branched["conversation"]["messages"][1]["reasoningContent"],
        "Plan α then answer."
    );
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn thinking_only_cancellation_and_streamed_combined_byte_limit(pool: PgPool) {
    let (app, state, _, task) = fixture(pool).await;
    let (cookie, csrf, session) = login(&app, "test-admin").await;
    let owner = Uuid::parse_str(session["user"]["id"].as_str().unwrap()).unwrap();
    let id = conversation(&app, &cookie, &csrf).await;
    let chat_id = Uuid::parse_str(&id).unwrap();
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"thinking-only","model":"fixture-model","reasoningEffort":"high"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let mut seen = String::new();
    while !seen.contains("then answer.") {
        seen.push_str(&String::from_utf8_lossy(
            &stream.next().await.unwrap().unwrap(),
        ));
    }
    let blocked = app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/api/conversations/{id}"),
            json!({"reasoningEffort":"none"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    drop(stream);
    let progress = state.store.get(owner, chat_id).await.unwrap();
    let stopped = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/stop"),
            json!({"generationId":progress.active_generation_id.unwrap()}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(stopped.status(), StatusCode::OK);
    let saved = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let chat = state.store.get(owner, chat_id).await.unwrap();
            if chat.messages[1].status != "pending" {
                break chat;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(saved.messages[1].status, "interrupted");
    assert_eq!(saved.messages[1].content, "");
    assert_eq!(saved.messages[1].reasoning_content, "Plan α then answer.");
    assert_eq!(saved.reasoning_effort, "high");
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"oversized-reasoning","model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let wire = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(wire.contains("event: error"));
    assert!(wire.contains("1 MiB storage limit"));
    let saved = state.store.get(owner, chat_id).await.unwrap();
    assert_eq!(saved.messages[3].status, "error");
    assert!(saved.messages[3]
        .error_message
        .as_ref()
        .is_some_and(|message| !message.is_empty()));
    assert_eq!(saved.messages[3].content, "");
    assert_eq!(
        saved.messages[3].reasoning_content.len(),
        obleth_chat::store::MAX_RESPONSE_BYTES
    );
    assert_eq!(state.metrics.streams.get(), 0);
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn background_completion_and_same_chat_regeneration_restore_selected_context(pool: PgPool) {
    let (app, state, fake, task) = fixture(pool.clone()).await;
    let (cookie, csrf, session) = login(&app, "test-admin").await;
    let owner = Uuid::parse_str(session["user"]["id"].as_str().unwrap()).unwrap();
    let id = conversation(&app, &cookie, &csrf).await;
    let chat = Uuid::parse_str(&id).unwrap();
    let path = format!("/api/conversations/{id}");
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{path}/messages"),
            json!({"content":"background","model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    // The acceptance event is available even before inference has answered.
    let mut stream = response.into_body().into_data_stream();
    let accepted = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&accepted).contains("event: meta"));
    drop(stream);
    let duplicate = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{path}/messages"),
            json!({"content":"duplicate","model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let c = state.store.get(owner, chat).await.unwrap();
            if c.messages[1].status == "completed" {
                break c;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert!(completed.active_generation_id.is_none());
    let original = completed.messages[1].id;
    let regen = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{path}/messages"),
            json!({"regenerateMessageId":original,"model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(regen.status(), StatusCode::OK);
    let _ = regen.into_body().collect().await.unwrap();
    let regenerated = state.store.get(owner, chat).await.unwrap();
    assert_eq!(regenerated.messages.len(), 2);
    assert_ne!(regenerated.messages[1].id, original);
    assert_eq!(regenerated.messages[1].versions.len(), 2);
    assert_eq!(state.store.list(owner).await.unwrap().len(), 1);
    let selected = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{path}/messages/{original}/select"),
            json!(null),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::OK);
    let send = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{path}/messages"),
            json!({"content":"follow-up","model":"fixture-model"}),
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    let _ = send.into_body().collect().await.unwrap();
    let body = fake.seen.lock().unwrap().last().unwrap()["body"].clone();
    // The system message tells the model what Chat draws; then the selected history.
    assert_eq!(body["messages"].as_array().unwrap().len(), 4);
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("```mermaid"));
    assert_eq!(body["messages"][1]["content"], "background");
    assert_eq!(body["messages"][2]["content"], "Hello α");
    assert_eq!(body["messages"][3]["content"], "follow-up");
    let final_chat = state.store.get(owner, chat).await.unwrap();
    assert_eq!(final_chat.messages.len(), 4);
    assert_eq!(final_chat.messages[1].id, original);
    assert_eq!(state.store.list(owner).await.unwrap().len(), 1);
    task.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn metrics_need_the_configured_token(pool: PgPool) {
    let (app, mut state, _, task) = fixture(pool).await;
    // Without a token, metrics are open as before.
    assert_eq!(fetch(&app, "/metrics", &[]).await.status(), StatusCode::OK);
    state.metrics.require_token("scrape-fixture-token");
    let app = router(state, "web".into());
    for headers in [
        vec![],
        vec![("authorization", "Bearer wrong-token")],
        vec![("authorization", "Basic scrape-fixture-token")],
        vec![("authorization", "Bearer scrape-fixture-token-and-more")],
    ] {
        let response = fetch(&app, "/metrics", &headers).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{headers:?}");
    }
    let response = fetch(
        &app,
        "/metrics",
        &[("authorization", "Bearer scrape-fixture-token")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("obleth_chat_http_requests_total"));
    task.abort();
}
