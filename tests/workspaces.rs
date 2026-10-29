//! Workspaces with and without a preview domain: without one, workspaces still run,
//! Chat says previews are off and leaves preview-like hosts alone. (The frame policy
//! is set once per process, so it isn't compared across these routers.) Previews
//! only reach a pod the Kubernetes API still reports at the address on record.
//! Requires a disposable DATABASE_URL; no cluster is contacted: a fake API server
//! answers for pods.
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode, Uri},
    routing::get,
    Json, Router,
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
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";
const DOMAIN: &str = "sbx.example.com";
const POD_SLUG: &str = "podslug000000001";
const SLUG: &str = "abcdefghijklmnop";

/// The Kubernetes client is configured from the environment when Sandboxes is built.
static KUBE_ENV: Mutex<()> = Mutex::new(());

/// A Kubernetes API that knows only the pods a test gives it, and notes each request.
#[derive(Default)]
struct FakeKube {
    pods: Mutex<Vec<Value>>,
    seen: Mutex<Vec<String>>,
    /// Answers 500 to everything, as an API server that is down.
    down: std::sync::atomic::AtomicBool,
}

impl FakeKube {
    fn set_pod(&self, ip: Option<&str>) {
        *self.pods.lock().unwrap() = ip
            .map(|ip| {
                json!({
                    "metadata": {"name": format!("sbx-{POD_SLUG}"), "labels": {
                        "app.kubernetes.io/managed-by": "obleth-chat",
                        "app.kubernetes.io/name": "obleth-sandbox",
                        "obleth.io/pod": POD_SLUG,
                    }},
                    "status": {"phase": "Running", "podIP": ip, "containerStatuses": [{"ready": true}]},
                })
            })
            .into_iter()
            .collect();
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

struct TlsListener {
    tcp: tokio::net::TcpListener,
    tls: tokio_rustls::TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((stream, address)) = self.tcp.accept().await else {
                continue;
            };
            if let Ok(stream) = self.tls.accept(stream).await {
                return (stream, address);
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// Serves `kube` over TLS on a local port; returns its URL and CA file.
async fn serve_kube(kube: Arc<FakeKube>) -> (String, PathBuf) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .unwrap()
        .signed_by(&key, &ca, &ca_key)
        .unwrap();
    let ca_file = std::env::temp_dir().join(format!("obleth-fake-kube-{}.pem", Uuid::new_v4()));
    std::fs::write(&ca_file, ca.pem()).unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let routes = Router::new()
        .route(
            "/api/v1/namespaces/nowhere/pods/{name}",
            get(
                |State(kube): State<Arc<FakeKube>>, Path(name): Path<String>| async move {
                    let pod = kube
                        .pods
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|p| p["metadata"]["name"] == name.as_str())
                        .cloned();
                    match pod {
                        Some(pod) => (StatusCode::OK, Json(pod)),
                        None => (StatusCode::NOT_FOUND, Json(json!({"message": "not found"}))),
                    }
                },
            )
            .delete(
                |State(kube): State<Arc<FakeKube>>, Path(name): Path<String>| async move {
                    let mut pods = kube.pods.lock().unwrap();
                    let before = pods.len();
                    pods.retain(|p| p["metadata"]["name"] != name.as_str());
                    if pods.len() < before {
                        (StatusCode::OK, Json(json!({})))
                    } else {
                        (StatusCode::NOT_FOUND, Json(json!({"message": "not found"})))
                    }
                },
            ),
        )
        .route(
            "/api/v1/namespaces/nowhere/pods",
            get(|State(kube): State<Arc<FakeKube>>| async move {
                Json(json!({"items": *kube.pods.lock().unwrap()}))
            }),
        )
        .route(
            "/api/v1/namespaces/nowhere/persistentvolumeclaims",
            get(|| async { Json(json!({"items": []})) }),
        )
        .layer(axum::middleware::from_fn_with_state(
            kube.clone(),
            |State(kube): State<Arc<FakeKube>>,
             request: Request<Body>,
             next: axum::middleware::Next| async move {
                kube.seen.lock().unwrap().push(format!(
                    "{} {}",
                    request.method(),
                    request.uri().path()
                ));
                if kube.down.load(std::sync::atomic::Ordering::SeqCst) {
                    return axum::response::IntoResponse::into_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                    );
                }
                next.run(request).await
            },
        ))
        .with_state(kube);
    let listener = TlsListener {
        tcp: listener,
        tls: tokio_rustls::TlsAcceptor::from(Arc::new(tls)),
    };
    tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    (format!("https://{address}"), ca_file)
}

async fn app(pool: PgPool, preview_domain: Option<&str>) -> Router {
    app_with(pool, preview_domain, None).await.0
}

async fn app_with(
    pool: PgPool,
    preview_domain: Option<&str>,
    kube: Option<Arc<FakeKube>>,
) -> (Router, Sandboxes) {
    // Without a fake API server the Kubernetes client points nowhere, so nothing
    // here starts a pod.
    let (api, ca) = match kube {
        Some(kube) => serve_kube(kube).await,
        None => {
            let ca = std::env::temp_dir().join("obleth-workspaces-test-ca.pem");
            std::fs::write(&ca, "").unwrap();
            ("https://127.0.0.1:9".to_owned(), ca)
        }
    };
    let token = std::env::temp_dir().join("obleth-workspaces-test-token");
    std::fs::write(&token, "fixture-kube-token").unwrap();
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
        slots: 8,
        preview_domain: preview_domain.map(Into::into),
        preview_scheme: "http".into(),
        chat_origin: ORIGIN.into(),
        idle: Duration::from_secs(3600),
        retention: Duration::from_secs(86400),
    };
    let sandboxes = {
        let _env = KUBE_ENV.lock().unwrap();
        std::env::set_var("OBLETH_CHAT_KUBE_API", &api);
        std::env::set_var("OBLETH_CHAT_KUBE_CA_FILE", &ca);
        std::env::set_var("OBLETH_CHAT_KUBE_TOKEN_FILE", &token);
        Sandboxes::new(config, pool.clone()).unwrap()
    };
    let mut state = AppState::new(auth, pool, None, 4);
    state.sandboxes = Some(sandboxes.clone());
    (router(state, "web".into()), sandboxes)
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

/// A dev server standing in for whatever pod now answers at an address.
async fn serve_app() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = Router::new().fallback(|| async { "answered by the pod at this address" });
    tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    port
}

/// The admin's conversation with a project in a pod recorded as running at 127.0.0.1.
async fn workspace(pool: &PgPool, preview_port: u16, status: &str) -> Uuid {
    let owner: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username='test-admin'")
        .fetch_one(pool)
        .await
        .unwrap();
    let conversation = Uuid::new_v4();
    sqlx::query("INSERT INTO conversations(id,owner_id,title,model) VALUES($1,$2,'Site','m')")
        .bind(conversation)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    let pod = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sandbox_pods(id,owner_id,slug,status,pod_ip) VALUES($1,$2,$3,$4,'127.0.0.1')",
    )
    .bind(pod)
    .bind(owner)
    .bind(POD_SLUG)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO sandboxes(id,conversation_id,owner_id,slug,pod_id,port,preview_port) VALUES($1,$2,$3,$4,$5,3000,$6)")
        .bind(Uuid::new_v4())
        .bind(conversation)
        .bind(owner)
        .bind(SLUG)
        .bind(pod)
        .bind(i32::from(preview_port))
        .execute(pool)
        .await
        .unwrap();
    conversation
}

async fn pod_record(pool: &PgPool) -> (String, Option<String>, Option<i32>) {
    sqlx::query_as("SELECT p.status,p.pod_ip,s.preview_port FROM sandbox_pods p JOIN sandboxes s ON s.pod_id=p.id")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn preview_request(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .uri(uri)
        .header("host", format!("{SLUG}.{DOMAIN}"));
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    request.body(Body::empty()).unwrap()
}

/// Opens the preview as Chat's page does; returns the preview cookie.
async fn enter_preview(app: &Router, session: &(String, String), conversation: Uuid) -> String {
    let response = app
        .clone()
        .oneshot(req(
            &format!("/api/conversations/{conversation}/workspace/preview"),
            "localhost:8085",
            session,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let link: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(link["url"], format!("http://{SLUG}.{DOMAIN}/"));
    let response = app
        .clone()
        .oneshot(enter_request(&link, Some(ORIGIN), "POST"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
    assert_eq!(
        response.headers()["access-control-allow-credentials"],
        "true"
    );
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("obleth_preview="), "{cookie}");
    assert!(cookie.contains("Max-Age=3600"), "{cookie}");
    cookie.split(';').next().unwrap().to_owned()
}

/// Redeems a preview link's ticket, as a request from `origin`.
fn enter_request(link: &Value, origin: Option<&str>, method: &str) -> Request<Body> {
    let enter: Uri = link["enter"].as_str().unwrap().parse().unwrap();
    let mut request = Request::builder()
        .method(method)
        .uri(enter.path())
        .header("host", format!("{SLUG}.{DOMAIN}"))
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    request
        .body(Body::from(format!(
            "ticket={}",
            link["ticket"].as_str().unwrap()
        )))
        .unwrap()
}

async fn text(response: axum::response::Response) -> String {
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
}

#[sqlx::test(migrations = "./migrations")]
async fn previews_reach_only_the_pod_still_at_the_recorded_address(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    let (app, _) = app_with(pool.clone(), Some(DOMAIN), Some(kube.clone())).await;
    let admin = login(&app).await;
    let port = serve_app().await;
    let conversation = workspace(&pool, port, "running").await;
    let cookie = enter_preview(&app, &admin, conversation).await;

    // The pod was replaced and its old address may belong to another pod now.
    kube.set_pod(Some("10.0.0.9"));
    let response = app
        .clone()
        .oneshot(preview_request("/", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(text(response).await.contains("not running"));
    assert_eq!(pod_record(&pool).await, ("stopped".into(), None, None));

    // Once the record and the pod agree again, the preview is served, and the
    // confirmation is reused for the next few seconds of requests.
    sqlx::query("UPDATE sandbox_pods SET status='running',pod_ip='127.0.0.1'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sandboxes SET preview_port=$1")
        .bind(i32::from(port))
        .execute(&pool)
        .await
        .unwrap();
    kube.set_pod(Some("127.0.0.1"));
    kube.seen.lock().unwrap().clear();
    for path in ["/", "/app.js"] {
        let response = app
            .clone()
            .oneshot(preview_request(path, Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(text(response).await, "answered by the pod at this address");
    }
    assert_eq!(
        kube.seen(),
        [format!(
            "GET /api/v1/namespaces/nowhere/pods/sbx-{POD_SLUG}"
        )]
    );

    // A deleted pod is noticed the same way once the confirmation is old.
    kube.set_pod(None);
    tokio::time::sleep(Duration::from_millis(5200)).await;
    let response = app
        .clone()
        .oneshot(preview_request("/", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(pod_record(&pool).await, ("stopped".into(), None, None));
}

#[sqlx::test(migrations = "./migrations")]
async fn preview_access_ends_with_the_sign_in(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, _) = app_with(pool.clone(), Some(DOMAIN), Some(kube)).await;
    let admin = login(&app).await;
    let port = serve_app().await;
    let conversation = workspace(&pool, port, "running").await;
    let cookie = enter_preview(&app, &admin, conversation).await;
    let response = app
        .clone()
        .oneshot(preview_request("/", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(req(
            "/api/auth/logout",
            "localhost:8085",
            &admin,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    // A confirmed sign-in is trusted for a few seconds of requests.
    tokio::time::sleep(Duration::from_millis(5200)).await;
    let response = app
        .clone()
        .oneshot(preview_request("/", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // A new sign-in works, and disabling the account ends it as well.
    let admin = login(&app).await;
    let cookie = enter_preview(&app, &admin, conversation).await;
    sqlx::query("UPDATE users SET role='user' WHERE username='test-admin'")
        .execute(&pool)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(preview_request("/", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "workspaces are for administrators here"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn reconciling_stops_records_whose_pod_is_gone(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    let (app, sandboxes) = app_with(pool.clone(), Some(DOMAIN), Some(kube.clone())).await;
    login(&app).await;
    workspace(&pool, 3000, "running").await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(sandboxes.worker(stop.clone()));
    let mut record = pod_record(&pool).await;
    for _ in 0..50 {
        if record.0 == "stopped" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        record = pod_record(&pool).await;
    }
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(record, ("stopped".into(), None, None), "{:?}", kube.seen());
}

#[sqlx::test(migrations = "./migrations")]
async fn stopping_an_idle_pod_does_not_wait_on_its_own_pass(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, sandboxes) = app_with(pool.clone(), Some(DOMAIN), Some(kube.clone())).await;
    login(&app).await;
    workspace(&pool, 3000, "running").await;
    // Idle past the hour allowed: the pass deletes the pod, then finds it gone.
    for table in ["sandbox_pods", "sandboxes"] {
        sqlx::query(&format!(
            "UPDATE {table} SET last_used_at=NOW()-interval '2 hours'"
        ))
        .execute(&pool)
        .await
        .unwrap();
    }
    let stop = CancellationToken::new();
    let worker = tokio::spawn(sandboxes.worker(stop.clone()));
    let mut record = pod_record(&pool).await;
    for _ in 0..50 {
        if record.0 == "stopped" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        record = pod_record(&pool).await;
    }
    stop.cancel();
    let finished = tokio::time::timeout(Duration::from_secs(10), worker).await;
    assert_eq!(record, ("stopped".into(), None, None), "{:?}", kube.seen());
    assert!(
        finished.is_ok(),
        "the pass never finished: {:?}",
        kube.seen()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn downloading_never_wakes_a_workspace(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    let (app, _) = app_with(pool.clone(), Some(DOMAIN), Some(kube.clone())).await;
    let admin = login(&app).await;
    let conversation = workspace(&pool, 3000, "stopped").await;
    let response = app
        .clone()
        .oneshot(req(
            &format!("/api/conversations/{conversation}/workspace/download"),
            "localhost:8085",
            &admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(text(response).await.contains("Open it first"));
    assert_eq!(kube.seen(), Vec::<String>::new());
    assert_eq!(pod_record(&pool).await.0, "stopped");
}

#[sqlx::test(migrations = "./migrations")]
async fn research_runs_stop_when_workspace_access_ends(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, sandboxes) = app_with(pool.clone(), Some(DOMAIN), Some(kube)).await;
    login(&app).await;
    let conversation = workspace(&pool, 3000, "running").await;
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM conversations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (project, research, experiment, run) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    sqlx::query("INSERT INTO projects(id,owner_id,name) VALUES($1,$2,'Study')")
        .bind(project)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO research(id,project_id,owner_id,conversation_id,goal,metric,direction,run_command) VALUES($1,$2,$3,$4,'Faster','loss','min','python train.py')")
        .bind(research).bind(project).bind(owner).bind(conversation)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,title,branch) VALUES($1,$2,'Baseline','exp/baseline')")
        .bind(experiment).bind(research)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status) VALUES($1,$2,$3,1,'abc123','running')")
        .bind(run).bind(research).bind(experiment)
        .execute(&pool).await.unwrap();

    sqlx::query("UPDATE users SET enabled=FALSE WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!obleth_chat::research::check(&pool, &sandboxes, run).await);
    let (status, log): (String, String) =
        sqlx::query_as("SELECT status,log_tail FROM research_runs WHERE id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "lost");
    assert!(log.contains("no longer use workspaces"), "{log}");
}

#[sqlx::test(migrations = "./migrations")]
async fn only_chats_own_page_can_redeem_a_preview_ticket(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, _) = app_with(pool.clone(), Some(DOMAIN), Some(kube)).await;
    let admin = login(&app).await;
    let port = serve_app().await;
    let conversation = workspace(&pool, port, "running").await;
    let response = app
        .clone()
        .oneshot(req(
            &format!("/api/conversations/{conversation}/workspace/preview"),
            "localhost:8085",
            &admin,
            Some(json!({})),
        ))
        .await
        .unwrap();
    let link: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    // A ticket sent to someone else as a link, or posted to the preview by any page
    // but Chat's (another site, another workspace's preview, a page with its origin
    // hidden), opens nothing.
    let lure = format!(
        "/__obleth/enter?ticket={}",
        link["ticket"].as_str().unwrap()
    );
    let response = app
        .clone()
        .oneshot(preview_request(&lure, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get("set-cookie").is_none());
    for (origin, method) in [
        (None, "POST"),
        (Some("https://elsewhere.example"), "POST"),
        (Some("http://bbcdefghijklmnop.sbx.example.com"), "POST"),
        (Some("null"), "POST"),
        (Some(ORIGIN), "GET"),
    ] {
        let response = app
            .clone()
            .oneshot(enter_request(&link, origin, method))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{origin:?} {method}"
        );
        assert!(response.headers().get("set-cookie").is_none(), "{origin:?}");
        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }
    // Chat's page can, and a forged ticket is refused there too.
    let response = app
        .clone()
        .oneshot(enter_request(&link, Some(ORIGIN), "POST"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut forged = link.clone();
    forged["ticket"] = json!("e30.AAAA");
    let response = app
        .clone()
        .oneshot(enter_request(&forged, Some(ORIGIN), "POST"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
    assert!(text(response).await.contains("expired"));
}

#[sqlx::test(migrations = "./migrations")]
async fn other_pages_cannot_send_changes_into_a_preview(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, _) = app_with(pool.clone(), Some(DOMAIN), Some(kube)).await;
    let admin = login(&app).await;
    let port = serve_app().await;
    let conversation = workspace(&pool, port, "running").await;
    let cookie = enter_preview(&app, &admin, conversation).await;
    let post = |headers: &[(&str, &str)]| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/save")
            .header("host", format!("{SLUG}.{DOMAIN}"))
            .header("cookie", &cookie);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.body(Body::from("x=1")).unwrap()
    };
    let own = format!("http://{SLUG}.{DOMAIN}");
    // Another workspace's preview is the same site, so the browser sends the cookie.
    for refused in [
        vec![("sec-fetch-site", "same-site")],
        vec![("sec-fetch-site", "cross-site"), ("origin", own.as_str())],
        vec![("origin", "http://bbcdefghijklmnop.sbx.example.com")],
    ] {
        let response = app.clone().oneshot(post(&refused)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{refused:?}");
    }
    for allowed in [
        vec![("sec-fetch-site", "same-origin"), ("origin", own.as_str())],
        vec![("origin", own.as_str())],
    ] {
        let response = app.clone().oneshot(post(&allowed)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{allowed:?}");
        assert_eq!(text(response).await, "answered by the pod at this address");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_page_load_checks_the_pod_once_and_rides_out_an_api_outage(pool: PgPool) {
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, _) = app_with(pool.clone(), Some(DOMAIN), Some(kube.clone())).await;
    let admin = login(&app).await;
    let port = serve_app().await;
    let conversation = workspace(&pool, port, "running").await;
    let cookie = enter_preview(&app, &admin, conversation).await;
    // Many assets at once share one check of the pod.
    kube.seen.lock().unwrap().clear();
    let loads = (0..40).map(|index| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            app.oneshot(preview_request(
                &format!("/asset-{index}.js"),
                Some(&cookie),
            ))
            .await
            .unwrap()
            .status()
        }
    });
    let statuses = futures_util::future::join_all(loads).await;
    assert!(
        statuses.iter().all(|s| *s == StatusCode::OK),
        "{statuses:?}"
    );
    assert_eq!(kube.seen().len(), 1, "{:?}", kube.seen());
    // While the API server is down, a recent confirmation still serves the preview.
    kube.down.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(5200)).await;
    let response = app
        .clone()
        .oneshot(preview_request("/", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(pod_record(&pool).await.0, "running");
}

/// A verdicts endpoint that keeps each request and answers `yes` for every question.
async fn verdicts_endpoint(yes: f64) -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock = Router::new()
        .route(
            "/v1/verdicts",
            axum::routing::post(
                move |State(seen): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    let verdicts: serde_json::Map<String, Value> = body["questions"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .map(|(id, _)| (id.clone(), json!({"type": "boolean", "value": yes >= 0.5, "probabilities": {"true": yes, "false": 1.0 - yes}, "confidence": 0.7})))
                        .collect();
                    seen.lock().unwrap().push(body);
                    Json(json!({"model": "judge-31b", "verdicts": verdicts, "usage": {"prompt_tokens": 900, "completion_tokens": 1, "total_tokens": 901}}))
                },
            ),
        )
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    (format!("http://{address}"), seen)
}

#[sqlx::test(migrations = "./migrations")]
async fn a_finished_run_is_checked_for_whether_it_tested_its_idea(pool: PgPool) {
    use obleth_chat::{
        auth::{Identity, Role},
        decisions::{Asker, Decisions},
        research::tools::{run, Ctx},
    };
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, sandboxes) = app_with(pool.clone(), Some(DOMAIN), Some(kube)).await;
    login(&app).await;
    let conversation = workspace(&pool, 3000, "running").await;
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM conversations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let sandbox = sandboxes.get(owner, conversation).await.unwrap().unwrap();
    let (project, research, baseline, child) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    sqlx::query("INSERT INTO projects(id,owner_id,name) VALUES($1,$2,'Study')")
        .bind(project)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO research(id,project_id,owner_id,conversation_id,goal,metric,direction,run_command) VALUES($1,$2,$3,$4,'Lower loss','loss','min','python train.py')")
        .bind(research).bind(project).bind(owner).bind(conversation)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,title,branch,frozen) VALUES($1,$2,'Baseline','research/baseline',TRUE)")
        .bind(baseline).bind(research)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,parent_id,title,description,branch,frozen) VALUES($1,$2,$3,'Higher learning rate','Raise lr from 3e-4 to 1e-3','research/lr-1e3',TRUE)")
        .bind(child).bind(research).bind(baseline)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status,metric,reported,finished_at) VALUES($1,$2,$3,1,'aaa','completed',2.5,TRUE,NOW())")
        .bind(Uuid::new_v4()).bind(research).bind(baseline)
        .execute(&pool).await.unwrap();
    let identity = Identity {
        id: owner,
        username: "test-admin".into(),
        display_name: "Test admin".into(),
        role: Role::Admin,
        session: None,
    };
    let decisions = Decisions::new(pool.clone(), None);
    let ctx = Ctx {
        pool: &pool,
        sb: &sandboxes,
        sandbox: &sandbox,
        user: owner,
        conversation,
        project,
        ask: Some(Asker {
            decisions: &decisions,
            gateway: None,
            user: &identity,
        }),
    };
    let mut number = 1;
    let mut finish = |diff: &'static str| {
        number += 1;
        let (id, number) = (Uuid::new_v4(), number);
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status,metric,metrics,log_tail,diff,finished_at) VALUES($1,$2,$3,$4,'bbb','completed',2.4,'{\"loss\":2.4}','config: lr=0.0003\nloss=2.4\n',$5,NOW())")
                .bind(id).bind(research).bind(child).bind(number).bind(diff)
                .execute(&pool).await.unwrap();
            id
        }
    };
    let decision = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<Value>>("SELECT decision FROM research_runs WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    const DIFF: &str = "-lr = 3e-4\n+lr = 1e-3\n";
    let set = |endpoint: &str, url: Option<&str>, check_runs: bool| {
        let pool = pool.clone();
        let (endpoint, url) = (endpoint.to_owned(), url.map(str::to_owned));
        async move {
            sqlx::query("UPDATE decisions SET endpoint=$1, url=$2, check_runs=$3")
                .bind(endpoint)
                .bind(url)
                .bind(check_runs)
                .execute(&pool)
                .await
                .unwrap();
        }
    };

    // The output still shows the old learning rate: the check doubts the run, the
    // model is told to read before trusting it, and the node stays frozen.
    let (url, seen) = verdicts_endpoint(0.1).await;
    set("custom", Some(&format!("{url}/v1/verdicts")), true).await;
    let doubted = finish(DIFF).await;
    let out = run(&ctx, "experiment_wait", "{}").await;
    assert!(out.summary.contains("check doubts it"), "{}", out.summary);
    assert!(out.output.contains("A check by judge-31b doubts this run tested its idea: it gives 10% that it did. Before trusting loss=2.4"), "{}", out.output);
    let found = decision(doubted).await.unwrap();
    assert_eq!(found["model"], "judge-31b");
    assert_eq!(found["tested"], 0.1);
    let asked = seen.lock().unwrap()[0].clone();
    assert_eq!(asked["state"]["idea"]["title"], "Higher learning rate");
    assert_eq!(
        asked["state"]["idea"]["description"],
        "Raise lr from 3e-4 to 1e-3"
    );
    assert_eq!(asked["state"]["change_from_parent"], DIFF);
    assert!(asked["state"]["output"]
        .as_str()
        .unwrap()
        .contains("lr=0.0003"));
    assert_eq!(asked["questions"]["tested"]["type"], "boolean");
    assert_eq!(asked["on_no_answer"], "low_confidence");
    assert!(asked.get("model").is_none());

    // A run the check believes is reported as before.
    let (url, _) = verdicts_endpoint(0.9).await;
    set("custom", Some(&format!("{url}/v1/verdicts")), true).await;
    let trusted = finish(DIFF).await;
    let out = run(&ctx, "experiment_wait", "{}").await;
    assert!(!out.output.contains("doubts"), "{}", out.output);
    assert_eq!(decision(trusted).await.unwrap()["tested"], 0.9);

    // A run with its parent's code has nothing to check.
    let unchanged = finish("").await;
    run(&ctx, "experiment_wait", "{}").await;
    assert_eq!(decision(unchanged).await, None);

    // With no answer the result is reported without a check, and the run says why.
    set("gateway", None, true).await;
    let skipped = finish(DIFF).await;
    let out = run(&ctx, "experiment_wait", "{}").await;
    assert!(
        out.output.contains("The node is now frozen."),
        "{}",
        out.output
    );
    assert_eq!(
        decision(skipped).await.unwrap(),
        json!({"skipped": "Chat has no gateway to ask"})
    );

    // Turned off, nothing is asked or recorded.
    set("custom", Some(&format!("{url}/v1/verdicts")), false).await;
    let unchecked = finish(DIFF).await;
    run(&ctx, "experiment_wait", "{}").await;
    assert_eq!(decision(unchecked).await, None);
}

#[sqlx::test(migrations = "./migrations")]
async fn new_experiments_say_what_they_build_on_and_a_doubted_placement_is_refused_once(
    pool: PgPool,
) {
    use obleth_chat::{
        auth::{Identity, Role},
        decisions::{Asker, Decisions},
        research::tools::{run, Ctx},
    };
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, sandboxes) = app_with(pool.clone(), Some(DOMAIN), Some(kube)).await;
    login(&app).await;
    let conversation = workspace(&pool, 3000, "running").await;
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM conversations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let sandbox = sandboxes.get(owner, conversation).await.unwrap().unwrap();
    let (project, research, baseline, warmup) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    sqlx::query("INSERT INTO projects(id,owner_id,name) VALUES($1,$2,'Study')")
        .bind(project)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO research(id,project_id,owner_id,conversation_id,goal,metric,direction,run_command) VALUES($1,$2,$3,$4,'Lower loss','loss','min','python train.py')")
        .bind(research).bind(project).bind(owner).bind(conversation)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,title,branch,frozen) VALUES($1,$2,'Baseline','research/baseline',TRUE)")
        .bind(baseline).bind(research)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,parent_id,title,description,branch,frozen) VALUES($1,$2,$3,'Warmup 500 steps','Add a 500-step linear warmup','research/warmup-500',TRUE)")
        .bind(warmup).bind(research).bind(baseline)
        .execute(&pool).await.unwrap();
    for (number, node, loss) in [(1, baseline, 2.5), (2, warmup, 2.3)] {
        sqlx::query("INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status,metric,reported,finished_at) VALUES($1,$2,$3,$4,'aaa','completed',$5,TRUE,NOW())")
            .bind(Uuid::new_v4()).bind(research).bind(node).bind(number).bind(loss)
            .execute(&pool).await.unwrap();
    }
    let identity = Identity {
        id: owner,
        username: "test-admin".into(),
        display_name: "Test admin".into(),
        role: Role::Admin,
        session: None,
    };
    let decisions = Decisions::new(pool.clone(), None);
    let ctx = Ctx {
        pool: &pool,
        sb: &sandboxes,
        sandbox: &sandbox,
        user: owner,
        conversation,
        project,
        ask: Some(Asker {
            decisions: &decisions,
            gateway: None,
            user: &identity,
        }),
    };
    let set = |url: String, check_tree: bool| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE decisions SET endpoint='custom', url=$1, check_tree=$2")
                .bind(url)
                .bind(check_tree)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let short = &warmup.simple().to_string()[..8];

    // A child of a measured node has to say what it keeps from it.
    let out = run(
        &ctx,
        "experiment_create",
        &json!({"parent": short, "title": "Cosine decay"}).to_string(),
    )
    .await;
    assert!(!out.ok);
    assert!(
        out.output
            .contains("Say in builds_on what “Warmup 500 steps” established"),
        "{}",
        out.output
    );
    assert!(
        out.output
            .contains("make it a child of “Baseline” [baseline]"),
        "{}",
        out.output
    );

    // An alternative to the parent, filed under it, is refused once with where it belongs.
    let (url, seen) = verdicts_endpoint(0.2).await;
    set(format!("{url}/v1/verdicts"), true).await;
    let args = json!({"parent": short, "title": "Warmup 2000 steps", "description": "A 2000-step warmup instead of 500", "builds_on": "the warmup"});
    let out = run(&ctx, "experiment_create", &args.to_string()).await;
    assert!(!out.ok);
    assert_eq!(out.summary, "Placement doubted");
    assert!(out.output.contains("A check by judge-31b doubts “Warmup 2000 steps” builds on “Warmup 500 steps”: it gives 20% that it fits."), "{}", out.output);
    assert!(
        out.output
            .contains("as a child of “Baseline” [baseline]. Nothing was created."),
        "{}",
        out.output
    );
    assert!(out.output.contains("confirm: true"), "{}", out.output);
    let asked = seen.lock().unwrap()[0].clone();
    assert_eq!(asked["state"]["parent"]["title"], "Warmup 500 steps");
    assert_eq!(asked["state"]["parent"]["result"], "loss=2.3 (run 2)");
    assert_eq!(asked["state"]["new_experiment"]["builds_on"], "the warmup");
    assert_eq!(asked["questions"]["fits"]["type"], "boolean");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM experiments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);

    // Confirmed, it goes on to make the branch (which this test has no pod for).
    let mut confirmed = args.clone();
    confirmed["confirm"] = json!(true);
    let out = run(&ctx, "experiment_create", &confirmed.to_string()).await;
    assert_ne!(out.summary, "Placement doubted");

    // Under the baseline, once a round has a winner, a new decision is pointed to the winner.
    let out = run(
        &ctx,
        "experiment_create",
        &json!({"parent": "baseline", "title": "Bigger batch"}).to_string(),
    )
    .await;
    assert!(
        out.output.contains("“Warmup 500 steps” ["),
        "{}",
        out.output
    );
    assert!(
        out.output
            .contains("has the best result so far (run 2: 2.3)"),
        "{}",
        out.output
    );
    let asked = seen.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        asked["state"]["round_under_baseline"][0]["title"],
        "Warmup 500 steps"
    );
    assert_eq!(asked["state"]["best_so_far"]["result"], "loss=2.3 (run 2)");

    // Turned off, nothing is asked.
    set(format!("{url}/v1/verdicts"), false).await;
    let before = seen.lock().unwrap().len();
    let out = run(&ctx, "experiment_create", &args.to_string()).await;
    assert_ne!(out.summary, "Placement doubted");
    assert_eq!(seen.lock().unwrap().len(), before);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_research_reply_is_checked_against_its_runs(pool: PgPool) {
    use obleth_chat::{
        auth::{Identity, Role},
        decisions::{Asker, Decisions},
        research::{self, report, tools},
    };
    let kube = Arc::new(FakeKube::default());
    kube.set_pod(Some("127.0.0.1"));
    let (app, sandboxes) = app_with(pool.clone(), Some(DOMAIN), Some(kube)).await;
    login(&app).await;
    let conversation = workspace(&pool, 3000, "running").await;
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM conversations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let sandbox = sandboxes.get(owner, conversation).await.unwrap().unwrap();
    let (project, id, baseline, warmup) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    sqlx::query("INSERT INTO projects(id,owner_id,name) VALUES($1,$2,'Study')")
        .bind(project)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO research(id,project_id,owner_id,conversation_id,goal,metric,direction,run_command) VALUES($1,$2,$3,$4,'Lower loss','loss','min','python train.py')")
        .bind(id).bind(project).bind(owner).bind(conversation)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,title,branch,frozen) VALUES($1,$2,'Baseline','research/baseline',TRUE)")
        .bind(baseline).bind(id)
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,parent_id,title,branch,frozen) VALUES($1,$2,$3,'Warmup','research/warmup',TRUE)")
        .bind(warmup).bind(id).bind(baseline)
        .execute(&pool).await.unwrap();
    for (number, node, loss) in [(1, baseline, 2.4113), (2, warmup, 2.3148)] {
        sqlx::query("INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status,metric,metrics,reported,finished_at) VALUES($1,$2,$3,$4,'aaa','completed',$5,$6,TRUE,NOW())")
            .bind(Uuid::new_v4()).bind(id).bind(node).bind(number).bind(loss).bind(json!({"loss": loss, "tok_s": 1500}))
            .execute(&pool).await.unwrap();
    }
    let research = research::for_project(&pool, project)
        .await
        .unwrap()
        .unwrap();
    let identity = Identity {
        id: owner,
        username: "test-admin".into(),
        display_name: "Test admin".into(),
        role: Role::Admin,
        session: None,
    };
    let decisions = Decisions::new(pool.clone(), None);
    let ask = Asker {
        decisions: &decisions,
        gateway: None,
        user: &identity,
    };
    let (url, seen) = verdicts_endpoint(0.1).await;
    sqlx::query("UPDATE decisions SET endpoint='custom', url=$1")
        .bind(format!("{url}/v1/verdicts"))
        .execute(&pool)
        .await
        .unwrap();
    let saved = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<Value>>("SELECT report_check FROM research")
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // A wrong value and a run that doesn't exist are found in code; a cause goes to the check.
    let reply = "Round done.\n\n- Run 2 (warmup) reached loss 2.29, the best so far.\n- Run 1 is the baseline at loss 2.41.\n- Run 7 is still to come.\n\nThe warmup helped because early steps were unstable in run 1.";
    let entry = report::check(&pool, Some(ask), &research, reply)
        .await
        .unwrap();
    assert_eq!(entry["tool"], "report_check");
    assert_eq!(entry["flagged"], true);
    assert_eq!(
        entry["summary"],
        "Checked 4 statements against the runs: 3 don't match"
    );
    let detail = entry["detail"].as_str().unwrap();
    assert!(
        detail.contains("Run 2 printed loss=2.3148, not 2.29."),
        "{detail}"
    );
    assert!(detail.contains("There is no run 7."), "{detail}");
    assert!(
        detail.contains(
            "A check by judge-31b doubts the runs support this: it gives 10% that they do."
        ),
        "{detail}"
    );
    let asked = seen.lock().unwrap()[0].clone();
    assert_eq!(asked["questions"].as_object().unwrap().len(), 1);
    assert!(asked["questions"]["s1"]["instructions"]
        .as_str()
        .unwrap()
        .contains("The warmup helped because"));
    assert_eq!(asked["state"]["runs"][1]["printed"]["loss"], 2.3148);
    let found = saved().await.unwrap();
    assert_eq!(found["problems"].as_array().unwrap().len(), 3);

    // The next research turn is told what to correct.
    let prompt = tools::prompt(&pool, &tools::Standing::Here(research.clone()), &sandbox).await;
    assert!(prompt.contains("## Your last report"), "{prompt}");
    assert!(
        prompt.contains("Run 2 printed loss=2.3148, not 2.29."),
        "{prompt}"
    );

    // A corrected reply clears it; a reply that cites no run asks nothing.
    let entry = report::check(
        &pool,
        Some(ask),
        &research,
        "Run 2 reached loss 2.31 (run 1: 2.41).",
    )
    .await
    .unwrap();
    assert_eq!(entry["flagged"], false);
    assert_eq!(
        entry["summary"],
        "Checked 1 statement against the runs: they match"
    );
    assert_eq!(saved().await, None);
    let before = seen.lock().unwrap().len();
    assert!(
        report::check(&pool, Some(ask), &research, "Shall I start the next round?")
            .await
            .is_none()
    );
    assert_eq!(seen.lock().unwrap().len(), before);

    // With report checks off, values are still checked but nothing is asked.
    sqlx::query("UPDATE decisions SET check_reports=FALSE")
        .execute(&pool)
        .await
        .unwrap();
    let entry = report::check(
        &pool,
        Some(ask),
        &research,
        "Run 2 is better because of the warmup, at loss 2.31.",
    )
    .await
    .unwrap();
    assert_eq!(entry["flagged"], false);
    assert_eq!(seen.lock().unwrap().len(), before);
}
