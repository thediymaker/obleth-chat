//! The browser end to end, without a real Chromium: which tools a chat is offered
//! with the browser off and on, the cards that end a reply, take over and hand back,
//! "Always on this site" approvals and the settings. Requires a disposable
//! DATABASE_URL. Driving a real browser is covered by the ignored live test in
//! api/src/browser/tools.rs.
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
    browser::{BrowserConfig, Browsers},
    gateway::Gateway,
    router,
    sandbox::SandboxConfig,
    AppState,
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

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";

type Seen = Arc<Mutex<Vec<Value>>>;
/// The tool call the scripted gateway makes when a request has no tool results.
type Script = Arc<Mutex<Option<(String, Value)>>>;

async fn chat(
    axum::extract::State((seen, script)): axum::extract::State<(Seen, Script)>,
    Json(body): Json<Value>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    seen.lock().unwrap().push(body.clone());
    let answered = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool");
    let call = script.lock().unwrap().clone().filter(|_| !answered);
    let stream = async_stream::stream! {
        match call {
            Some((name, args)) => {
                yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":name,"arguments":args.to_string()}}]}}]}).to_string()));
                yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}).to_string()));
            }
            None => {
                yield Ok(Event::default().data(json!({"choices":[{"delta":{"content":"Done."}}]}).to_string()));
                yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string()));
            }
        }
        yield Ok(Event::default().data("[DONE]"));
    };
    Sse::new(stream)
}

struct Fixture {
    app: Router,
    seen: Seen,
    script: Script,
}

async fn fixture(pool: PgPool) -> Fixture {
    let seen: Seen = Arc::default();
    let script: Script = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mocks = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"fixture-model"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(chat).with_state((seen.clone(), script.clone())),
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
    // No cluster: the Kubernetes client points nowhere and the browser address is
    // fixed, so nothing here starts a pod.
    let ca = std::env::temp_dir().join("obleth-browser-test-ca.pem");
    std::fs::write(&ca, "").unwrap();
    std::env::set_var("OBLETH_CHAT_KUBE_API", "https://127.0.0.1:9");
    std::env::set_var("OBLETH_CHAT_KUBE_CA_FILE", &ca);
    std::env::set_var("OBLETH_CHAT_BROWSER_ADDRESS", "127.0.0.1:9");
    let sandbox = SandboxConfig {
        namespace: "nowhere".into(),
        image: "example.invalid/none".into(),
        runtime_class: None,
        storage_class: None,
        storage: "1Gi".into(),
        disk_limit: 1 << 30,
        cpu_limit: "1".into(),
        memory_limit: "1Gi".into(),
        everyone: false,
        preview_domain: Some("sbx.example.com".into()),
        preview_scheme: "http".into(),
        chat_origin: ORIGIN.into(),
        idle: Duration::from_secs(3600),
        retention: Duration::from_secs(86400),
    };
    let config = BrowserConfig {
        image: "example.invalid/none".into(),
        cpu_limit: "1".into(),
        memory_limit: "1Gi".into(),
        storage: "1Gi".into(),
        idle: Duration::from_secs(900),
    };
    let mut state = AppState::new(auth, pool.clone(), Some(gateway), 4);
    state.followups_enabled = false;
    state.browsers = Some(Browsers::new(config, sandbox, pool).unwrap());
    Fixture {
        app: router(state, "web".into()),
        seen,
        script,
    }
}

fn req(method: &str, path: &str, body: Value, session: &(String, String)) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("origin", ORIGIN)
        .header("cookie", &session.0)
        .header("x-csrf-token", &session.1)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    session: &(String, String),
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(req(method, path, body, session))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn login(app: &Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/auth/login",
            json!({"username":"test-admin","password":PASSWORD}),
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

/// Sends a message and waits for the reply; returns the reply's activity.
async fn reply(f: &Fixture, id: &str, prompt: &str, session: &(String, String)) -> Vec<Value> {
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":prompt,"model":"auto"}),
            session,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap();
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{id}"),
        Value::Null,
        session,
    )
    .await;
    let messages = detail["conversation"]["messages"]
        .as_array()
        .or(detail["messages"].as_array())
        .cloned()
        .unwrap_or_default();
    messages.last().unwrap()["activity"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn offered(seen: &Seen) -> Vec<String> {
    seen.lock().unwrap().last().unwrap()["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[sqlx::test(migrations = "./migrations")]
async fn the_browser_is_offered_handed_over_and_remembered(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app).await;
    let (_, capabilities) = call(&f.app, "GET", "/api/capabilities", Value::Null, &admin).await;
    assert_eq!(capabilities["features"]["browser"], true, "{capabilities}");
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Look it up","model":"auto"}),
        &admin,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();

    // Off: the model can only ask for the browser, and that ends the reply.
    *f.script.lock().unwrap() = Some((
        "request_browser".into(),
        json!({"reason":"To check the release notes."}),
    ));
    let activity = reply(&f, &id, "What changed in the latest release?", &admin).await;
    let tools = offered(&f.seen);
    assert!(tools.contains(&"request_browser".to_owned()), "{tools:?}");
    assert!(!tools.contains(&"browser_search".to_owned()));
    let card = activity
        .iter()
        .find(|e| e["kind"] == "browser-request")
        .unwrap();
    assert_eq!(card["reason"], "To check the release notes.");
    assert_eq!(
        f.seen.lock().unwrap().len(),
        1,
        "the reply ends at the card"
    );

    // On: the browser tools and their instructions, and no request.
    let path = format!("/api/conversations/{id}/browser");
    let (status, state) = call(&f.app, "PUT", &path, json!({"on":true}), &admin).await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["on"], true);
    *f.script.lock().unwrap() = Some((
        "browser_handoff".into(),
        json!({"reason":"Sign in to the journal, then hand back."}),
    ));
    let activity = reply(&f, &id, "Get the full text.", &admin).await;
    let tools = offered(&f.seen);
    for name in [
        "browser_search",
        "browser_open",
        "browser_click",
        "browser_type",
        "browser_read",
        "browser_handoff",
    ] {
        assert!(tools.contains(&name.to_owned()), "{name} in {tools:?}");
    }
    assert!(!tools.contains(&"request_browser".to_owned()));
    let system = f.seen.lock().unwrap().last().unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(system.contains("# Your browser") && system.contains("DuckDuckGo"));

    // The handoff gives the person the tab; the model's tools then refuse to run.
    assert!(activity.iter().any(|e| e["kind"] == "browser-handoff"));
    let (_, state) = call(&f.app, "GET", &path, Value::Null, &admin).await;
    assert_eq!(state["controller"], "user");
    *f.script.lock().unwrap() = Some((
        "browser_search".into(),
        json!({"query":"carnot efficiency"}),
    ));
    let activity = reply(&f, &id, "Search meanwhile.", &admin).await;
    let step = activity
        .iter()
        .find(|e| e["tool"] == "browser_search")
        .unwrap();
    assert_eq!(step["status"], "error");
    assert!(step["detail"].as_str().unwrap().contains("has control"));
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("{path}/control"),
        json!({"controller":"model"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, state) = call(&f.app, "GET", &path, Value::Null, &admin).await;
    assert_eq!(state["controller"], "model");

    // "Always on this site" on a browser approval remembers the site.
    let generation = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO tool_approvals(generation_id,call_id,conversation_id,owner_id,tool_name) SELECT $1,'call_9',$2::uuid,owner_id,'browser-site:shop.example' FROM conversations WHERE id=$2::uuid")
        .bind(generation)
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/approvals"),
        json!({"callId":"call_9","decision":"allow","remember":true}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, account) = call(&f.app, "GET", "/api/browser", Value::Null, &admin).await;
    assert_eq!(account["sites"][0]["host"], "shop.example", "{account}");
    assert_eq!(account["tabs"][0]["title"], "Look it up");
    let (status, _) = call(
        &f.app,
        "DELETE",
        "/api/browser/sites/shop.example",
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The search engine is a setting with three choices.
    let (status, _) = call(
        &f.app,
        "PUT",
        "/api/browser",
        json!({"searchEngine":"bing"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &f.app,
        "PUT",
        "/api/browser",
        json!({"searchEngine":"altavista"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, account) = call(&f.app, "GET", "/api/browser", Value::Null, &admin).await;
    assert_eq!(account["searchEngine"], "bing");
    assert!(account["sites"].as_array().unwrap().is_empty());

    // Off again: the tools go away.
    call(&f.app, "PUT", &path, json!({"on":false}), &admin).await;
    *f.script.lock().unwrap() = None;
    reply(&f, &id, "Thanks.", &admin).await;
    assert!(!offered(&f.seen).contains(&"browser_search".to_owned()));
}
