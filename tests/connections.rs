//! Connections end to end: a mock MCP server, mock slurmrestd and Kubernetes API
//! servers, a scripted gateway that calls their tools, and the approval flow.
//! Requires a disposable DATABASE_URL.
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
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
    knowledge::ObjectStore,
    router,
    secrets::SecretKey,
    AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{
    collections::HashMap,
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
/// A token whose every answer is larger than Chat reads.
const HUGE_SLURM_TOKEN: &str = "fixture-huge-answers";
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
/// account "alice", recording every request. Like a large cluster's, slurmrestd's list
/// of every job is too big to read: Chat must ask accounting for the person's jobs.
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
    let slurm = path
        .strip_prefix("/slurm/")
        .map(|rest| ("slurm", rest))
        .or_else(|| path.strip_prefix("/slurmdb/").map(|rest| ("slurmdb", rest)));
    if let Some((api, rest)) = slurm {
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
        if header("x-slurm-user-token") == HUGE_SLURM_TOKEN {
            let pad = "x".repeat(33 * 1024 * 1024);
            return Json(json!({"partitions":[],"pad":pad})).into_response();
        }
        if header("x-slurm-user-token") != SLURM_TOKEN {
            return (
                StatusCode::from_u16(511).unwrap(),
                Json(json!({"errors":[]})),
            )
                .into_response();
        }
        let query: Vec<(String, String)> =
            url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        let param = |name: &str| {
            query
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        return match (method, api, rest) {
            ("GET", "slurm", "jobs") => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"errors":[{"description":"the whole cluster's job list is too large to read"}]})),
            )
                .into_response(),
            ("GET", "slurm", "diag") => Json(json!({"statistics":{"jobs_running":{"set":true,"infinite":false,"number":2},"jobs_pending":3}})).into_response(),
            ("GET", "slurmdb", "jobs") => {
                let jobs = [
                    json!({"job_id":6,"name":"mine","user":"alice","partition":"debug","nodes":"n1","state":{"current":["RUNNING"],"reason":"None"},"time":{"elapsed":75,"submission":100,"limit":{"set":true,"infinite":false,"number":30}}}),
                    json!({"job_id":9,"name":"queued","user":"alice","partition":"debug","nodes":"None assigned","state":{"current":["PENDING"],"reason":"Priority"},"time":{"elapsed":0,"submission":200,"limit":{"set":true,"infinite":false,"number":60}}}),
                    json!({"job_id":7,"name":"theirs","user":"bob","partition":"debug","nodes":"n1","state":{"current":["RUNNING"],"reason":"None"},"time":{"elapsed":5,"submission":150,"limit":{"set":true,"infinite":false,"number":30}}}),
                ];
                let (users, state) = (param("users"), param("state"));
                let jobs: Vec<Value> = jobs
                    .into_iter()
                    .filter(|j| users.as_deref().is_none_or(|u| j["user"] == u))
                    .filter(|j| state.as_deref().is_none_or(|s| j["state"]["current"][0] == s))
                    .collect();
                Json(json!({"jobs":jobs})).into_response()
            }
            // Finished long ago: only accounting still has it.
            ("GET", "slurmdb", "job/77") => Json(json!({"jobs":[{"job_id":77,"name":"old-run","user":"alice","account":"lab","partition":"debug","nodes":"n1","state":{"current":["COMPLETED"],"reason":"None"},"exit_code":{"status":["SUCCESS"],"return_code":{"set":true,"infinite":false,"number":0}},"time":{"elapsed":125,"submission":100,"start":110,"end":235,"limit":{"set":true,"infinite":false,"number":30}},"working_directory":"/home/alice","stdout_expanded":"/home/alice/slurm-77.out"}]})).into_response(),
            ("GET", "slurm", "partitions") => Json(json!({"partitions":[{"name":"debug","partition":{"state":["UP"]},"cpus":{"total":2}}]})).into_response(),
            ("GET", "slurm", "nodes") => Json(json!({"nodes":[
                {"name":"n1","state":["MIXED"],"cpus":4,"alloc_cpus":1,"real_memory":8000,"alloc_memory":2000,"gres":"gpu:a100:2(S:0)","gres_used":"gpu:a100:1(IDX:0)","partitions":["debug"]},
                {"name":"n2","state":["IDLE","DRAIN"],"cpus":8,"alloc_cpus":0,"real_memory":16000,"alloc_memory":0,"partitions":["debug"]},
                // In a partition the person's account doesn't see.
                {"name":"n3","state":["IDLE"],"cpus":16,"alloc_cpus":0,"real_memory":32000,"alloc_memory":0,"partitions":["hidden"]}
            ]})).into_response(),
            ("POST", "slurm", "job/submit") => Json(json!({"job_id":8,"errors":[]})).into_response(),
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

/// The decisions judge: a sure yes to everything, unless the person's messages say
/// "someone else's", which means the action was not theirs to ask for.
async fn verdicts(
    axum::extract::State(seen): axum::extract::State<Seen>,
    Json(body): Json<Value>,
) -> Json<Value> {
    seen.lock().unwrap().push(json!({"verdicts": body.clone()}));
    let doubtful = body["state"]["the person's messages"]
        .as_str()
        .is_some_and(|m| m.contains("someone else's"));
    let answers: serde_json::Map<String, Value> = body["questions"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, _)| {
            let yes = if doubtful && id == "asked" { 0.2 } else { 0.95 };
            (id.clone(), json!({"type":"boolean","value":yes>0.5,"probabilities":{"true":yes,"false":1.0-yes},"confidence":0.9}))
        })
        .collect();
    Json(json!({"model":"judge-31b","verdicts":answers}))
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

/// The fake object storage: key to bytes.
type Objects = Arc<Mutex<HashMap<String, Vec<u8>>>>;

async fn put_object(State(o): State<Objects>, Path(key): Path<String>, body: Bytes) -> StatusCode {
    o.lock().unwrap().insert(key, body.to_vec());
    StatusCode::OK
}
async fn get_object(State(o): State<Objects>, Path(key): Path<String>) -> Response {
    match o.lock().unwrap().get(&key) {
        Some(bytes) => (StatusCode::OK, bytes.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "<Error><Code>NoSuchKey</Code></Error>",
        )
            .into_response(),
    }
}

struct Fixture {
    app: Router,
    /// Where the app itself listens, for a cluster's file session job to reach.
    app_url: String,
    /// Object storage, where jobs bring their output back.
    objects: Objects,
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

async fn fixture_with(pool: PgPool, clusters: bool) -> Fixture {
    fixture_full(pool, clusters, true).await
}

/// `clusters`: whether Slurm and Kubernetes are offered (OBLETH_CHAT_CONNECTIONS).
async fn fixture_full(pool: PgPool, clusters: bool, storage: bool) -> Fixture {
    let objects: Objects = Arc::default();
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
        .route(
            "/bucket/{*key}",
            get(get_object).put(put_object).with_state(objects.clone()),
        )
        .route("/mcp", post(mcp).with_state(mcp_seen.clone()))
        .route(
            "/v1/models",
            get(|| async {
                Json(json!({"data":[{"id":"fixture-model"},{"id":"fixture-better"}]}))
            }),
        )
        .route(
            "/v1/chat/completions",
            post(chat).with_state((gateway_seen.clone(), script.clone())),
        )
        .route(
            "/v1/verdicts",
            post(verdicts).with_state(gateway_seen.clone()),
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
    // The app also listens, as it does in service, so a file session job (a real
    // python3 process in the test) can reach it.
    let app_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app_url = format!("http://{}", app_listener.local_addr().unwrap());
    // The mock listens on loopback, so private addresses are allowed here.
    state.connections = Some(
        Connections::new(pool, pem.as_bytes(), true)
            .unwrap()
            .clusters(clusters)
            .origin(Some(app_url.parse().unwrap()))
            .objects(storage.then(|| {
                ObjectStore::new(
                    format!("http://{address}").parse().unwrap(),
                    "bucket".into(),
                    "auto".into(),
                    "fixture-key".into(),
                    "fixture-secret".into(),
                )
            })),
    );
    let app = router(state.clone(), "branding".into());
    let served = app.clone();
    tokio::spawn(async move { axum::serve(app_listener, served).await.unwrap() });
    Fixture {
        objects,
        app,
        app_url,
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
    assert_eq!(
        names,
        ["fixture__lookup", "fixture__submit_job", "pause_work"]
    );
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
    // These cover the Allow card itself, so no decision may stand in for it.
    sqlx::query("UPDATE decisions SET check_actions = FALSE WHERE id")
        .execute(&pool)
        .await
        .unwrap();
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
    assert!(text.contains("6 mine [RUNNING]"), "{text}");
    assert!(text.contains("9 queued [PENDING]"), "{text}");
    assert!(text.contains("reason Priority"), "{text}");
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
async fn a_slurm_cluster_brings_job_output_back(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    // These cover the Allow card itself, so no decision may stand in for it.
    sqlx::query("UPDATE decisions SET check_actions = FALSE WHERE id")
        .execute(&pool)
        .await
        .unwrap();
    let admin = login(&f.app, "test-admin").await;
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true,"options":{"returnOutput":true}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(shared["returnOutput"], true);
    let tools: Vec<&str> = shared["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(tools.contains(&"output"), "{tools:?}");
    let id = shared["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &f.app,
        "PUT",
        &format!("/api/connections/{id}/credential"),
        json!({"token":SLURM_TOKEN,"user":"alice"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The job Slurm receives runs the script, then uploads its output to a link.
    *f.script.lock().unwrap() = tool_script(&[(
        "call_submit",
        "slurm__submit",
        json!({"script":"echo hi","name":"hello"}),
    )]);
    let (_, stream) = send(&f, &admin, "Submit hello").await;
    assert_eq!(pending(&pool).await, "call_submit");
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
    assert!(
        text.contains("read what it printed with Job output"),
        "{text}"
    );
    let submitted = cluster_requests(&f, "POST", "/slurm/v0.0.45/job/submit");
    let body: Value = serde_json::from_str(submitted[0]["body"].as_str().unwrap()).unwrap();
    let script = body["script"].as_str().unwrap();
    assert!(script.contains("\n#!/bin/bash\necho hi\n"), "{script}");
    assert_eq!(body["job"]["standard_output"], "slurm-%j.out");
    let upload = script
        .split('\'')
        .find(|part| part.starts_with("http://"))
        .expect("an upload link")
        .to_owned();
    assert!(
        upload.contains("/bucket/job-output/") && upload.contains("X-Amz-Signature="),
        "{upload}"
    );

    // Until the job ends there is nothing to read.
    let read = || {
        *f.script.lock().unwrap() =
            tool_script(&[("call_output", "slurm__output", json!({"job_id":8}))]);
        async {
            let (_, stream) = send(&f, &admin, "What did job 8 print?").await;
            tokio::time::timeout(Duration::from_secs(20), stream)
                .await
                .unwrap()
                .unwrap()
        }
    };
    let text = read().await;
    assert!(text.contains("Job 8 has not ended yet"), "{text}");

    // The job uploads the end of its output as it ends, and the model reads it.
    let uploaded = reqwest::Client::new()
        .put(&upload)
        .body("hi from the job\n")
        .send()
        .await
        .unwrap();
    assert!(uploaded.status().is_success());
    let text = read().await;
    assert!(text.contains("Output of job 8:"), "{text}");
    assert!(text.contains("hi from the job"), "{text}");
    // Another job's output, or another person's, is not there to read.
    *f.script.lock().unwrap() =
        tool_script(&[("call_output", "slurm__output", json!({"job_id":9}))]);
    let (_, stream) = send(&f, &admin, "And job 9?").await;
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("Chat has no output for job 9"), "{text}");
    assert_eq!(f.objects.lock().unwrap().len(), 1);

    // An output file name Chat can't follow runs the job as asked, and says its
    // output won't come back instead of promising it.
    *f.script.lock().unwrap() = tool_script(&[(
        "call_padded",
        "slurm__submit",
        json!({"script":"echo hi","output":"out-%4j.txt"}),
    )]);
    let (padded, stream) = send(&f, &admin, "Submit with a padded id").await;
    assert_eq!(pending(&pool).await, "call_padded");
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{padded}/approvals"),
        json!({"callId":"call_padded","decision":"allow"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(
        text.contains("Its output will not come back to Chat"),
        "{text}"
    );
    let submitted = cluster_requests(&f, "POST", "/slurm/v0.0.45/job/submit");
    let body: Value = serde_json::from_str(submitted[1]["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["script"], "#!/bin/bash\necho hi");
    assert_eq!(body["job"]["standard_output"], "out-%4j.txt");
}

#[sqlx::test(migrations = "./migrations")]
async fn job_details_come_from_accounting_once_the_scheduler_forgets_a_job(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true}),
        &admin,
    )
    .await;
    let id = shared["id"].as_str().unwrap().to_owned();
    call(
        &f.app,
        "PUT",
        &format!("/api/connections/{id}/credential"),
        json!({"token":SLURM_TOKEN,"user":"alice"}),
        &admin,
    )
    .await;
    *f.script.lock().unwrap() = tool_script(&[("call_job", "slurm__job", json!({"job_id":77}))]);
    let (_, stream) = send(&f, &admin, "How did job 77 go?").await;
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(
        text.contains("Job 77: old-run [COMPLETED], from accounting"),
        "{text}"
    );
    assert!(text.contains("Exit code 0"), "{text}");
    assert!(text.contains("ran 2m05s of 30m"), "{text}");
    assert!(text.contains("/home/alice/slurm-77.out"), "{text}");
}

#[sqlx::test(migrations = "./migrations")]
async fn job_output_needs_file_storage(pool: PgPool) {
    let f = fixture_full(pool, true, false).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, capabilities) = call(&f.app, "GET", "/api/capabilities", json!({}), &admin).await;
    assert_eq!(capabilities["features"]["jobOutput"], false);
    let (status, refused) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true,"options":{"returnOutput":true}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_cluster_answer_too_large_to_read_is_refused_not_held(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Slurm","url":f.cluster_url,"shared":true}),
        &admin,
    )
    .await;
    let id = shared["id"].as_str().unwrap().to_owned();
    let (status, refused) = call(
        &f.app,
        "PUT",
        &format!("/api/connections/{id}/credential"),
        json!({"token":HUGE_SLURM_TOKEN,"user":"alice"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("larger than 32 MiB"),
        "{refused}"
    );
    // Chat carries on with the next request.
    let (status, _) = call(&f.app, "GET", "/api/connections", Value::Null, &admin).await;
    assert_eq!(status, StatusCode::OK);
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
    // Counts, not every node: the drained node is in the totals but never idle, and
    // a node only in a partition the account doesn't see is left out.
    assert!(s.get("nodes").is_none(), "{s}");
    let t = &s["totals"];
    assert_eq!(
        (
            t["nodes"].clone(),
            t["idleNodes"].clone(),
            t["unavailableNodes"].clone()
        ),
        (json!(2), json!(0), json!(1))
    );
    assert_eq!(
        (
            t["cpus"].clone(),
            t["allocCpus"].clone(),
            t["idleCpus"].clone()
        ),
        (json!(12), json!(1), json!(3))
    );
    assert_eq!(
        (t["gpus"].clone(), t["idleGpus"].clone()),
        (json!(2), json!(1))
    );
    assert_eq!(s["partitions"][0]["name"], "debug");
    assert_eq!(s["partitions"][0]["idleCpus"], 3);
    // The queue's size comes from the scheduler's statistics.
    assert_eq!(
        (s["queue"]["running"].clone(), s["queue"]["pending"].clone()),
        (json!(2), json!(3))
    );
    // Only the person's own jobs, newest first, asked of accounting by user name.
    let ids: Vec<&Value> = s["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| &j["id"])
        .collect();
    assert_eq!(ids, [&json!(9), &json!(6)]);
    assert_eq!(
        (
            s["jobs"][1]["elapsed"].clone(),
            s["jobs"][1]["timeLimit"].clone()
        ),
        (json!("1m15s"), json!("30m"))
    );
    assert_eq!(s["jobs"][0]["nodes"], "");
    let asked: Vec<String> = cluster_requests(&f, "GET", "/slurmdb/v0.0.45/jobs")
        .iter()
        .map(|r| r["query"].as_str().unwrap().to_owned())
        .collect();
    // Running and pending jobs, then the ones that ended recently.
    assert_eq!(
        asked[..2],
        ["users=alice&state=RUNNING", "users=alice&state=PENDING"]
    );
    assert_eq!(asked.len(), 3);
    assert!(asked[2].starts_with("users=alice&start_time="));
    assert!(cluster_requests(&f, "GET", "/slurm/v0.0.45/jobs").is_empty());
    // A second look within half a minute asks for partitions with the person's own
    // token again, but takes the nodes from the first.
    let (code, again) = call(
        &f.app,
        "GET",
        &format!("/api/connections/{slurm}/status"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(again["status"]["totals"], *t);
    assert_eq!(cluster_requests(&f, "GET", "/slurm/v0.0.45/nodes").len(), 1);
    assert!(cluster_requests(&f, "GET", "/slurm/v0.0.45/partitions").len() >= 2);

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
    let app = router(state, "branding".into());
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

#[sqlx::test(migrations = "./migrations")]
async fn pausing_ends_the_reply_with_where_the_work_stands(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    // A pause without next steps is sent back, and the reply goes on.
    *f.script.lock().unwrap() =
        tool_script(&[("call_p", "pause_work", json!({"state":"Stopped the job."}))]);
    let (first, stream) = start_turn(&f, &admin).await;
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("Nothing was paused"), "{text}");
    let offered: Vec<String> = f.gateway_seen.lock().unwrap()[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(offered.iter().any(|t| t == "pause_work"), "{offered:?}");

    // A full note ends the reply after that round, shown at its end.
    *f.script.lock().unwrap() = tool_script(&[(
        "call_q",
        "pause_work",
        json!({"done":"Looked up job 42.","state":"Nothing is running.","next":"Resubmit job 42 with the same script."}),
    )]);
    let before = f.gateway_seen.lock().unwrap().len();
    let (id, stream) = send(&f, &admin, "Check job 42, then pause there").await;
    tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        f.gateway_seen.lock().unwrap().len(),
        before + 1,
        "nothing runs after the pause"
    );
    let uuid: uuid::Uuid = id.parse().unwrap();
    let (content, status, activity): (String, String, Value) = sqlx::query_as(
        "SELECT content,status,activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "completed");
    assert!(
        content.starts_with("**Paused.** Where the work stands is below."),
        "{content}"
    );
    assert!(
        content.ends_with("**Next**\n\nResubmit job 42 with the same script."),
        "{content}"
    );
    let step = activity
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["tool"] == "pause_work")
        .expect("a pause step");
    assert_eq!(step["summary"], "Paused at your request");
    // Auto mode leaves it alone until the person writes again.
    let nudges: i32 = sqlx::query_scalar("SELECT auto_nudges FROM conversations WHERE id=$1")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(nudges, obleth_chat::autopilot::MAX_NUDGES);
    let other: uuid::Uuid = first.parse().unwrap();
    let nudges: i32 = sqlx::query_scalar("SELECT auto_nudges FROM conversations WHERE id=$1")
        .bind(other)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(nudges, 0);
}

/// Waits for a device call the response sent to the browser.
async fn device_call(f: &Fixture, id: &str, session: &(String, String)) -> Value {
    for _ in 0..100 {
        let (status, body) = call(
            &f.app,
            "GET",
            &format!("/api/conversations/{id}/device-calls"),
            Value::Null,
            session,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if let Some(first) = body["calls"].as_array().and_then(|c| c.first()) {
            return first.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no device call waited for the browser");
}

#[sqlx::test(migrations = "./migrations")]
async fn device_tools_are_carried_out_by_the_persons_browser(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    // A device is the person's own, takes no address or token, and is never checked.
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Board","url":"","kind":"device","options":{"vendorId":0x303a,"productId":4097,"label":"USB JTAG/serial debug unit"}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["kind"], "device");
    assert_eq!(created["vendorId"], 0x303a);
    assert_eq!(created["label"], "USB JTAG/serial debug unit");
    assert_eq!(created["serverName"], "USB JTAG/serial debug unit");
    assert_eq!(created["hasToken"], false);
    let tools: Vec<&str> = created["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        [
            "status", "open", "close", "read", "write", "signals", "login", "identify", "flash",
            "erase"
        ]
    );
    let device = created["id"].as_str().unwrap().to_owned();
    let (status, refused) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Shared board","url":"","kind":"device","shared":true}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, refused) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{device}"),
        json!({"url":"https://example.com/mcp"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    // Renaming keeps the USB ids; a new label is kept too.
    let (status, renamed) = call(
        &f.app,
        "PATCH",
        &format!("/api/connections/{device}"),
        json!({"name":"My board","options":{"label":"ESP32-S3"}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "My board");
    assert_eq!(renamed["vendorId"], 0x303a);
    assert_eq!(renamed["label"], "ESP32-S3");
    let (status, refreshed) = call(
        &f.app,
        "POST",
        &format!("/api/connections/{device}/refresh"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{refreshed}");

    // A read runs without asking; a write waits for Allow. Both are carried out by
    // the browser, which posts each result.
    *f.script.lock().unwrap() = tool_script(&[
        ("call_read", "my_board__read", json!({"waitMs":10})),
        ("call_write", "my_board__write", json!({"text":"AT"})),
    ]);
    let (id, stream) = send(&f, &admin, "Talk to the board").await;
    let first = device_call(&f, &id, &admin).await;
    assert_eq!(first["callId"], "call_read");
    assert_eq!(first["tool"], "read");
    assert_eq!(first["arguments"], json!({"waitMs":10}));
    assert_eq!(first["connectionId"], device);
    // The saved step carries the request, so a reloaded page can carry it out.
    let activity: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activity[0]["service"], "device");
    assert_eq!(activity[0]["connection"], "My board");
    assert_eq!(activity[0]["status"], "running");
    assert_eq!(
        activity[0]["request"],
        json!({"tool":"read","arguments":{"waitMs":10}})
    );
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/device-calls/call_read"),
        json!({"ok":true,"output":"ready>\r\n","detail":"7 bytes"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/device-calls/call_read"),
        json!({"ok":true,"output":"again"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    assert_eq!(pending(&pool).await, "call_write");
    let activity: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activity[0]["status"], "completed");
    assert_eq!(activity[0]["detail"], "ready>\r\n");
    assert!(activity[0].get("request").is_none());
    assert_eq!(activity[1]["status"], "awaiting");
    assert_eq!(activity[1]["change"]["items"][0]["verb"], "Send");
    assert_eq!(activity[1]["change"]["items"][0]["what"], "AT");
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/approvals"),
        json!({"callId":"call_write","decision":"allow"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let second = device_call(&f, &id, &admin).await;
    assert_eq!(second["callId"], "call_write");
    assert_eq!(second["arguments"], json!({"text":"AT"}));
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/device-calls/call_write"),
        json!({"ok":true,"output":"Sent 3 bytes"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("\"status\":\"completed\""), "{text}");
    assert!(
        text.contains(r#"Done: ready>\r\n | Sent 3 bytes"#),
        "{text}"
    );
    // The model was told what the device is, under the connection's prefix.
    let first = f.gateway_seen.lock().unwrap()[0].clone();
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"my_board__flash"), "{names:?}");
    let prompt = first["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("My board, a device plugged into the person's computer (ESP32-S3), reached through their browser; no console sign-in is saved"), "{prompt}");
    assert!(prompt.contains("(tools named my_board__…)"), "{prompt}");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM device_calls")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_model_can_ask_for_a_device_at_the_persons_computer(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    *f.script.lock().unwrap() = tool_script(&[(
        "call_ask",
        "request_device",
        json!({"reason":"To read what the BeagleBone prints.","name":"BeagleBone"}),
    )]);
    // The auto model is one that calls tools; with no connections, only the offer
    // tools are on.
    let (status, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Board","model":"auto"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{chat}");
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            json!({"content":"Connect to my BeagleBone","model":"auto"}),
            &admin,
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
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("\"kind\":\"device-request\""), "{text}");
    assert!(
        text.contains("To read what the BeagleBone prints."),
        "{text}"
    );
    let activity: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activity[0]["kind"], "device-request");
    assert_eq!(activity[0]["name"], "BeagleBone");
    // The model was told the person sees the button, and had the tool offered.
    let seen = f.gateway_seen.lock().unwrap().clone();
    let names: Vec<&str> = seen[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"request_device"), "{names:?}");
    // The card ends the reply: the model is not called again until the person answers.
    assert_eq!(seen.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn secrets_are_named_to_the_model_and_opened_only_by_the_owners_browser(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, refused) = call(
        &f.app,
        "PUT",
        "/api/secrets/has%20space",
        json!({"value":"x"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, saved) = call(
        &f.app,
        "PUT",
        "/api/secrets/bb-pass",
        json!({"value":"temppwd","note":"BeagleBone console"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["name"], "bb-pass");
    assert_eq!(saved["note"], "BeagleBone console");
    assert!(saved.get("value").is_none());
    let (status, listed) = call(&f.app, "GET", "/api/secrets", Value::Null, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["secrets"][0]["name"], "bb-pass");
    assert!(!listed.to_string().contains("temppwd"));
    // Only the note changes when no value is sent; the value opens for the owner.
    let (status, noted) = call(
        &f.app,
        "PUT",
        "/api/secrets/bb-pass",
        json!({"note":"console"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{noted}");
    let (status, value) = call(
        &f.app,
        "GET",
        "/api/secrets/bb-pass/value",
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value, json!({"name":"bb-pass","value":"temppwd"}));
    // A device set to sign in with it: the model is told the user and the secret's
    // name, and the names of the secrets, never a value.
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"name":"Beagle","url":"","kind":"device","options":{"vendorId":0x1d6b,"user":"debian","secret":"bb-pass"}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["user"], "debian");
    assert_eq!(created["secret"], "bb-pass");
    *f.script.lock().unwrap() = tool_script(&[("call_l", "beagle__login", json!({}))]);
    let (id, stream) = send(&f, &admin, "Sign in to the board").await;
    let first = device_call(&f, &id, &admin).await;
    assert_eq!(first["tool"], "login");
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{id}/device-calls/call_l"),
        json!({"ok":true,"output":"Signed in as debian."}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("Done: Signed in as debian."), "{text}");
    let seen = f.gateway_seen.lock().unwrap().clone();
    let prompt = seen[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        prompt.contains("user debian, password in the secret bb-pass"),
        "{prompt}"
    );
    assert!(prompt.contains("saved secrets named bb-pass"), "{prompt}");
    assert!(!seen.iter().any(|r| r.to_string().contains("temppwd")));
    let (status, _) = call(
        &f.app,
        "DELETE",
        "/api/secrets/bb-pass",
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &f.app,
        "GET",
        "/api/secrets/bb-pass/value",
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The session job's batch script and environment, from what slurmrestd received.
fn session_job(f: &Fixture, index: usize) -> (String, Vec<(String, String)>, Value) {
    let submitted = cluster_requests(f, "POST", "/slurm/v0.0.45/job/submit");
    let body: Value = serde_json::from_str(submitted[index]["body"].as_str().unwrap()).unwrap();
    let env: Vec<(String, String)> = body["job"]["environment"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let (k, v) = e.as_str().unwrap().split_once('=').unwrap();
            (k.to_owned(), v.to_owned())
        })
        .collect();
    (
        body["script"].as_str().unwrap().to_owned(),
        env,
        body["job"].clone(),
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn a_slurm_cluster_reaches_files_through_a_session_job(pool: PgPool) {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("python3 is not installed; skipping");
        return;
    }
    let f = fixture(pool.clone()).await;
    // These cover the Allow card itself, so no decision may stand in for it.
    sqlx::query("UPDATE decisions SET check_actions = FALSE WHERE id")
        .execute(&pool)
        .await
        .unwrap();
    let admin = login(&f.app, "test-admin").await;
    // The person's files, as the cluster's home directory and a scratch space.
    let home = std::env::temp_dir().join(format!("chat-files-home-{}", Uuid::new_v4().simple()));
    let scratch =
        std::env::temp_dir().join(format!("chat-files-scratch-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(home.join("project")).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        home.join("project/run.sh"),
        "#!/bin/bash\nsrun python train.py\n",
    )
    .unwrap();
    std::fs::write(scratch.join("out.log"), "step 1\nerror: out of memory\n").unwrap();
    let roots = format!("~,{}", scratch.display());
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Sol","url":f.cluster_url,"shared":true,"options":{"files":true,"filesPartition":"htc","filesHours":4,"filesRoots":roots}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(shared["files"], true);
    assert_eq!(shared["filesPartition"], "htc");
    assert_eq!(shared["filesRoots"], roots);
    let tools: Vec<&str> = shared["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        tools.contains(&"files_list") && tools.contains(&"files_remove"),
        "{tools:?}"
    );
    let id = shared["id"].as_str().unwrap().to_owned();
    // The job runs as whoever runs this test, and must say so.
    let me = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| {
            String::from_utf8(
                std::process::Command::new("id")
                    .arg("-un")
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_owned()
        });
    let (status, _) = call(
        &f.app,
        "PUT",
        &format!("/api/connections/{id}/credential"),
        json!({"token":SLURM_TOKEN,"user":me}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The first files call starts the session job: a small job as the person, whose
    // script carries the agent and whose environment carries the session token.
    *f.script.lock().unwrap() =
        tool_script(&[("call_list", "sol__files_list", json!({"path":"project"}))]);
    let (_, stream) = send(&f, &admin, "What is in my project directory?").await;
    for _ in 0..100 {
        if !cluster_requests(&f, "POST", "/slurm/v0.0.45/job/submit").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (script, env, job) = session_job(&f, 0);
    assert!(script.starts_with("#!/bin/bash\n"), "{script}");
    assert!(
        script.contains("exec python3 - <<'OBLETH_CHAT_AGENT_"),
        "{script}"
    );
    assert_eq!(job["name"], "chat-files");
    assert_eq!(job["partition"], "htc");
    assert_eq!(job["time_limit"]["number"], 240);
    assert_eq!(job["cpus_per_task"], 1);
    assert_eq!(job["current_working_directory"], format!("/home/{me}"));
    assert!(job.get("qos").is_none());
    let var = |name: &str| {
        env.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(var("OBLETH_CHAT_URL"), f.app_url);
    assert_eq!(var("OBLETH_CHAT_ROOTS"), roots);
    assert_eq!(var("OBLETH_CHAT_SESSION").len(), 64);
    assert!(!script.contains(&var("OBLETH_CHAT_SESSION")));

    // Run the job's script as Slurm would, as this user, with the test's home.
    let dir = std::env::temp_dir().join(format!("chat-files-job-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("job.sh"), &script).unwrap();
    let log = std::fs::File::create(dir.join("job.log")).unwrap();
    let mut command = std::process::Command::new("bash");
    command
        .arg(dir.join("job.sh"))
        .env_clear()
        .env("HOME", &home)
        .env("USER", &me)
        .env("LOGNAME", &me)
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    for (k, v) in &env {
        command.env(k, v);
    }
    let mut child = command.spawn().unwrap();
    let text = tokio::time::timeout(Duration::from_secs(60), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(
        text.contains("A file session started on the cluster as job 8"),
        "{text}"
    );
    assert!(text.contains("run.sh"), "{text}");
    assert!(text.contains("(1 entries, hidden ones left out)"), "{text}");
    let (state, host): (String, Option<String>) =
        sqlx::query_as("SELECT state, host FROM cluster_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "live");
    assert!(host.is_some_and(|h| !h.is_empty()));

    // Later calls, in any chat, go down the same line: no new job.
    *f.script.lock().unwrap() = tool_script(&[(
        "call_search",
        "sol__files_search",
        json!({"path":scratch.display().to_string(),"pattern":"out of memory"}),
    )]);
    let (_, stream) = send(&f, &admin, "Why did it fail?").await;
    let text = tokio::time::timeout(Duration::from_secs(60), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("out.log:2: error: out of memory"), "{text}");
    assert!(!text.contains("A file session started"), "{text}");
    assert_eq!(
        cluster_requests(&f, "POST", "/slurm/v0.0.45/job/submit").len(),
        1
    );

    // Outside the roots is refused on the cluster, in words the model can act on.
    *f.script.lock().unwrap() =
        tool_script(&[("call_etc", "sol__files_list", json!({"path":"/etc"}))]);
    let (_, stream) = send(&f, &admin, "List /etc").await;
    let text = tokio::time::timeout(Duration::from_secs(60), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("outside what Chat may reach here"), "{text}");

    // A write waits for Allow, then lands as the person.
    *f.script.lock().unwrap() = tool_script(&[(
        "call_write",
        "sol__files_write",
        json!({"path":"project/notes.md","content":"The run ran out of memory.\n"}),
    )]);
    let (chat, stream) = send(&f, &admin, "Write that down").await;
    assert_eq!(pending(&pool).await, "call_write");
    assert!(!home.join("project/notes.md").exists());
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{chat}/approvals"),
        json!({"callId":"call_write","decision":"allow"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(60), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("Wrote "), "{text}");
    assert_eq!(
        std::fs::read_to_string(home.join("project/notes.md")).unwrap(),
        "The run ran out of memory.\n"
    );

    // Removing the person's credential ends the session; the job learns it at its
    // next poll and exits.
    let (status, _) = call(
        &f.app,
        "DELETE",
        &format!("/api/connections/{id}/credential"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let exit = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();
    assert!(
        exit.success(),
        "{}",
        std::fs::read_to_string(dir.join("job.log")).unwrap()
    );
    let (state, error): (String, Option<String>) =
        sqlx::query_as("SELECT state, error FROM cluster_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "ended");
    assert_eq!(error.as_deref(), Some("The file session was ended"));
    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&scratch).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_session_job_running_as_someone_else_is_turned_away(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Sol","url":f.cluster_url,"shared":true,"options":{"files":true}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let connection = Uuid::parse_str(shared["id"].as_str().unwrap()).unwrap();
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username='test-admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    // A session as Chat records one, for a job whose token is this.
    let token = "ab".repeat(32);
    let hash = <sha2::Sha256 as sha2::Digest>::digest(token.as_bytes()).to_vec();
    sqlx::query("INSERT INTO cluster_sessions(id,connection_id,user_id,job_id,token_hash,state,username,ends_at) VALUES($1,$2,$3,8,$4,'starting','alice',NOW()+INTERVAL '1 hour')")
        .bind(Uuid::new_v4()).bind(connection).bind(user).bind(&hash)
        .execute(&pool).await.unwrap();
    let client = reqwest::Client::new();
    let hello = |who: &str, token: &str| {
        client
            .post(format!("{}/api/cluster-sessions/hello", f.app_url))
            .bearer_auth(token)
            .json(&json!({"username":who,"host":"n1","version":1}))
            .send()
    };
    // No token, or a wrong one, is no session.
    assert_eq!(
        hello("alice", "cd".repeat(32).as_str())
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(hello("alice", "short").await.unwrap().status(), 401);
    // The right token from the wrong account ends the session.
    let turned = hello("mallory", &token).await.unwrap();
    assert_eq!(turned.status(), 409);
    assert!(turned
        .text()
        .await
        .unwrap()
        .contains("ran as mallory, not alice"));
    let (state, error): (String, Option<String>) =
        sqlx::query_as("SELECT state, error FROM cluster_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "ended");
    assert!(error.unwrap().contains("belongs to another account"));
    let next = client
        .get(format!("{}/api/cluster-sessions/next", f.app_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(next.status(), 410);
}

/// The decisions the gateway was asked while a change waited.
fn judged(f: &Fixture) -> Vec<Value> {
    f.gateway_seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|m| m.get("verdicts").cloned())
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_decision_lets_a_change_the_person_asked_for_run_unasked(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, shared) = call(
        &f.app,
        "POST",
        "/api/connections",
        json!({"kind":"slurm","name":"Sol","url":f.cluster_url,"shared":true,"options":{"files":true}}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let id = shared["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &f.app,
        "PUT",
        &format!("/api/connections/{id}/credential"),
        json!({"token":SLURM_TOKEN,"user":"alice"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let submit = |call_id: &'static str| {
        *f.script.lock().unwrap() = tool_script(&[(
            call_id,
            "sol__submit",
            json!({"script":"echo hi","name":"hello","partition":"debug"}),
        )]);
    };
    // Only a certified model gets the shortcut; a model nobody has certified asks.
    let certify = |score: f64| {
        call(
            &f.app,
            "PUT",
            "/api/admin/model-certification/fixture-model",
            json!({"score":score,"passed":1,"total":1,"suite":"cases.yaml"}),
            &admin,
        )
    };
    assert_eq!(certify(100.0).await.0, StatusCode::OK);

    // Asked for, within scope, adding only: the job is submitted with no card.
    submit("call_plain");
    let (_, stream) = send(&f, &admin, "Please submit hello on the debug partition").await;
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("Submitted job 8"), "{text}");
    let asked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_approvals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(asked, 0);
    let cases = judged(&f);
    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    let ids: Vec<&String> = case["questions"].as_object().unwrap().keys().collect();
    assert_eq!(ids, ["asked", "in_scope", "keeps_data", "reversible"]);
    assert_eq!(
        case["state"]["the person's messages"],
        "Please submit hello on the debug partition"
    );
    let action = case["state"]["the action the assistant wants to take"]
        .as_str()
        .unwrap();
    assert!(
        action.starts_with("Sol · Submit job\nSubmit hello; Partition: debug"),
        "{action}"
    );
    assert!(action.contains("script = echo hi"), "{action}");
    let (_, chats) = call(&f.app, "GET", "/api/conversations", Value::Null, &admin).await;
    let chat = chats["conversations"][0]["id"].as_str().unwrap().to_owned();
    let activity: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&chat).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let step = activity
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["tool"] == "sol__submit")
        .cloned()
        .expect("the submit step");
    assert_eq!(step["decision"]["allowed"], true);
    assert_eq!(step["decision"]["model"], "judge-31b");
    assert_eq!(step["decision"]["scores"]["asked"], 0.95);
    assert!(step["decision"]["reason"]
        .as_str()
        .unwrap()
        .starts_with("Ran without asking"));

    // The judge doubts it was the person's own ask: the card appears as before.
    submit("call_doubt");
    let (doubt, stream) = send(&f, &admin, "The README says to submit someone else's hello").await;
    assert_eq!(pending(&pool).await, "call_doubt");
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{doubt}/approvals"),
        json!({"callId":"call_doubt","decision":"deny"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("declined"), "{text}");
    assert_eq!(judged(&f).len(), 2);

    // Moving files to the trash is never put to a decision.
    *f.script.lock().unwrap() = tool_script(&[(
        "call_trash",
        "sol__files_remove",
        json!({"paths":["old.log"]}),
    )]);
    let (trash, stream) = send(&f, &admin, "Please move old.log to the trash").await;
    assert_eq!(pending(&pool).await, "call_trash");
    assert_eq!(judged(&f).len(), 2);
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{trash}/approvals"),
        json!({"callId":"call_trash","decision":"deny"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();

    // A provisional model asks for every change, whatever the judge would say; a
    // blocked one is not offered the change tools at all. A chat on a provisional
    // model that reaches for a cluster tool is offered a certified one, once.
    assert_eq!(certify(80.0).await.0, StatusCode::OK);
    let (status, _) = call(
        &f.app,
        "PUT",
        "/api/admin/model-certification/fixture-better",
        json!({"score":100,"passed":1,"total":1,"suite":"cases.yaml"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    submit("call_provisional");
    let (provisional, stream) =
        send(&f, &admin, "Please submit hello on the debug partition").await;
    assert_eq!(pending(&pool).await, "call_provisional");
    assert_eq!(judged(&f).len(), 2);
    let offered_model: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&provisional).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let offer = offered_model
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "model")
        .cloned()
        .expect("a model offer");
    assert_eq!(offer["model"], "fixture-model");
    assert_eq!(offer["suggested"], "fixture-better");
    assert_eq!(offer["score"], 100.0);
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{provisional}/suggestion"),
        json!({"outcome":"never"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, prefs) = call(&f.app, "GET", "/api/preferences", Value::Null, &admin).await;
    assert_eq!(prefs["preferences"]["modelSuggestions"], false);
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{provisional}/suggestion"),
        json!({"outcome":"maybe"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{provisional}/approvals"),
        json!({"callId":"call_provisional","decision":"deny"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    // With suggestions off, the next such chat gets no offer.
    submit("call_quiet");
    let (quiet, stream) = send(&f, &admin, "Please submit hello on the debug partition").await;
    assert_eq!(pending(&pool).await, "call_quiet");
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{quiet}/approvals"),
        json!({"callId":"call_quiet","decision":"deny"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    let quiet_activity: Value = sqlx::query_scalar(
        "SELECT activity FROM messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(Uuid::parse_str(&quiet).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!quiet_activity
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "model"));
    let shown: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM model_suggestions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(shown, 1);
    let (status, _) = call(
        &f.app,
        "PUT",
        "/api/admin/models/fixture-model",
        json!({"name":"Fixture","actions":"blocked"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    submit("call_blocked");
    let (_, stream) = send(&f, &admin, "Please submit hello on the debug partition").await;
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(!text.contains("Submitted job 8"), "{text}");
    let offered: Vec<String> = f
        .gateway_seen
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m.get("tools").is_some())
        .unwrap()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        offered.iter().any(|t| t == "sol__files_list"),
        "{offered:?}"
    );
    assert!(
        !offered
            .iter()
            .any(|t| t == "sol__submit" || t == "sol__files_write"),
        "{offered:?}"
    );
    let prompt = f
        .gateway_seen
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m.get("tools").is_some())
        .unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        prompt.contains("You may only read through these services"),
        "{prompt}"
    );
    // Back to certified for the rest.
    assert_eq!(certify(100.0).await.0, StatusCode::OK);
    let (status, _) = call(
        &f.app,
        "PUT",
        "/api/admin/models/fixture-model",
        json!({"name":"Fixture","actions":"auto"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // With the setting off, every change asks.
    sqlx::query("UPDATE decisions SET check_actions = FALSE WHERE id")
        .execute(&pool)
        .await
        .unwrap();
    submit("call_off");
    let (off, stream) = send(&f, &admin, "Please submit hello on the debug partition").await;
    assert_eq!(pending(&pool).await, "call_off");
    assert_eq!(judged(&f).len(), 2);
    let (status, _) = call(
        &f.app,
        "POST",
        &format!("/api/conversations/{off}/approvals"),
        json!({"callId":"call_off","decision":"allow"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let text = tokio::time::timeout(Duration::from_secs(20), stream)
        .await
        .unwrap()
        .unwrap();
    assert!(text.contains("Submitted job 8"), "{text}");
}
