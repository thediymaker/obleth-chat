//! Connections end to end: a mock MCP server, mock slurmrestd and Kubernetes API
//! servers, a scripted gateway that calls their tools, and the approval flow.
//! Requires a disposable DATABASE_URL.
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    connections::Connections,
    gateway::Gateway,
    router,
    secrets::SecretKey,
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
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";
const TOKEN: &str = "fixture-mcp-token";
/// A person's own token for a shared MCP server set to personal tokens.
const PERSONAL_TOKEN: &str = "fixture-personal-mcp-token";
const SLURM_TOKEN: &str = "fixture-slurm-jwt";
const KUBE_TOKEN: &str = "fixture-kube-token";
/// A service account token of namespace "team".
const SA_TOKEN: &str = "fixture-service-account-token";

type Seen = Arc<Mutex<Vec<Value>>>;
/// The tool calls the scripted gateway makes in its first round.
type Script = Arc<Mutex<Value>>;

/// A Streamable HTTP MCP server with one read-only tool and one action.
async fn mcp(
    axum::extract::State(seen): axum::extract::State<Seen>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let personal = auth == format!("Bearer {PERSONAL_TOKEN}");
    if auth != format!("Bearer {TOKEN}") && !personal {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut record = message.clone();
    record["personal"] = json!(personal);
    seen.lock().unwrap().push(record);
    let id = message["id"].clone();
    match message["method"].as_str().unwrap_or("") {
        "initialize" => (
            [("mcp-session-id", "fixture-session")],
            Json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture-jobs"}}})),
        )
            .into_response(),
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => {
            assert_eq!(headers["mcp-session-id"], "fixture-session");
            Json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
                {"name":"lookup","description":"Look up a job","inputSchema":{"type":"object","properties":{"job":{"type":"string"}}},"annotations":{"readOnlyHint":true}},
                {"name":"submit_job","description":"Submit a job","inputSchema":{"type":"object","properties":{"script":{"type":"string"}}}}
            ]}}))
            .into_response()
        }
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap_or("").to_owned();
            let text = format!("{name} ran with {}", message["params"]["arguments"]);
            // Answer as an SSE stream with a notification first.
            let stream = async_stream::stream! {
                yield Ok::<_, Infallible>(Event::default().data(json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}).to_string()));
                yield Ok(Event::default().data(json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}}).to_string()));
            };
            Sse::new(stream).into_response()
        }
        _ => Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown method"}})).into_response(),
    }
}

/// slurmrestd (v0.0.45, and the older v0.0.42) and a Kubernetes API server for
/// account "alice", recording every request.
async fn cluster(
    axum::extract::State(seen): axum::extract::State<Seen>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = body.collect().await.unwrap().to_bytes();
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    let (method, path) = (parts.method.as_str(), parts.uri.path());
    seen.lock().unwrap().push(json!({
        "method": method, "path": path, "query": parts.uri.query().unwrap_or(""),
        "slurmUser": header("x-slurm-user-name"), "slurmToken": header("x-slurm-user-token"),
        "authorization": header("authorization"), "contentType": header("content-type"),
        "body": String::from_utf8_lossy(&body),
    }));
    let missing = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"kind":"Status","message":"not found"})),
        )
            .into_response()
    };
    if let Some(rest) = path.strip_prefix("/slurm/") {
        let Some(rest) = rest
            .strip_prefix("v0.0.45/")
            .or_else(|| rest.strip_prefix("v0.0.42/"))
        else {
            return missing();
        };
        // Like slurmrestd, ping answers without checking the token.
        if rest == "ping" {
            return Json(json!({"pings":[{"hostname":"slurm"}]})).into_response();
        }
        if header("x-slurm-user-token") != SLURM_TOKEN {
            return (
                StatusCode::from_u16(511).unwrap(),
                Json(json!({"errors":[]})),
            )
                .into_response();
        }
        return match (method, rest) {
            ("GET", "partitions") => Json(json!({"partitions":[{"name":"debug","partition":{"state":["UP"]},"cpus":{"total":2}}]})).into_response(),
            ("GET", "jobs") => Json(json!({"jobs":[
                {"job_id":6,"name":"mine","job_state":["COMPLETED"],"user_name":"alice","partition":"debug"},
                {"job_id":{"set":true,"infinite":false,"number":7},"name":"theirs","job_state":["RUNNING"],"user_name":"bob","partition":"debug"}
            ]}))
            .into_response(),
            ("GET", "nodes") => Json(json!({"nodes":[{"name":"n1","state":["MIXED"],"cpus":4,"alloc_cpus":1,"real_memory":8000,"alloc_memory":2000,"partitions":["debug"]}]})).into_response(),
            ("POST", "job/submit") => Json(json!({"job_id":8,"errors":[]})).into_response(),
            _ => missing(),
        };
    }
    if path == "/version" {
        return Json(json!({"gitVersion":"v1.31.0"})).into_response();
    }
    if !path.starts_with("/api") {
        return missing();
    }
    let who = match header("authorization").strip_prefix("Bearer ") {
        Some(KUBE_TOKEN) => "alice",
        Some(SA_TOKEN) => "system:serviceaccount:team:bot",
        _ => "",
    };
    if who.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"kind":"Status","message":"Unauthorized"})),
        )
            .into_response();
    }
    match (method, path) {
        ("POST", "/apis/authentication.k8s.io/v1/selfsubjectreviews") => {
            Json(json!({"status":{"userInfo":{"username":who}}})).into_response()
        }
        ("GET", "/api/v1") => Json(json!({"resources":[
            {"name":"pods","singularName":"pod","kind":"Pod","namespaced":true,"shortNames":["po"]},
            {"name":"pods/log","singularName":"","kind":"Pod","namespaced":true},
            {"name":"configmaps","singularName":"configmap","kind":"ConfigMap","namespaced":true,"shortNames":["cm"]}
        ]}))
        .into_response(),
        ("GET", "/api/v1/namespaces/team/pods") => Json(json!({"items":[
            {"metadata":{"name":"web-1","namespace":"team"},"status":{"phase":"Running"}}
        ]}))
        .into_response(),
        ("GET", "/api/v1/namespaces/team/resourcequotas") => Json(json!({"items":[
            {"status":{"hard":{"pods":"10","requests.cpu":"2"},"used":{"pods":"1","requests.cpu":"500m"}}}
        ]}))
        .into_response(),
        ("GET", "/apis/apps/v1/namespaces/team/deployments") | ("GET", "/apis/batch/v1/namespaces/team/jobs") => {
            Json(json!({"items":[]})).into_response()
        }
        ("GET", "/api/v1/nodes") => (
            StatusCode::FORBIDDEN,
            Json(json!({"kind":"Status","message":"nodes is forbidden"})),
        )
            .into_response(),
        ("POST", "/apis/authorization.k8s.io/v1/selfsubjectrulesreviews") => Json(json!({"status":{"resourceRules":[
            {"verbs":["create"],"resources":["selfsubjectreviews"]},
            {"verbs":["get","list"],"resources":["pods"]}
        ]}}))
        .into_response(),
        ("PATCH", "/api/v1/namespaces/team/configmaps/settings") => {
            Json(json!({"kind":"ConfigMap","metadata":{"name":"settings","namespace":"team"}})).into_response()
        }
        _ => missing(),
    }
}

/// Calls the scripted tools, then answers with what the tools returned.
async fn chat(
    axum::extract::State((seen, script)): axum::extract::State<(Seen, Script)>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    seen.lock().unwrap().push(body.clone());
    let messages = body["messages"].as_array().unwrap();
    let results: Vec<String> = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    let calls = script.lock().unwrap().clone();
    let thinks = messages
        .iter()
        .any(|m| m["role"] == "user" && m["content"].as_str().is_some_and(|c| c.contains("think")));
    let cut = messages.iter().any(|m| {
        m["role"] == "user"
            && m["content"]
                .as_str()
                .is_some_and(|c| c.contains("gets cut off"))
    });
    let warned = messages.iter().any(|m| {
        m["role"] == "user"
            && m["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("[Chat: your last step stopped"))
    });
    // A worker the lead sent answers its task.
    let worker = messages[0]["content"]
        .as_str()
        .is_some_and(|c| c.starts_with("You are a worker"))
        .then(|| messages[1]["content"].as_str().unwrap_or("").to_owned());
    let stream = async_stream::stream! {
        if let Some(task) = worker {
            yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"content":format!("Worker: {task}")}}]}).to_string()));
            yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":5,"total_tokens":55}}).to_string()));
            yield Ok(Event::default().data("[DONE]"));
            return;
        }
        if cut && !warned {
            // The output limit stops the call half written.
            let partial = json!([{"index":0,"id":"call_cut","type":"function","function":{"name":"fixture__lookup","arguments":"{\"job\":\"4"}}]);
            yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"tool_calls":partial}}]}).to_string()));
            yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"length"}]}).to_string()));
            yield Ok(Event::default().data("[DONE]"));
            return;
        }
        if cut {
            yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"content":"Smaller this time."}}]}).to_string()));
            yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string()));
            yield Ok(Event::default().data("[DONE]"));
            return;
        }
        if thinks {
            let thought = if results.is_empty() { "Look the job up first." } else { "Both ran; report θ." };
            yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"reasoning_content":thought}}]}).to_string()));
        }
        if results.is_empty() {
            yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"tool_calls":calls}}]}).to_string()));
            yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110}}).to_string()));
        } else {
            yield Ok(Event::default().data(json!({"choices":[{"delta":{"content":format!("Done: {}", results.join(" | "))}}]}).to_string()));
            yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":300,"completion_tokens":20,"total_tokens":320}}).to_string()));
        }
        yield Ok(Event::default().data("[DONE]"));
    };
    Sse::new(stream)
}

struct Fixture {
    app: Router,
    state: AppState,
    mcp_url: String,
    /// slurmrestd and the Kubernetes API.
    cluster_url: String,
    mcp_seen: Seen,
    cluster_seen: Seen,
    gateway_seen: Seen,
    script: Script,
    /// The gateway signing key, which connection tokens are sealed with.
    pem: String,
}

/// The gateway's tool calls as (id, tool, arguments).
fn tool_script(calls: &[(&str, &str, Value)]) -> Value {
    Value::Array(
        calls
            .iter()
            .enumerate()
            .map(|(i, (id, name, args))| json!({"index":i,"id":id,"function":{"name":name,"arguments":args.to_string()}}))
            .collect(),
    )
}

async fn fixture(pool: PgPool) -> Fixture {
    fixture_with(pool, true).await
}

/// `clusters`: whether Slurm and Kubernetes are offered (OBLETH_CHAT_CONNECTIONS).
async fn fixture_with(pool: PgPool, clusters: bool) -> Fixture {
    let mcp_seen: Seen = Arc::default();
    let cluster_seen: Seen = Arc::default();
    let gateway_seen: Seen = Arc::default();
    let script: Script = Arc::new(Mutex::new(tool_script(&[
        ("call_lookup", "fixture__lookup", json!({"job":"42"})),
        (
            "call_submit",
            "fixture__submit_job",
            json!({"script":"run.sh"}),
        ),
    ])));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mocks = Router::new()
        .route("/mcp", post(mcp).with_state(mcp_seen.clone()))
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"fixture-model"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(chat).with_state((gateway_seen.clone(), script.clone())),
        )
        .fallback(cluster)
        .with_state(cluster_seen.clone());
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
            ..AuthConfig::default()
        },
    )
    .await
    .unwrap();
    let mut state = AppState::new(auth, pool.clone(), Some(gateway), 4);
    state.followups_enabled = false;
    // The mock listens on loopback, so private addresses are allowed here.
    state.connections = Some(
        Connections::new(pool, pem.as_bytes(), true)
            .unwrap()
            .clusters(clusters),
    );
    Fixture {
        app: router(state.clone(), "web".into()),
        state,
        mcp_url: format!("http://{address}/mcp"),
        cluster_url: format!("http://{address}"),
        mcp_seen,
        cluster_seen,
        gateway_seen,
        script,
        pem: pem.to_string(),
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

async fn login(app: &Router, name: &str) -> (String, String) {
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
async fn connections_are_private_encrypted_and_shareable(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":format!("Bearer {TOKEN}")}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["hasToken"], true);
    assert_eq!(created["serverName"], "fixture-jobs");
    assert_eq!(created["tools"].as_array().unwrap().len(), 2);
    assert_eq!(created["tools"][0]["readOnly"], true);
    assert!(!created.to_string().contains(TOKEN));
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token FROM connections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&stored).contains(TOKEN));

    let (status, error) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Wrong","url":f.mcp_url,"token":"nope"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refused the token"));
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"fixture","url":f.mcp_url,"token":TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"second-user","password":PASSWORD,"displayName":"Second","role":"user"}),
        &admin,
    )
    .await;
    assert!(status.is_success());
    let user = login(&f.app, "second-user").await;
    let (_, listed) = call(&f.app, "GET", "/api/connections", Value::Null, &user).await;
    assert!(listed["connections"].as_array().unwrap().is_empty());
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Mine","url":f.mcp_url,"token":TOKEN,"shared":true}),
        &user,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Everyone","url":f.mcp_url,"token":TOKEN,"shared":true}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let id = shared["id"].as_str().unwrap();
    let (_, listed) = call(&f.app, "GET", "/api/connections", Value::Null, &user).await;
    let seen = listed["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap()
        .clone();
    assert_eq!(
        (
            seen["shared"].clone(),
            seen["canManage"].clone(),
            seen["active"].clone()
        ),
        (json!(true), json!(false), json!(true))
    );
    let (status, _) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{id}"),
        json!({"name":"Mine now"}),
        &user,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &f.app,
        "DELETE",
        &format!("/api/connections/{id}"),
        Value::Null,
        &user,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, updated) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{id}"),
        json!({"optedOut":true}),
        &user,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (updated["optedOut"].clone(), updated["active"].clone()),
        (json!(true), json!(false))
    );
    let audited: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action='connection.create'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audited, 1);
}

async fn start_turn(
    f: &Fixture,
    session: &(String, String),
) -> (String, tokio::task::JoinHandle<String>) {
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN}),
        session,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    send(f, session, "Check job 42 and resubmit it").await
}

/// Starts a reply in a new chat; the handle yields the streamed response.
async fn send(
    f: &Fixture,
    session: &(String, String),
    prompt: &str,
) -> (String, tokio::task::JoinHandle<String>) {
    let (status, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Jobs","model":"fixture-model"}),
        session,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":prompt,"model":"fixture-model"}),
            session,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = tokio::spawn(async move {
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    });
    (id, stream)
}

async fn pending(pool: &PgPool) -> String {
    for _ in 0..100 {
        let call: Option<String> =
            sqlx::query_scalar("SELECT call_id FROM tool_approvals WHERE decision IS NULL")
                .fetch_optional(pool)
                .await
                .unwrap();
        if let Some(call) = call {
            return call;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no tool call waited for approval");
}

fn tool_calls(seen: &Seen) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|m| m["method"] == "tools/call")
        .map(|m| m["params"]["name"].as_str().unwrap().to_owned())
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn read_only_tools_run_and_actions_wait_for_allow(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (id, stream) = start_turn(&f, &admin).await;
    assert_eq!(pending(&pool).await, "call_submit");
    // The read-only lookup ran without asking; the action has not run yet.
    assert_eq!(tool_calls(&f.mcp_seen), ["lookup"]);
    let activity: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activity[1]["status"], "awaiting");
    assert_eq!(activity[1]["summary"], "Fixture · submit_job");
    assert!(activity[1]["detail"].as_str().unwrap().contains("run.sh"));
    // The reply can tell a look from a change, and what kind of service it is.
    assert_eq!(activity[0]["readOnly"], true);
    assert_eq!(activity[1]["readOnly"], false);
    assert_eq!(activity[1]["service"], "mcp");
    assert_eq!(activity[1]["action"], "submit_job");

    let other = (admin.0.clone(), admin.1.clone());
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/approvals"),
        json!({"callId":"call_submit","decision":"allow"}),
        &other,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("\"status\":\"completed\""), "{text}");
    assert!(text.contains(r#"Done: lookup ran with {\"job\":\"42\"} | submit_job ran with {\"script\":\"run.sh\"}"#), "{text}");
    assert_eq!(tool_calls(&f.mcp_seen), ["lookup", "submit_job"]);
    // The model was offered both tools under the connection's prefix.
    let first = f.gateway_seen.lock().unwrap()[0].clone();
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["fixture__lookup", "fixture__submit_job"]);
    assert!(first["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Fixture (tools named fixture__…)"));
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/approvals"),
        json!({"callId":"call_submit","decision":"allow"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_approvals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn auto_mode_runs_actions_without_asking(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Jobs","model":"fixture-model"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/conversations/{id}/connections");
    let (status, _) = call(&f.app, "PATCH", &path, json!({"auto":true}), &admin).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, listed) = call(&f.app, "GET", &path, json!({}), &admin).await;
    assert_eq!(
        (listed["auto"].clone(), listed["autoAllowed"].clone()),
        (json!(true), json!(true))
    );

    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"Check job 42 and resubmit it","model":"fixture-model"}),
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = String::from_utf8(
        tokio::time::timeout(Duration::from_secs(20), response.into_body().collect())
            .await
            .unwrap()
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("\"status\":\"completed\""), "{text}");
    // The action ran without an approval card.
    assert_eq!(tool_calls(&f.mcp_seen), ["lookup", "submit_job"]);
    let asked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_approvals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(asked, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn auto_mode_nudges_a_turn_that_stopped_until_the_person_stops_it(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN}),
        &admin,
    )
    .await;
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Overnight","model":"fixture-model"}),
        &admin,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let uuid: uuid::Uuid = id.parse().unwrap();
    let path = format!("/api/conversations/{id}/connections");
    call(&f.app, "PATCH", &path, json!({"auto":true}), &admin).await;
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"Check job 42 and resubmit it","model":"fixture-model"}),
            &admin,
        ))
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();

    // The pod restarted mid-turn a few minutes ago.
    let stop = |reason: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE messages SET status='interrupted',error_message=$2 WHERE id=(SELECT selected_leaf_id FROM conversations WHERE id=$1)")
                .bind(uuid).bind(reason).execute(&pool).await.unwrap();
            sqlx::query("UPDATE conversations SET updated_at=NOW()-INTERVAL '5 minutes',auto_nudged_at=auto_nudged_at-INTERVAL '1 hour' WHERE id=$1")
                .bind(uuid).execute(&pool).await.unwrap();
        }
    };
    let finished = || async {
        for _ in 0..200 {
            let active: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT active_generation FROM conversations WHERE id=$1")
                    .bind(uuid)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if active.is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the nudged turn did not finish");
    };
    let messages = || async {
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT role,content,status FROM messages WHERE conversation_id=$1 ORDER BY ordinal",
        )
        .bind(uuid)
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    stop("Service shutting down").await;
    obleth_chat::autopilot::tick(&f.state).await.unwrap();
    finished().await;
    let after = messages().await;
    assert_eq!(after.len(), 4, "{after:?}");
    assert_eq!(after[2].0, "user");
    assert!(
        after[2].1.starts_with(
            "[Auto mode] Your last reply stopped before finishing (Service shutting down)"
        ),
        "{after:?}"
    );
    assert_eq!(after[3].2, "completed", "the nudged turn ran: {after:?}");
    let nudges: i32 = sqlx::query_scalar("SELECT auto_nudges FROM conversations WHERE id=$1")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(nudges, 1);

    // A reply the person stopped stays stopped.
    stop("Response stopped").await;
    obleth_chat::autopilot::tick(&f.state).await.unwrap();
    assert_eq!(messages().await.len(), 4);

    // Nor is a chat nudged once Auto mode is off.
    stop("Service shutting down").await;
    call(&f.app, "PATCH", &path, json!({"auto":false}), &admin).await;
    obleth_chat::autopilot::tick(&f.state).await.unwrap();
    assert_eq!(messages().await.len(), 4);
}

#[sqlx::test(migrations = "./migrations")]
async fn declined_actions_do_not_run(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (id, stream) = start_turn(&f, &admin).await;
    pending(&pool).await;
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/approvals"),
        json!({"callId":"call_submit","decision":"deny"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(
        text.contains("The person declined this tool call"),
        "{text}"
    );
    assert!(text.contains("\"status\":\"declined\""));
    assert_eq!(tool_calls(&f.mcp_seen), ["lookup"]);
    // Two requests: the totals add them up, the context meter reads the last one.
    let usage: Value = sqlx::query_scalar(
        "SELECT usage FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (&usage["prompt_tokens"], &usage["last_prompt_tokens"]),
        (&json!(400), &json!(300))
    );
    assert_eq!(
        (
            &usage["completion_tokens"],
            &usage["last_completion_tokens"],
            &usage["steps"]
        ),
        (&json!(30), &json!(20), &json!(2))
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn connections_can_skip_confirmation_or_be_switched_off(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN,"confirmActions":false}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Jobs","model":"fixture-model"}),
        &admin,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap();
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/messages"),
        json!({"content":"Go","model":"fixture-model"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tool_calls(&f.mcp_seen), ["lookup", "submit_job"]);

    let connection = created["id"].as_str().unwrap();
    let (status, _) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{connection}"),
        json!({"enabled":false}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let before = f.gateway_seen.lock().unwrap().len();
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Plain","model":"fixture-model"}),
        &admin,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap();
    call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/messages"),
        json!({"content":"Go","model":"fixture-model"}),
        &admin,
    )
    .await;
    let request = f.gateway_seen.lock().unwrap()[before].clone();
    assert!(request.get("tools").is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn allow_for_this_chat_skips_later_questions_for_that_tool_only(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (id, stream) = start_turn(&f, &admin).await;
    pending(&pool).await;
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/approvals"),
        json!({"callId":"call_submit","decision":"allow","remember":true}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    // The next turn in the same chat calls both tools again without asking.
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/messages"),
        json!({"content":"Again","model":"fixture-model"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tool_calls(&f.mcp_seen),
        ["lookup", "submit_job", "lookup", "submit_job"]
    );
    // A different chat still asks.
    let (status, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Other","model":"fixture-model"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let other = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{other}/messages"),
            json!({"content":"Go","model":"fixture-model"}),
            &admin,
        ))
        .await
        .unwrap();
    let stream =
        tokio::spawn(async move { response.into_body().collect().await.unwrap().to_bytes() });
    assert_eq!(pending(&pool).await, "call_submit");
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{other}/approvals"),
        json!({"callId":"call_submit","decision":"deny"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_connection_can_be_switched_off_for_one_chat(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN,"confirmActions":false}),
        &admin,
    )
    .await;
    let connection = created["id"].as_str().unwrap();
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Quiet","model":"fixture-model"}),
        &admin,
    )
    .await;
    let quiet = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let (status, listed) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{quiet}/connections"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["connections"][0]["on"], true);
    assert_eq!(listed["connections"][0]["tools"], 2);
    let (status, _) = call(
        &f.app,
        "PATCH",
        &format!("/api/conversations/{quiet}/connections"),
        json!({"connectionId":connection,"on":false}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, listed) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{quiet}/connections"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(listed["connections"][0]["on"], false);
    call(
        &f.app,
        "POST",
        &format!("/api/conversations/{quiet}/messages"),
        json!({"content":"Go","model":"fixture-model"}),
        &admin,
    )
    .await;
    assert!(f.gateway_seen.lock().unwrap()[0].get("tools").is_none());
    // Other chats keep it.
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Busy","model":"fixture-model"}),
        &admin,
    )
    .await;
    let busy = chat["conversation"]["id"].as_str().unwrap().to_owned();
    call(
        &f.app,
        "POST",
        &format!("/api/conversations/{busy}/messages"),
        json!({"content":"Go","model":"fixture-model"}),
        &admin,
    )
    .await;
    assert!(f.gateway_seen.lock().unwrap()[1]["tools"].is_array());
    // Someone else's chat cannot be read or changed.
    call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"second-user","password":PASSWORD,"displayName":"Second","role":"user"}),
        &admin,
    )
    .await;
    let user = login(&f.app, "second-user").await;
    let (status, _) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{quiet}/connections"),
        Value::Null,
        &user,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn cluster_requests(f: &Fixture, method: &str, path: &str) -> Vec<Value> {
    f.cluster_seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r["method"] == method && r["path"] == path)
        .cloned()
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_shared_mcp_server_can_take_each_persons_own_token(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    // The administrator's token only lists the tools; calls use each person's own.
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN,"shared":true,"options":{"personalTokens":true}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(
        (
            shared["personalTokens"].clone(),
            shared["needsCredential"].clone(),
            shared["active"].clone()
        ),
        (json!(true), json!(true), json!(false))
    );
    assert_eq!(shared["tools"].as_array().unwrap().len(), 2);
    let path = format!(
        "/api/connections/{}/credential",
        shared["id"].as_str().unwrap()
    );
    let (status, _) = call(&f.app, "PUT", &path, json!({"token":"wrong"}), &admin).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, ready) = call(
        &f.app,
        "PUT",
        &path,
        json!({"token":PERSONAL_TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(
        (ready["active"].clone(), ready["credential"].is_object()),
        (json!(true), true)
    );

    let (_, stream) = send(&f, &admin, "Check job 42 and resubmit it").await;
    let waiting = pending(&pool).await;
    let calls: Vec<bool> = f
        .mcp_seen
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m["method"] == "tools/call")
        .map(|m| m["personal"] == true)
        .collect();
    assert_eq!(calls, [true], "the lookup ran with the person's own token");
    let conversation: String =
        sqlx::query_scalar("SELECT conversation_id::text FROM tool_approvals WHERE call_id=$1")
            .bind(&waiting)
            .fetch_one(&pool)
            .await
            .unwrap();
    call(
        &f.app,
        "POST",
        &format!("/api/conversations/{conversation}/approvals"),
        json!({"callId":waiting,"decision":"deny"}),
        &admin,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();

    // Removing the token takes the server out of this person's chats again.
    let (status, removed) = call(&f.app, "DELETE", &path, json!({}), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed["active"], false);
}

#[sqlx::test(migrations = "./migrations")]
async fn clusters_can_be_turned_off_for_the_installation(pool: PgPool) {
    let f = fixture_with(pool.clone(), false).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, capabilities) = call(&f.app, "GET", "/api/capabilities", json!({}), &admin).await;
    assert_eq!(
        (
            capabilities["features"]["connections"].clone(),
            capabilities["features"]["clusters"].clone()
        ),
        (json!(true), json!(false))
    );
    let (status, refused) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    // A cluster saved while they were on is hidden, and MCP servers still work.
    sqlx::query("INSERT INTO connections(id,owner_id,name,url,kind,options,tools) VALUES($1,NULL,'Old','http://127.0.0.1:1','slurm','{}','[]')")
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (_, listed) = call(&f.app, "GET", "/api/connections", json!({}), &admin).await;
    let kinds: Vec<&str> = listed["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["mcp"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_shared_slurm_cluster_runs_jobs_with_each_persons_own_token(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    // Shared clusters take no token of their own; each person adds theirs.
    let (status, rejected) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true,"token":SLURM_TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(
        (
            shared["kind"].clone(),
            shared["needsCredential"].clone(),
            shared["active"].clone()
        ),
        (json!("slurm"), json!(true), json!(false))
    );
    assert_eq!(shared["tools"].as_array().unwrap().len(), 5);
    let id = shared["id"].as_str().unwrap().to_owned();
    let path = format!("/api/connections/{id}/credential");
    let (status, refused) = call(
        &f.app,
        "PUT",
        &path,
        json!({"token":"wrong","user":"alice"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or(&refused.to_string())
            .contains("refused the token for alice"),
        "{refused}"
    );
    let (status, ready) = call(
        &f.app,
        "PUT",
        &path,
        json!({"token":SLURM_TOKEN,"user":"alice"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready["credential"]["identity"], "alice");
    assert_eq!(ready["active"], true);
    let sealed: Vec<u8> = sqlx::query_scalar("SELECT token FROM connection_credentials")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!sealed
        .windows(SLURM_TOKEN.len())
        .any(|w| w == SLURM_TOKEN.as_bytes()));

    // Another person sees the cluster but cannot use it until they add a token.
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"second-user","password":PASSWORD,"displayName":"Second","role":"user"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let user = login(&f.app, "second-user").await;
    let (_, theirs) = call(&f.app, "GET", "/api/connections", Value::Null, &user).await;
    let theirs = &theirs["connections"][0];
    assert_eq!(
        (
            theirs["needsCredential"].clone(),
            theirs["credential"].clone()
        ),
        (json!(true), Value::Null)
    );

    *f.script.lock().unwrap() = tool_script(&[
        ("call_jobs", "slurm__jobs", json!({})),
        (
            "call_submit",
            "slurm__submit",
            json!({"script":"echo hi","name":"hello"}),
        ),
    ]);
    let (_, stream) = send(&f, &admin, "Show my jobs and submit hello").await;
    assert_eq!(pending(&pool).await, "call_submit");
    assert!(cluster_requests(&f, "POST", "/slurm/v0.0.45/job/submit").is_empty());
    let (_, chats) = call(&f.app, "GET", "/api/conversations", Value::Null, &admin).await;
    let chat = chats["conversations"][0]["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{chat}/approvals"),
        json!({"callId":"call_submit","decision":"allow"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    // Only the person's own jobs, then the submitted one.
    assert!(text.contains("6 mine [COMPLETED]"), "{text}");
    assert!(!text.contains("theirs"), "{text}");
    assert!(text.contains("Submitted job 8 as alice"), "{text}");
    let submitted = cluster_requests(&f, "POST", "/slurm/v0.0.45/job/submit");
    assert_eq!(submitted.len(), 1);
    assert_eq!(
        (
            submitted[0]["slurmUser"].as_str(),
            submitted[0]["slurmToken"].as_str()
        ),
        (Some("alice"), Some(SLURM_TOKEN))
    );
    let body: Value = serde_json::from_str(submitted[0]["body"].as_str().unwrap()).unwrap();
    assert!(
        body["script"].as_str().unwrap().ends_with("echo hi"),
        "{body}"
    );
    assert_eq!(body["job"]["name"], "hello");
    assert_eq!(body["job"]["current_working_directory"], "/home/alice");
    // The model was told whom it acts as.
    let first = f.gateway_seen.lock().unwrap()[0].clone();
    assert!(
        first["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("as user alice"),
        "{}",
        first["messages"][0]["content"]
    );

    // Removing the token takes the cluster out of the person's chats again.
    let (status, removed) = call(&f.app, "DELETE", &path, Value::Null, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed["needsCredential"], true);
    let (_, listed) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{chat}/connections"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(listed["connections"], json!([]));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_kubernetes_connection_lists_and_applies_with_the_persons_token(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let create = |token: &str| json!({"kind":"kubernetes","name":"Kube","url":f.cluster_url,"token":token,"options":{"namespace":"team"},"confirmActions":false});
    let (status, refused) = call(&f.app, "POST", "/api/connections", create("nope"), &admin).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        create(KUBE_TOKEN),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(
        (
            created["identity"].clone(),
            created["namespace"].clone(),
            created["active"].clone()
        ),
        (json!("alice"), json!("team"), json!(true))
    );
    // Renaming keeps the token and settings without checking again.
    let id = created["id"].as_str().unwrap();
    let checks = cluster_requests(
        &f,
        "POST",
        "/apis/authentication.k8s.io/v1/selfsubjectreviews",
    )
    .len();
    let (status, renamed) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{id}"),
        json!({"name":"Kube"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["identity"], "alice");
    assert_eq!(
        cluster_requests(
            &f,
            "POST",
            "/apis/authentication.k8s.io/v1/selfsubjectreviews"
        )
        .len(),
        checks
    );
    // A service account token with no namespace given works in its own namespace.
    let (status, robot) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{id}"),
        json!({"token":SA_TOKEN,"options":{"namespace":""}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{robot}");
    assert_eq!(
        (robot["identity"].as_str(), robot["namespace"].as_str()),
        (Some("system:serviceaccount:team:bot"), Some("team"))
    );
    let (status, back) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{id}"),
        json!({"token":KUBE_TOKEN,"options":{"namespace":"team"}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{back}");

    *f.script.lock().unwrap() = tool_script(&[
        ("call_list", "kube__list", json!({"kind":"pods"})),
        (
            "call_apply",
            "kube__apply",
            json!({"manifest":"apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\ndata:\n  mode: fast\n"}),
        ),
        (
            "call_missing",
            "kube__list",
            json!({"kind":"widgets","api_version":"v1"}),
        ),
    ]);
    let (_, stream) = send(&f, &admin, "List pods and apply settings").await;
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("web-1"), "{text}");
    assert!(
        text.contains("This cluster has no resource kind widgets in v1"),
        "{text}"
    );
    let applied = cluster_requests(&f, "PATCH", "/api/v1/namespaces/team/configmaps/settings");
    assert_eq!(applied.len(), 1, "{text}");
    assert_eq!(applied[0]["contentType"], "application/apply-patch+yaml");
    assert_eq!(applied[0]["authorization"], format!("Bearer {KUBE_TOKEN}"));
    assert!(applied[0]["query"]
        .as_str()
        .unwrap()
        .contains("fieldManager=obleth-chat"));
    assert!(applied[0]["body"].as_str().unwrap().contains("mode: fast"));
    // Without confirmation nothing waited for the person.
    let asked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_approvals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(asked, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_slurm_cluster_uses_the_api_version_set_on_it(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let create = |version: &str| json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true,"options":{"apiVersion":version}});
    let (status, refused) =
        call(&f.app, "POST", "/api/connections", create("0.0.45"), &admin).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    // A version the server does not offer is caught when the cluster is saved.
    let (status, missing) = call(
        &f.app,
        "POST",
        "/api/connections",
        create("v0.0.46"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{missing}");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("does not offer the Slurm REST API v0.0.46"),
        "{missing}"
    );

    // Set to the older version the server still offers: Chat uses it, not the newest.
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        create("v0.0.42"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(shared["apiVersion"], "v0.0.42");
    let id = shared["id"].as_str().unwrap().to_owned();
    let (status, mine) = call(
        &f.app,
        "PUT",
        &format!("/api/connections/{id}/credential"),
        json!({"token":SLURM_TOKEN,"user":"alice"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let slurm_paths = || -> Vec<String> {
        f.cluster_seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| {
                r["path"]
                    .as_str()
                    .filter(|p| p.starts_with("/slurm/"))
                    .map(str::to_owned)
            })
            .collect()
    };
    assert!(
        !slurm_paths()
            .iter()
            .any(|p| p.starts_with("/slurm/v0.0.45/")),
        "{:?}",
        slurm_paths()
    );
    assert!(!cluster_requests(&f, "GET", "/slurm/v0.0.42/partitions").is_empty());

    // Cleared, Chat finds the newest version again.
    let (status, cleared) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{id}"),
        json!({"options":{"apiVersion":""}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["apiVersion"], Value::Null);
    let (status, _) = call(
        &f.app,
        "GET",
        &format!("/api/connections/{id}/status"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!cluster_requests(&f, "GET", "/slurm/v0.0.45/partitions").is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn cluster_status_shows_what_the_persons_account_sees(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, slurm) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true}),
        &admin,
    )
    .await;
    let slurm = slurm["id"].as_str().unwrap().to_owned();
    let (_, status) = call(
        &f.app,
        "GET",
        &format!("/api/connections/{slurm}/status"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, json!({"state":"needsCredential"}));
    call(
        &f.app,
        "PUT",
        &format!("/api/connections/{slurm}/credential"),
        json!({"token":SLURM_TOKEN,"user":"alice"}),
        &admin,
    )
    .await;
    let (code, status) = call(
        &f.app,
        "GET",
        &format!("/api/connections/{slurm}/status"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(status["state"], "connected", "{status}");
    let s = &status["status"];
    assert_eq!(s["totals"]["cpus"], 4);
    assert_eq!(s["totals"]["allocCpus"], 1);
    assert_eq!(
        (s["queue"]["running"].clone(), s["queue"]["total"].clone()),
        (json!(1), json!(2))
    );
    // Only the person's own jobs are listed.
    assert_eq!(s["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(s["jobs"][0]["id"], 6);

    let (_, kube) = call(&f.app, "POST", "/api/connections", json!({"kind":"kubernetes","name":"Kube","url":f.cluster_url,"token":KUBE_TOKEN,"options":{"namespace":"team"}}), &admin).await;
    let kube = kube["id"].as_str().unwrap().to_owned();
    let (_, status) = call(
        &f.app,
        "GET",
        &format!("/api/connections/{kube}/status"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status["state"], "connected", "{status}");
    let k = &status["status"];
    assert_eq!(
        (k["identity"].as_str(), k["namespace"].as_str()),
        (Some("alice"), Some("team"))
    );
    assert_eq!(k["pods"][0]["name"], "web-1");
    let cpu = k["quota"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["resource"] == "requests.cpu")
        .unwrap();
    assert_eq!(cpu["ratio"], 0.25);
    assert_eq!(k["nodes"]["error"], "Your account may not read this.");
    assert_eq!(
        k["allowed"],
        json!([{"resources":"pods","verbs":"get, list"}])
    );

    // A token that stops working shows as an error, not a failed request.
    sqlx::query("UPDATE connections SET token=NULL WHERE id=$1")
        .bind(Uuid::parse_str(&kube).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let (code, status) = call(
        &f.app,
        "GET",
        &format!("/api/connections/{kube}/status"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_ne!(status["state"], "connected", "{status}");
}

#[sqlx::test(migrations = "./migrations")]
async fn thoughts_are_placed_between_steps_and_notes_steer_the_next_step(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (id, stream) = send(&f, &admin, "think, then check job 42 and resubmit it").await;
    assert_eq!(pending(&pool).await, "call_submit");
    // A note sent while the reply waits is read at its next step.
    let (status, sent) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/steer"),
        json!({"text":"use the gpu partition"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/approvals"),
        json!({"callId":"call_submit","decision":"allow"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains(r#""kind":"thought""#), "{text}");
    let second = f.gateway_seen.lock().unwrap()[1].clone();
    let notes: Vec<&Value> = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .collect();
    assert!(
        notes.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("[A note from the person while you work: use the gpu partition]"),
        "{second}"
    );
    let (activity, reasoning): (Value, String) = sqlx::query_as("SELECT activity, reasoning_content FROM messages WHERE conversation_id=$1 AND role='assistant'")
        .bind(Uuid::parse_str(&id).unwrap()).fetch_one(&pool).await.unwrap();
    let kinds: Vec<&str> = activity
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().or(e["tool"].as_str()).unwrap_or(""))
        .collect();
    assert_eq!(
        kinds,
        [
            "thought",
            "fixture__lookup",
            "fixture__submit_job",
            "note",
            "thought"
        ],
        "{activity}"
    );
    // Offsets count UTF-16 units, as the browser slices the reasoning.
    let units: Vec<u16> = reasoning.encode_utf16().collect();
    let slice = |e: &Value| {
        String::from_utf16(
            &units[e["from"].as_u64().unwrap() as usize..e["to"].as_u64().unwrap() as usize],
        )
        .unwrap()
    };
    assert_eq!(slice(&activity[0]), "Look the job up first.");
    assert_eq!(slice(&activity[4]), "Both ran; report θ.");
    assert_eq!(activity[3]["summary"], "use the gpu partition");
    // Nothing is running any more, so a note is refused.
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/steer"),
        json!({"text":"too late"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_cut_off_at_the_output_limit_never_runs(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (id, stream) = send(&f, &admin, "Look up job 42, but this gets cut off").await;
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("Smaller this time."), "{text}");
    // The half-written lookup never reached the server; the model was told why.
    assert!(tool_calls(&f.mcp_seen).is_empty());
    let activity: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(activity
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["tool"] == "output_limit" && e["status"] == "error"));
    let second = f.gateway_seen.lock().unwrap()[1].clone();
    let note = second["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(note.contains("fixture__lookup, so nothing ran"), "{note}");
}

#[sqlx::test(migrations = "./migrations")]
async fn any_chat_can_send_workers_without_a_workspace(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    sqlx::query("INSERT INTO model_profiles (model_id, name, worker) VALUES ('fixture-model','Fixture',TRUE)")
        .execute(&pool)
        .await
        .unwrap();
    *f.script.lock().unwrap() = tool_script(&[(
        "call_agents",
        "delegate",
        json!({"tasks":[{"title":"Queues","task":"Name a queue design."},{"title":"Stacks","task":"Name a stack design."}]}),
    )]);
    let (_, stream) = send(&f, &admin, "Use some sub-agents to compare designs").await;
    let body = stream.await.unwrap();
    assert!(
        body.contains("Worker: Name a queue design.")
            && body.contains("Worker: Name a stack design."),
        "{body}"
    );
    let seen = f.gateway_seen.lock().unwrap().clone();
    let lead = &seen[0];
    // The lead hears that agents means delegate, never cluster jobs.
    assert!(lead["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Never start cluster jobs"));
    let delegate = lead["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["function"]["name"] == "delegate")
        .expect("delegate is offered without a workspace");
    // Chat has no workspaces here, so workers answer from what they know.
    assert!(delegate["function"]["parameters"]["properties"]["repo"].is_null());
    let workers: Vec<&Value> = seen
        .iter()
        .filter(|r| {
            r["messages"][0]["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("You are a worker"))
        })
        .collect();
    assert_eq!(workers.len(), 2);
    assert!(workers.iter().all(|w| w.get("tools").is_none()));
    // No connection or cluster was asked for anything: only the gateway's model lookups.
    assert!(f
        .cluster_seen
        .lock()
        .unwrap()
        .iter()
        .all(|r| r["path"] == "/model/info"));
    assert!(tool_calls(&f.mcp_seen).is_empty());
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM generation_usage WHERE purpose='agent' AND status='completed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_secret_key_takes_over_tokens_sealed_with_the_signing_key(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Fixture","url":f.mcp_url,"token":TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Everyone","url":f.mcp_url,"token":TOKEN,"shared":true,"options":{"personalTokens":true}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let path = format!(
        "/api/connections/{}/credential",
        shared["id"].as_str().unwrap()
    );
    let (status, _) = call(
        &f.app,
        "PUT",
        &path,
        json!({"token":PERSONAL_TOKEN}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username='test-admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let formats = || async {
        sqlx::query_scalar::<_, i32>(
            "SELECT get_byte(token, 0) FROM connections UNION ALL SELECT get_byte(token, 0) FROM connection_credentials",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    assert_eq!(formats().await, [1, 1, 1]);

    let key = SecretKey::from_bytes([5u8; 32]);
    let secret_only = Connections::with_keys(pool.clone(), None, Some(key.clone()), true).unwrap();
    assert!(
        secret_only
            .toolset(admin_id, Uuid::new_v4())
            .await
            .is_empty(),
        "tokens sealed with the signing key need it until they move"
    );
    // Two replicas starting at once move each token exactly once.
    let both =
        Connections::with_keys(pool.clone(), Some(f.pem.as_bytes()), Some(key), true).unwrap();
    let replica = both.clone();
    let (first, second) = tokio::join!(both.reseal(), replica.reseal());
    assert_eq!(first.unwrap() + second.unwrap(), 3);
    assert_eq!(formats().await, [2, 2, 2]);
    assert_eq!(
        both.reseal().await.unwrap(),
        0,
        "moving again changes nothing"
    );
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token FROM connection_credentials")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&stored[0]).contains(PERSONAL_TOKEN));

    // Without the signing key, the secret key alone now reads every token.
    assert_eq!(
        secret_only
            .toolset(admin_id, Uuid::new_v4())
            .await
            .definitions()
            .len(),
        4
    );
    let mut state = f.state.clone();
    state.connections = Some(secret_only);
    let app = router(state, "web".into());
    let (_, listed) = call(&app, "GET", "/api/connections", Value::Null, &admin).await;
    let own = listed["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Fixture")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, refreshed) = call(
        &app,
        "POST",
        &format!("/api/connections/{own}/refresh"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{refreshed}");
}
