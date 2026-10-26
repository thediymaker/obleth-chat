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
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let url: Uri = body["url"].as_str().unwrap().parse().unwrap();
    let response = app
        .clone()
        .oneshot(preview_request(
            url.path_and_query().unwrap().as_str(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("obleth_preview="), "{cookie}");
    assert!(cookie.contains("Max-Age=3600"), "{cookie}");
    cookie.split(';').next().unwrap().to_owned()
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
