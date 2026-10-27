//! Identity-scoped catalog, profile ACL, parameter-policy and durable preferences.
//! All tests run against disposable databases and a local discovery fixture.
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig, Identity, Role},
    gateway::Gateway,
    models::{self, ParameterOverrides},
    AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::{
    atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering},
    Arc, LazyLock,
};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
static PEM: LazyLock<String> = LazyLock::new(|| {
    RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(Default::default())
        .unwrap()
        .to_string()
});
#[derive(Clone)]
struct Fake {
    info_status: Arc<AtomicU16>,
    healthy: Arc<AtomicBool>,
    context_window: Arc<AtomicU64>,
    load_status: Arc<AtomicU16>,
}
/// The gateway's `/v1/load`, with an entry of each kind Chat must drop.
async fn load(State(fake): State<Fake>) -> impl IntoResponse {
    (
        StatusCode::from_u16(fake.load_status.load(Ordering::SeqCst)).unwrap(),
        Json(json!({
            "object": "list",
            "scope": "cluster",
            "data": [
                {"id": "alpha", "state": "busy", "slots": 40, "in_flight": 34, "waiting": 0, "you": {"in_flight": 2, "limit": 4}},
                {"id": "team/model", "state": "full", "slots": 8, "in_flight": 8, "waiting": 3},
                {"id": "odd", "state": "melting", "slots": 1, "in_flight": 1, "waiting": 0},
                {"id": "bad\u{7}id", "state": "free", "slots": 1, "in_flight": 0, "waiting": 0},
                {"state": "free"}
            ]
        })),
    )
}
async fn discovery() -> Json<Value> {
    Json(
        json!({"data":[{"id":"alpha","mode":"chat","tags":["general"]},{"id":"team/model","mode":"chat"},{"id":"admin","mode":"chat"},{"id":"disabled","mode":"chat"},{"id":"unpublished","mode":"chat"},{"id":"embedding","mode":"embedding"},{"id":"image","mode":"image_generation"}]}),
    )
}
async fn info(State(fake): State<Fake>) -> impl IntoResponse {
    (
        StatusCode::from_u16(fake.info_status.load(Ordering::SeqCst)).unwrap(),
        Json(
            json!({"data":[{"model_name":"alpha","litellm_params":{"api_key":"DO-NOT-EXPOSE","api_base":"http://internal-provider"},"model_info":{"mode":"chat","context_window":fake.context_window.load(Ordering::SeqCst),"supports_vision":true,"supports_function_calling":false,"healthy":fake.healthy.load(Ordering::SeqCst),"api_key":"ALSO-SECRET","arbitrary":"DROP"}},{"model_name":"phantom","model_info":{"mode":"chat"}}]}),
        ),
    )
}
struct Fixture {
    app: Router,
    state: AppState,
    fake: Fake,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture(pool: PgPool) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway = Gateway::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
        PEM.as_bytes(),
        ORIGIN.into(),
        "obleth".into(),
    )
    .unwrap();
    let fake = Fake {
        info_status: Arc::new(AtomicU16::new(200)),
        healthy: Arc::new(AtomicBool::new(true)),
        context_window: Arc::new(AtomicU64::new(32768)),
        load_status: Arc::new(AtomicU16::new(200)),
    };
    let upstream = Router::new()
        .route("/v1/models", get(discovery))
        .route("/model/info", get(info))
        .route("/v1/load", get(load))
        .with_state(fake.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: ORIGIN.into(),
            session_hours: 12,
            bootstrap_username: None,
            bootstrap_password: None,
            oidc: None,
            login_rate_per_minute: 1000,
            ..AuthConfig::default()
        },
    )
    .await
    .unwrap();
    let state = AppState::new(auth, pool, Some(gateway), 4);
    Fixture {
        app: models::router(state.clone()),
        state,
        fake,
        server,
    }
}
struct Session {
    identity: Identity,
    cookie: String,
    csrf: String,
}
async fn session(pool: &PgPool, role: Role) -> Session {
    let id = Uuid::new_v4();
    let identity = Identity {
        id,
        username: format!("test_{}", id.simple()),
        display_name: "Fixture account".into(),
        role,
        session: None,
    };
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,$3,$4)")
        .bind(id)
        .bind(&identity.username)
        .bind(&identity.display_name)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()));
    sqlx::query("INSERT INTO auth_sessions(token_hash,user_id,expires_at) VALUES($1,$2,NOW()+INTERVAL '1 hour')").bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(pool).await.unwrap();
    let csrf = URL_SAFE_NO_PAD.encode(Sha256::digest(
        format!("obleth-chat-csrf:{token}").as_bytes(),
    ));
    Session {
        identity,
        cookie: format!("obleth_chat_session={token}"),
        csrf,
    }
}
async fn call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("origin", ORIGIN)
                .header("cookie", &user.cookie)
                .header("x-csrf-token", &user.csrf)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn profile(f: &Fixture, admin: &Session, id: &str, mut body: Value) {
    body["name"] = json!(format!("Friendly {id}"));
    let path = format!(
        "/api/admin/models/{}",
        url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
    );
    let (status, value) = call(&f.app, admin, "PUT", &path, body).await;
    assert_eq!(status, StatusCode::OK, "{value}");
}
fn ids(catalog: &Value) -> Vec<String> {
    catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap().into())
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn discovery_filters_nonchat_and_never_exposes_provider_credentials(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let user = session(&pool, Role::User).await;
    let (status, catalog) = call(&f.app, &user, "GET", "/api/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!ids(&catalog).contains(&"embedding".into()));
    assert!(!ids(&catalog).contains(&"image".into()));
    assert!(!ids(&catalog).contains(&"phantom".into()));
    let alpha = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "alpha")
        .unwrap();
    assert_eq!(alpha["metadata"]["contextWindow"], 32768);
    assert_eq!(alpha["metadata"]["supportsVision"], true);
    for forbidden in [
        "DO-NOT-EXPOSE",
        "ALSO-SECRET",
        "internal-provider",
        "api_key",
        "litellm_params",
        "arbitrary",
    ] {
        assert!(!catalog.to_string().contains(forbidden));
    }
    f.fake.info_status.store(404, Ordering::SeqCst);
    assert_eq!(
        call(&f.app, &user, "GET", "/api/models", Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    f.fake.info_status.store(403, Ordering::SeqCst);
    assert_eq!(
        call(&f.app, &user, "GET", "/api/models", Value::Null)
            .await
            .0,
        StatusCode::BAD_GATEWAY
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn model_acl_applies_to_catalog_direct_resolution_auto_and_group_changes(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = session(&pool, Role::Admin).await;
    let alice = session(&pool, Role::User).await;
    let bob = session(&pool, Role::User).await;
    let group = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups(id,name)VALUES($1,'Research')")
        .bind(group)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_group_members(group_id,user_id)VALUES($1,$2)")
        .bind(group)
        .bind(alice.identity.id)
        .execute(&pool)
        .await
        .unwrap();
    profile(
        &f,
        &admin,
        "admin",
        json!({"visibility":"admins","isDefault":true}),
    )
    .await;
    profile(
        &f,
        &admin,
        "team/model",
        json!({"visibility":"groups","groupIds":[group]}),
    )
    .await;
    profile(&f, &admin, "disabled", json!({"enabled":false})).await;
    profile(&f, &admin, "unpublished", json!({"published":false})).await;
    let (_, alice_catalog) = call(&f.app, &alice, "GET", "/api/models", Value::Null).await;
    let (_, bob_catalog) = call(&f.app, &bob, "GET", "/api/models", Value::Null).await;
    assert_eq!(ids(&bob_catalog), vec!["auto", "alpha"]);
    assert!(ids(&alice_catalog).contains(&"team/model".into()));
    assert!(!ids(&alice_catalog).contains(&"admin".into()));
    let gateway = f.state.gateway.as_ref().unwrap();
    assert_eq!(
        models::resolve_request(
            &pool,
            gateway,
            &admin.identity,
            "auto",
            ParameterOverrides::default()
        )
        .await
        .unwrap()
        .model,
        "auto",
        "Auto is the gateway's router, not a local choice"
    );
    assert_eq!(
        models::resolve_request(
            &pool,
            gateway,
            &bob.identity,
            "auto",
            ParameterOverrides::default()
        )
        .await
        .unwrap()
        .model,
        "auto"
    );
    for forbidden in [
        "admin",
        "team/model",
        "disabled",
        "unpublished",
        "embedding",
        "invented",
    ] {
        assert_eq!(
            models::resolve_request(
                &pool,
                gateway,
                &bob.identity,
                forbidden,
                ParameterOverrides::default()
            )
            .await
            .err()
            .unwrap()
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert!(models::resolve_request(
        &pool,
        gateway,
        &alice.identity,
        "team/model",
        ParameterOverrides::default()
    )
    .await
    .is_ok());
    sqlx::query("DELETE FROM user_group_members WHERE user_id=$1")
        .bind(alice.identity.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        models::resolve_request(
            &pool,
            gateway,
            &alice.identity,
            "team/model",
            ParameterOverrides::default()
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&f.app, &bob, "GET", "/api/admin/models", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &f.app,
            &bob,
            "PUT",
            "/api/admin/models/alpha",
            json!({"name":"Attack"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action='model.update' AND actor_id=$1",
    )
    .bind(admin.identity.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit, 4);
    // A caller may have authenticated before an administrator demotes them.
    // Resolution rechecks the live role rather than trusting that stale identity.
    sqlx::query("UPDATE users SET role='user' WHERE id=$1")
        .bind(admin.identity.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        models::resolve_request(
            &pool,
            gateway,
            &admin.identity,
            "admin",
            ParameterOverrides::default()
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn unhealthy_models_are_visible_but_not_selected_and_output_fits_known_context(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = session(&pool, Role::Admin).await;
    let user = session(&pool, Role::User).await;
    profile(&f, &admin, "alpha", json!({"isDefault":true})).await;
    let gateway = f.state.gateway.as_ref().unwrap();
    f.fake.context_window.store(4096, Ordering::SeqCst);
    let too_large = ParameterOverrides {
        max_tokens: Some(8192),
        ..Default::default()
    };
    // A larger request (often a saved preference) is clamped rather than refused.
    assert_eq!(
        models::resolve_request(&pool, gateway, &user.identity, "alpha", too_large)
            .await
            .unwrap()
            .parameters
            .max_tokens,
        4096
    );
    assert!(models::resolve_request(
        &pool,
        gateway,
        &user.identity,
        "alpha",
        ParameterOverrides {
            max_tokens: Some(4096),
            ..Default::default()
        }
    )
    .await
    .is_ok());
    f.fake.healthy.store(false, Ordering::SeqCst);
    let (status, catalog) = call(&f.app, &user, "GET", "/api/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let alpha = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "alpha")
        .unwrap();
    assert_eq!(alpha["available"], false);
    assert_ne!(catalog["autoModelId"], "alpha");
    assert_eq!(
        models::resolve_request(
            &pool,
            gateway,
            &user.identity,
            "alpha",
            ParameterOverrides::default()
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_ne!(
        models::resolve_request(
            &pool,
            gateway,
            &user.identity,
            "auto",
            ParameterOverrides::default()
        )
        .await
        .unwrap()
        .model,
        "alpha"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn profile_defaults_locks_and_auto_obey_personal_overrides_without_bypass(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = session(&pool, Role::Admin).await;
    let user = session(&pool, Role::User).await;
    profile(&f,&admin,"alpha",json!({"defaults":{"systemPrompt":"Managed instructions","temperature":0.1,"maxTokens":4096},"allowedOverrides":["topP","maxTokens","reasoningEffort"],"isDefault":true})).await;
    let (_, untouched) = call(&f.app, &user, "GET", "/api/preferences", Value::Null).await;
    // No output limit unless someone sets one.
    assert_eq!(untouched["preferences"]["maxTokens"], 0);
    assert_eq!(untouched["overrides"], json!({}));
    let (_, appearance) = call(
        &f.app,
        &user,
        "PATCH",
        "/api/preferences",
        json!({"density":"compact"}),
    )
    .await;
    assert_eq!(appearance["overrides"], json!({"density":"compact"}));
    let inherited = models::resolve_request(
        &pool,
        f.state.gateway.as_ref().unwrap(),
        &user.identity,
        "alpha",
        ParameterOverrides::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        inherited.parameters.max_tokens, 4096,
        "synthesized global defaults must not replace model defaults"
    );
    assert_eq!(inherited.parameters.temperature, 0.1);
    let (status,_)=call(&f.app,&user,"PATCH","/api/preferences",json!({"temperature":1.8,"systemPrompt":"Personal instructions","maxTokens":8192,"topP":0.8})).await;
    assert_eq!(status, StatusCode::OK);
    let gateway = f.state.gateway.as_ref().unwrap();
    let resolved = models::resolve_request(
        &pool,
        gateway,
        &user.identity,
        "alpha",
        ParameterOverrides::default(),
    )
    .await
    .unwrap();
    assert_eq!(resolved.model, "alpha");
    // Auto carries no model policy: the person's own preferences apply.
    let auto = models::resolve_request(
        &pool,
        gateway,
        &user.identity,
        "auto",
        ParameterOverrides::default(),
    )
    .await
    .unwrap();
    assert_eq!(auto.model, "auto");
    assert_eq!(auto.parameters.temperature, 1.8);
    assert_eq!(auto.parameters.system_prompt, "Personal instructions");
    assert_eq!(resolved.parameters.temperature, 0.1);
    assert_eq!(resolved.parameters.system_prompt, "Managed instructions");
    assert_eq!(resolved.parameters.max_tokens, 8192);
    assert_eq!(resolved.parameters.top_p, 0.8);
    let rejected = ParameterOverrides {
        temperature: Some(0.9),
        ..Default::default()
    };
    assert_eq!(
        models::resolve_request(&pool, gateway, &user.identity, "alpha", rejected)
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::FORBIDDEN
    );
    let valid = ParameterOverrides {
        temperature: Some(0.1),
        max_tokens: Some(512),
        ..Default::default()
    };
    assert_eq!(
        models::resolve_request(&pool, gateway, &user.identity, "alpha", valid)
            .await
            .unwrap()
            .parameters
            .max_tokens,
        512
    );
    let (_, saved) = call(
        &f.app,
        &user,
        "PATCH",
        "/api/preferences",
        json!({"defaultModel":"team/model"}),
    )
    .await;
    assert_eq!(saved["preferences"]["defaultModel"], "team/model");
    assert_eq!(
        models::resolve_request(
            &pool,
            gateway,
            &user.identity,
            "auto",
            ParameterOverrides::default()
        )
        .await
        .unwrap()
        .model,
        "auto"
    );
    profile(&f, &admin, "team/model", json!({"published":false})).await;
    assert_eq!(
        models::resolve_request(
            &pool,
            gateway,
            &user.identity,
            "auto",
            ParameterOverrides::default()
        )
        .await
        .unwrap()
        .model,
        "auto"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn preferences_and_presets_are_durable_validated_and_owner_scoped(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let alice = session(&pool, Role::User).await;
    let bob = session(&pool, Role::User).await;
    let patch = json!({"defaultModel":"alpha","reasoningEffort":"high","temperature":0.2,"topP":0.7,"maxTokens":4096,"density":"compact","focus":true,"context":false,"favorites":["alpha","alpha"]});
    let (status, saved) = call(&f.app, &alice, "PATCH", "/api/preferences", patch).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["preferences"]["favorites"], json!(["alpha"]));
    let fresh = models::router(f.state.clone());
    let (_, reloaded) = call(&fresh, &alice, "GET", "/api/preferences", Value::Null).await;
    assert_eq!(saved, reloaded);
    let (_, other) = call(&f.app, &bob, "GET", "/api/preferences", Value::Null).await;
    assert_eq!(other["preferences"]["density"], "comfortable");
    assert_eq!(other["preferences"]["defaultModel"], "auto");
    for invalid in [
        json!({"temperature":3}),
        json!({"topP":-1}),
        json!({"maxTokens":-1}),
        json!({"maxTokens":262_145}),
        json!({"reasoningEffort":"max"}),
        json!({"density":"bad"}),
        json!({"unknown":"value"}),
        json!({"systemPrompt":"x".repeat(8 * 1024 + 1)}),
    ] {
        assert_eq!(
            call(&f.app, &alice, "PATCH", "/api/preferences", invalid)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        call(
            &f.app,
            &alice,
            "PATCH",
            "/api/preferences",
            json!({"defaultModel":"imaginary"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, created) = call(
        &f.app,
        &alice,
        "POST",
        "/api/presets",
        json!({"name":"Coding","model":"alpha","parameters":{"temperature":0.3,"maxTokens":2048}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["preset"]["id"].as_str().unwrap();
    let path = format!("/api/presets/{id}");
    assert_eq!(
        call(&f.app, &bob, "DELETE", &path, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            &bob,
            "PATCH",
            &path,
            json!({"name":"Stolen","model":"alpha"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, others) = call(&f.app, &bob, "GET", "/api/presets", Value::Null).await;
    assert_eq!(others["presets"], json!([]));
    let (status, _) = call(
        &f.app,
        &alice,
        "PATCH",
        &path,
        json!({"name":"Careful coding","model":"alpha","parameters":{"reasoningEffort":"high"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(&fresh, &alice, "GET", "/api/presets", Value::Null).await;
    assert_eq!(list["presets"][0]["name"], "Careful coding");
    assert_eq!(
        call(&f.app, &alice, "DELETE", &path, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn model_load_comes_from_the_gateway_and_goes_quiet_when_it_has_none(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let user = session(&pool, Role::User).await;
    let (status, body) = call(&f.app, &user, "GET", "/api/models/load", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({
            "available": true,
            "models": {
                "alpha": {"state": "busy", "slots": 40, "inFlight": 34, "waiting": 0, "you": {"inFlight": 2, "limit": 4}},
                "team/model": {"state": "full", "slots": 8, "inFlight": 8, "waiting": 3}
            }
        })
    );
    // A gateway without the endpoint answers 404; Chat stops asking it for a while.
    f.fake.load_status.store(404, Ordering::SeqCst);
    let (_, body) = call(&f.app, &user, "GET", "/api/models/load", Value::Null).await;
    assert_eq!(body, json!({"available": false}));
    f.fake.load_status.store(200, Ordering::SeqCst);
    let (_, body) = call(&f.app, &user, "GET", "/api/models/load", Value::Null).await;
    assert_eq!(body, json!({"available": false}));
}
