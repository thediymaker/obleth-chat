type OidcCode = (String, String, String, String);
use super::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "test admin password 42";

fn config() -> AuthConfig {
    AuthConfig {
        origin: ORIGIN.into(),
        session_hours: 12,
        bootstrap_username: Some("admin".into()),
        bootstrap_password: Some(PASSWORD.into()),
        oidc: None,
        login_rate_per_minute: 300,
        ..AuthConfig::default()
    }
}

static VECTOR: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

struct Database {
    pool: PgPool,
    admin: PgPool,
    schema: String,
}
impl Database {
    async fn new() -> Option<Self> {
        let Ok(url) = env::var("TEST_DATABASE_URL") else {
            eprintln!("Skipping Postgres auth test: set TEST_DATABASE_URL to run");
            return None;
        };
        let schema = format!("auth_test_{}", Uuid::new_v4().simple());
        let admin = PgPool::connect(&url).await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        // pgvector's types live in public; the extension is made once per database.
        VECTOR
            .get_or_init(|| async {
                sqlx::query("CREATE EXTENSION IF NOT EXISTS vector SCHEMA public")
                    .execute(&admin)
                    .await
                    .unwrap();
            })
            .await;
        let search_path = format!("SET search_path TO {schema}, public");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |conn, _| {
                let statement = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&statement).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        // Every migration, so handlers that touch other features' tables (deleting a
        // group clears its shares, for one) work as they do in production.
        crate::MIGRATOR.run(&pool).await.unwrap();
        Some(Self {
            pool,
            admin,
            schema,
        })
    }
    async fn close(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    origin: Option<&str>,
    cookie: Option<&str>,
    csrf: Option<&str>,
    body: Value,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        request = request.header("x-csrf-token", csrf);
    }
    request = request.header(header::CONTENT_TYPE, "application/json");
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

/// Throttle counters use one-minute windows; start a counter test well inside one.
async fn avoid_minute_boundary() {
    let second = Utc::now().timestamp() % 60;
    if second > 50 {
        tokio::time::sleep(Duration::from_secs(61 - second as u64)).await;
    }
}

fn response_cookie(headers: &HeaderMap) -> String {
    headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn sign_in(app: &Router, username: &str, password: &str) -> (String, String, Value) {
    let (status, headers, body) = call(
        app,
        "POST",
        "/api/auth/login",
        Some(ORIGIN),
        None,
        None,
        json!({"username": username, "password": password}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let csrf = body["csrfToken"].as_str().unwrap().to_owned();
    assert!(headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("HttpOnly; SameSite=Lax"));
    (response_cookie(&headers), csrf, body["user"].clone())
}

#[test]
fn configuration_rejects_insecure_origins_and_ambiguous_cookies() {
    for origin in [
        "http://example.com",
        "https://user:password@example.com",
        "https://example.com/path",
        "https://example.com?next=foo",
        "ftp://localhost",
    ] {
        let mut invalid = config();
        invalid.origin = origin.into();
        assert!(invalid.validated().is_err(), "{origin}");
    }
    for origin in [
        "https://chat.example.com",
        "http://localhost:8085",
        "http://127.0.0.1:8085",
        "http://[::1]:8085",
    ] {
        let mut valid = config();
        valid.origin = origin.into();
        assert!(valid.validated().is_ok(), "{origin}");
    }
    let mut incomplete = config();
    incomplete.bootstrap_password = None;
    assert!(incomplete.validated().is_err());
    let mut weak = config();
    weak.bootstrap_password = Some("short".into());
    assert!(weak.validated().is_err());
    let token = random_token();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("session={token}; session={token}").parse().unwrap(),
    );
    assert!(cookie_value(&headers, "session").is_none());
    headers.insert(header::COOKIE, "session=../bad".parse().unwrap());
    assert!(cookie_value(&headers, "session").is_none());
}

#[test]
fn roles_keep_lowercase_json_and_reject_other_spellings() {
    assert_eq!(json!(Role::Admin), json!("admin"));
    assert_eq!(json!(Role::User), json!("user"));
    assert_eq!(
        serde_json::from_value::<Role>(json!("admin")).unwrap(),
        Role::Admin
    );
    assert!(serde_json::from_value::<Role>(json!("Admin")).is_err());
    assert_eq!(Role::requested(None).unwrap(), None);
    assert_eq!(Role::requested(Some("user")).unwrap(), Some(Role::User));
    for invalid in ["Admin", "owner", "", " admin"] {
        assert_eq!(
            Role::requested(Some(invalid)).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn roles_round_trip_text_and_unknown_values_fail_decoding() {
    let Some(db) = Database::new().await else {
        return;
    };
    for role in [Role::User, Role::Admin] {
        let (text, back): (String, Role) = sqlx::query_as("SELECT $1::text, $1::text")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!((text.as_str(), back), (role.as_str(), role));
    }
    // A value outside the CHECK constraint is a decode error, not a panic.
    let unknown = sqlx::query_scalar::<_, Role>("SELECT 'owner'::text")
        .fetch_one(&db.pool)
        .await;
    assert!(matches!(unknown, Err(sqlx::Error::ColumnDecode { .. })));
    db.close().await;
}

#[tokio::test]
async fn local_auth_origin_csrf_and_account_isolation() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (_, _, body) = call(&app, "GET", "/api/session", None, None, None, Value::Null).await;
    assert_eq!(
        body,
        json!({"authenticated": false, "oidcEnabled": false, "passwordSignIn": true})
    );
    for origin in [None, Some("https://attacker.example"), Some("null")] {
        let (status, _, _) = call(
            &app,
            "POST",
            "/api/auth/login",
            origin,
            None,
            None,
            json!({"username":"admin", "password":PASSWORD}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, _, wrong) = call(
        &app,
        "POST",
        "/api/auth/login",
        Some(ORIGIN),
        None,
        None,
        json!({"username":"admin", "password":"incorrect password"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, unknown) = call(
        &app,
        "POST",
        "/api/auth/login",
        Some(ORIGIN),
        None,
        None,
        json!({"username":"missing-user", "password":"incorrect password"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong, unknown);
    let (cookie, csrf, user) = sign_in(&app, " ADMIN ", PASSWORD).await;
    assert_eq!(user["role"], "admin");
    assert_eq!(user["username"], "admin");
    let (status, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["csrfToken"], csrf);
    for token in [None, Some("wrong-token")] {
        let (status, _, _) = call(
            &app,
            "POST",
            "/api/admin/users",
            Some(ORIGIN),
            Some(&cookie),
            token,
            json!({"username":"student","password":PASSWORD}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, _, created) = call(
        &app,
        "POST",
        "/api/admin/users",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        json!({"username":"student","displayName":"Student One","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["role"], "user");
    assert!(created.get("password_hash").is_none());
    let (status, _, listed) = call(
        &app,
        "GET",
        "/api/admin/users?limit=1",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["hasMore"], true);
    assert_eq!(listed["users"].as_array().unwrap().len(), 1);
    let (_, _, searched) = call(
        &app,
        "GET",
        "/api/admin/users?q=STUDENT&limit=1",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(searched["hasMore"], false);
    assert_eq!(searched["users"][0]["id"], created["id"]);
    let (_, _, literal) = call(
        &app,
        "GET",
        "/api/admin/users?q=%25",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(literal["users"].as_array().unwrap().len(), 0);
    let (status, _, _) = call(
        &app,
        "GET",
        "/api/admin/users?limit=0",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (student_cookie, student_csrf, student) = sign_in(&app, "student", PASSWORD).await;
    assert_ne!(student["id"], user["id"]);
    assert_ne!(student_csrf, csrf);
    let (status, _, _) = call(
        &app,
        "GET",
        "/api/admin/users",
        None,
        Some(&student_cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/auth/logout",
        Some(ORIGIN),
        Some(&student_cookie),
        Some(&csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, headers, _) = call(
        &app,
        "POST",
        "/api/auth/logout",
        Some(ORIGIN),
        Some(&student_cookie),
        Some(&student_csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&student_cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], false);
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], true);
    let persisted: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM auth_sessions LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(persisted.len(), 32);
    assert_ne!(persisted, cookie.as_bytes());
    db.close().await;
}

#[tokio::test]
async fn password_rotation_disable_and_expiry_revoke_sessions() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (admin_cookie, admin_csrf, _) = sign_in(&app, "admin", PASSWORD).await;
    let (status, _, student) = call(
        &app,
        "POST",
        "/api/admin/users",
        Some(ORIGIN),
        Some(&admin_cookie),
        Some(&admin_csrf),
        json!({"username":"student","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let user_id = student["id"].as_str().unwrap();
    let (cookie1, csrf1, _) = sign_in(&app, "student", PASSWORD).await;
    let (cookie2, _, _) = sign_in(&app, "student", PASSWORD).await;
    let (status, headers, changed) = call(
        &app,
        "POST",
        "/api/auth/password",
        Some(ORIGIN),
        Some(&cookie1),
        Some(&csrf1),
        json!({"currentPassword":PASSWORD,"newPassword":"a fresh longer password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rotated_cookie = response_cookie(&headers);
    assert_eq!(changed["authenticated"], true);
    for old in [&cookie1, &cookie2] {
        let (_, _, session) = call(
            &app,
            "GET",
            "/api/session",
            None,
            Some(old),
            None,
            Value::Null,
        )
        .await;
        assert_eq!(session["authenticated"], false);
    }
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/auth/login",
        Some(ORIGIN),
        None,
        None,
        json!({"username":"student","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/admin/users/{user_id}/password"),
        Some(ORIGIN),
        Some(&admin_cookie),
        Some(&admin_csrf),
        json!({"newPassword":"admin reset password 123"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&rotated_cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], false);
    let (cookie3, _, _) = sign_in(&app, "student", "admin reset password 123").await;
    let (status, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/admin/users/{user_id}"),
        Some(ORIGIN),
        Some(&admin_cookie),
        Some(&admin_csrf),
        json!({"enabled":false}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&cookie3),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], false);
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/auth/login",
        Some(ORIGIN),
        None,
        None,
        json!({"username":"student","password":"admin reset password 123"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    sqlx::query("UPDATE auth_sessions SET expires_at = NOW() - INTERVAL '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&admin_cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], false);
    db.close().await;
}

#[tokio::test]
async fn bootstrap_is_once_and_login_limits_are_shared_between_replicas() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (first, second) = tokio::join!(
        Auth::new(db.pool.clone(), config()),
        Auth::new(db.pool.clone(), config())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    let (cookie, csrf, _) = sign_in(&first.router(), "admin", PASSWORD).await;
    let (status, _, _) = call(
        &second.router(),
        "POST",
        "/api/auth/logout",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Exercise shared counters without spending CPU on ten deliberately bad hashes.
    avoid_minute_boundary().await;
    let client = Client("shared-client".into());
    for index in 0..10 {
        let replica = if index % 2 == 0 { &first } else { &second };
        replica
            .throttle("login", &client, Some("shared-user"))
            .await
            .unwrap();
    }
    assert_eq!(
        first
            .throttle("login", &client, Some("shared-user"))
            .await
            .unwrap_err()
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let mut modified = config();
    modified.bootstrap_password = Some("different bootstrap password".into());
    let third = Auth::new(db.pool.clone(), modified).await.unwrap();
    sign_in(&third.router(), "admin", PASSWORD).await;
    db.close().await;
}

#[tokio::test]
async fn simultaneous_admin_disables_preserve_an_enabled_administrator() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (first_cookie, first_csrf, first_user) = sign_in(&app, "admin", PASSWORD).await;
    let (status, _, second_user) = call(
        &app,
        "POST",
        "/api/admin/users",
        Some(ORIGIN),
        Some(&first_cookie),
        Some(&first_csrf),
        json!({"username":"other-admin","password":PASSWORD,"role":"admin"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (second_cookie, second_csrf, _) = sign_in(&app, "other-admin", PASSWORD).await;
    let first_path = format!("/api/admin/users/{}", first_user["id"].as_str().unwrap());
    let second_path = format!("/api/admin/users/{}", second_user["id"].as_str().unwrap());
    let (first, second) = tokio::join!(
        call(
            &app,
            "PATCH",
            &second_path,
            Some(ORIGIN),
            Some(&first_cookie),
            Some(&first_csrf),
            json!({"enabled":false})
        ),
        call(
            &app,
            "PATCH",
            &first_path,
            Some(ORIGIN),
            Some(&second_cookie),
            Some(&second_csrf),
            json!({"enabled":false})
        ),
    );
    assert!(first.0 == StatusCode::NO_CONTENT || second.0 == StatusCode::NO_CONTENT);
    assert_ne!(first.0, second.0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE enabled AND role = 'admin'")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    db.close().await;
}

#[derive(Clone)]
struct FakeIssuer {
    origin: String,
    key: Arc<jsonwebtoken::EncodingKey>,
    jwks: Value,
    codes: Arc<tokio::sync::Mutex<std::collections::HashMap<String, OidcCode>>>,
    /// Refresh tokens the provider would still accept; each works once.
    live: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    /// Access and refresh tokens in the order they were issued.
    access: Arc<tokio::sync::Mutex<Vec<String>>>,
    refresh: Arc<tokio::sync::Mutex<Vec<String>>>,
    /// Refresh tokens presented to the token endpoint, and to the revocation endpoint.
    presented: Arc<tokio::sync::Mutex<Vec<String>>>,
    revoked: Arc<tokio::sync::Mutex<Vec<String>>>,
    /// How refresh requests go: ok, invalid_grant, invalid_client, unavailable,
    /// reuse, which takes a refresh token again as providers with a reuse window do,
    /// or lost, which rotates the token and sends back an answer Chat can't read.
    refresh_mode: Arc<tokio::sync::Mutex<&'static str>>,
    refresh_delay: Arc<std::sync::atomic::AtomicU64>,
    /// This many refresh requests get an answer Chat can't read, without the
    /// provider using the token.
    garble: Arc<std::sync::atomic::AtomicU64>,
}

impl FakeIssuer {
    async fn start() -> (Self, tokio::task::JoinHandle<()>) {
        use rsa::{pkcs8::EncodePrivateKey, traits::PublicKeyParts};
        let private =
            tokio::task::spawn_blocking(|| rsa::RsaPrivateKey::new(&mut OsRng, 2048).unwrap())
                .await
                .unwrap();
        let public = private.to_public_key();
        let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Self {
            origin,
            key: Arc::new(jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap()),
            jwks: json!({"keys":[{"kty":"RSA","use":"sig","alg":"RS256","kid":"test-key","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]}),
            codes: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            live: Default::default(),
            access: Default::default(),
            refresh: Default::default(),
            presented: Default::default(),
            revoked: Default::default(),
            refresh_mode: Arc::new(tokio::sync::Mutex::new("ok")),
            refresh_delay: Default::default(),
            garble: Default::default(),
        };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(|State(s): State<FakeIssuer>| async move {
                // A careless provider may advertise MAC and unsigned ID tokens too.
                Json(json!({"issuer":s.origin,"authorization_endpoint":format!("{}/authorize",s.origin),"token_endpoint":format!("{}/token",s.origin),"jwks_uri":format!("{}/jwks",s.origin),"revocation_endpoint":format!("{}/revoke",s.origin),"response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["RS256","HS256","none"],"token_endpoint_auth_methods_supported":["client_secret_basic"],"code_challenge_methods_supported":["S256"]}))
            }))
            .route("/jwks", get(|State(s): State<FakeIssuer>| async move { Json(s.jwks) }))
            .route("/token", post(fake_token))
            .route("/revoke", post(fake_revoke))
            .with_state(state.clone());
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (state, handle)
    }

    async fn flow(&self, app: &Router, mode: &str, subject: &str) -> (String, String) {
        let (status, headers, _) = call(
            app,
            "GET",
            "/api/auth/oidc/login",
            None,
            None,
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let url = Url::parse(headers[header::LOCATION].to_str().unwrap()).unwrap();
        let query: std::collections::HashMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["redirect_uri"],
            format!("{ORIGIN}/api/auth/oidc/callback")
        );
        let code = random_token();
        self.codes.lock().await.insert(
            code.clone(),
            (
                query["nonce"].clone(),
                query["code_challenge"].clone(),
                mode.into(),
                subject.into(),
            ),
        );
        let path = format!(
            "/api/auth/oidc/callback?code={code}&state={}",
            query["state"]
        );
        (response_cookie(&headers), path)
    }
}

async fn fake_token(
    State(state): State<FakeIssuer>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Response {
    assert_eq!(headers[header::AUTHORIZATION], "Basic Y2hhdDpzZWNyZXQ=");
    if form["grant_type"] == "refresh_token" {
        return fake_refresh(state, &form["refresh_token"]).await;
    }
    assert_eq!(form["grant_type"], "authorization_code");
    let code = form.get("code").unwrap();
    let Some((nonce, challenge, mode, subject)) = state.codes.lock().await.remove(code) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        )
            .into_response();
    };
    assert_eq!(
        URL_SAFE_NO_PAD.encode(digest(&form["code_verifier"])),
        challenge,
        "OIDC code exchange must prove the saved PKCE verifier"
    );
    let now = Utc::now().timestamp();
    let mut claims = json!({"iss":state.origin,"sub":subject,"aud":"chat","iat":now,"exp":now+600,"nonce":nonce,"name":"OIDC Student","preferred_username":"admin","email":"admin@example.com"});
    match mode.as_str() {
        "groups" => {
            claims["groups"] = json!(["students", "admin"]);
            claims["role"] = json!("admin");
        }
        "bad-groups" => claims["groups"] = json!({"admin":true}),
        "nonce" => claims["nonce"] = json!("wrong-nonce"),
        "audience" => claims["aud"] = json!("another-client"),
        "issuer" => claims["iss"] = json!("https://another-issuer.example"),
        "expired" => claims["exp"] = json!(now - 10),
        "future" => claims["iat"] = json!(now + 1000),
        "azp" => claims["azp"] = json!("another-client"),
        "multiple-audiences" => claims["aud"] = json!(["chat", "another-client"]),
        "access-hash" => claims["at_hash"] = json!(URL_SAFE_NO_PAD.encode([0u8; 32])),
        "no-username" => {
            claims.as_object_mut().unwrap().remove("preferred_username");
        }
        name if name.starts_with("username=") => {
            claims["preferred_username"] = json!(&name["username=".len()..])
        }
        _ => (),
    }
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-key".into());
    let mut token = jsonwebtoken::encode(&header, &claims, &state.key).unwrap();
    if mode == "signature" {
        let prefix = token.rsplit_once('.').unwrap().0;
        token = format!("{prefix}.{}", URL_SAFE_NO_PAD.encode([0u8; 256]));
    }
    // Signed with the client secret, which the client also knows.
    if mode == "hs256" {
        token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
    }
    if mode == "none" {
        token = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
    }
    let mut body = state.issue().await;
    body["id_token"] = json!(token);
    Json(body).into_response()
}

impl FakeIssuer {
    /// A new access token and single-use refresh token.
    async fn issue(&self) -> Value {
        let (access, refresh) = (
            format!("at-{}", random_token()),
            format!("rt-{}", random_token()),
        );
        self.access.lock().await.push(access.clone());
        self.refresh.lock().await.push(refresh.clone());
        self.live.lock().await.insert(refresh.clone());
        json!({"access_token":access,"token_type":"Bearer","expires_in":600,"refresh_token":refresh})
    }
}

/// Rotates refresh tokens and refuses one presented twice, as providers that detect
/// theft do.
async fn fake_refresh(state: FakeIssuer, token: &str) -> Response {
    state.presented.lock().await.push(token.to_owned());
    let delay = state
        .refresh_delay
        .load(std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(delay)).await;
    let mode = *state.refresh_mode.lock().await;
    let garbled = || Json(json!({"token_type":"Bearer"})).into_response();
    // Takes one garbled answer if any are left.
    let mut left = state.garble.load(std::sync::atomic::Ordering::SeqCst);
    while left > 0 {
        match state.garble.compare_exchange(
            left,
            left - 1,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ) {
            Ok(_) => return garbled(),
            Err(current) => left = current,
        }
    }
    if mode == "lost" && state.live.lock().await.remove(token) {
        state.issue().await;
        return garbled();
    }
    if mode == "unavailable" {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if mode == "invalid_client" {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic")],
            Json(json!({"error":"invalid_client"})),
        )
            .into_response();
    }
    if mode == "reuse" && state.live.lock().await.contains(token) {
        return Json(state.issue().await).into_response();
    }
    if mode == "invalid_grant" || !state.live.lock().await.remove(token) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        )
            .into_response();
    }
    Json(state.issue().await).into_response()
}

async fn fake_revoke(
    State(state): State<FakeIssuer>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> StatusCode {
    assert_eq!(headers[header::AUTHORIZATION], "Basic Y2hhdDpzZWNyZXQ=");
    state.live.lock().await.remove(&form["token"]);
    state.revoked.lock().await.push(form["token"].clone());
    StatusCode::OK
}

#[tokio::test]
async fn oidc_validates_tokens_pkce_state_replay_and_explicit_provisioning() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let mut cfg = config();
    cfg.oidc = Some(OidcConfig {
        issuer: issuer.origin.clone(),
        client_id: "chat".into(),
        client_secret: "secret".into(),
        allow_signup: true,
        provider_usernames: false,
        scopes: parse_scopes(DEFAULT_OIDC_SCOPES).unwrap(),
    });
    let auth = Auth::new(db.pool.clone(), cfg.clone()).await.unwrap();
    let app = auth.router();
    let (browser, path) = issuer.flow(&app, "valid", "student-subject").await;
    let wrong_browser = format!("obleth_oidc={}", random_token());
    let (status, _, _) = call(
        &app,
        "GET",
        &path,
        None,
        Some(&wrong_browser),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "another browser must not consume the state"
    );
    let (status, headers, _) =
        call(&app, "GET", &path, None, Some(&browser), None, Value::Null).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let session_cookie = response_cookie(&headers);
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&session_cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["user"]["role"], "user");
    assert!(session["user"]["username"]
        .as_str()
        .unwrap()
        .starts_with("oidc_"));
    assert_ne!(
        session["user"]["username"], "admin",
        "IdP profile names cannot link local accounts"
    );
    let lifetime: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (expires_at-created_at))::FLOAT8 FROM auth_sessions LIMIT 1",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(lifetime <= 600.0);
    let (status, _, _) = call(&app, "GET", &path, None, Some(&browser), None, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "callback must be single-use");
    for mode in [
        "nonce",
        "audience",
        "issuer",
        "expired",
        "future",
        "azp",
        "multiple-audiences",
        "access-hash",
        "signature",
        "hs256",
        "none",
    ] {
        let (browser, path) = issuer.flow(&app, mode, "student-subject").await;
        let (status, _, _) =
            call(&app, "GET", &path, None, Some(&browser), None, Value::Null).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "reject invalid {mode}");
    }
    cfg.oidc.as_mut().unwrap().allow_signup = false;
    let restricted = Auth::new(db.pool.clone(), cfg).await.unwrap().router();
    let (browser, path) = issuer
        .flow(&restricted, "valid", "unapproved-subject")
        .await;
    let (status, _, _) = call(
        &restricted,
        "GET",
        &path,
        None,
        Some(&browser),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (admin_cookie, admin_csrf, admin_user) = sign_in(&restricted, "admin", PASSWORD).await;
    let (status, _, _) = call(
        &restricted,
        "POST",
        "/api/admin/oidc-identities",
        Some(ORIGIN),
        Some(&admin_cookie),
        Some(&admin_csrf),
        json!({"userId":admin_user["id"],"subject":"explicitly-approved-subject"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (browser, path) = issuer
        .flow(&restricted, "valid", "explicitly-approved-subject")
        .await;
    let (status, headers, _) = call(
        &restricted,
        "GET",
        &path,
        None,
        Some(&browser),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, _, session) = call(
        &restricted,
        "GET",
        "/api/session",
        None,
        Some(&response_cookie(&headers)),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(
        session["user"]["id"], admin_user["id"],
        "only explicit admin approval may link an OIDC identity"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    // Sign mode keeps no grant, so a failed sign-in has none to revoke.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(issuer.revoked.lock().await.is_empty());
    task.abort();
    db.close().await;
}

async fn managed_user(
    app: &Router,
    cookie: &str,
    csrf: &str,
    username: &str,
    role: &str,
    forced: bool,
) -> Value {
    let (status, _, body) = call(
        app,
        "POST",
        "/api/admin/users",
        Some(ORIGIN),
        Some(cookie),
        Some(csrf),
        json!({"username":username,"password":PASSWORD,"role":role,"forcePasswordChange":forced}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

#[tokio::test]
async fn account_role_changes_revoke_sessions_and_protect_the_last_admin() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (ac, at, admin) = sign_in(&app, "admin", PASSWORD).await;
    let last_path = format!("/api/admin/users/{}", admin["id"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "PATCH",
            &last_path,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let other = managed_user(&app, &ac, &at, "second-admin", "admin", false).await;
    let ordinary = managed_user(&app, &ac, &at, "ordinary", "user", false).await;
    let (bc, bt, _) = sign_in(&app, "second-admin", PASSWORD).await;
    let (uc, ut, _) = sign_in(&app, "ordinary", PASSWORD).await;
    assert_eq!(
        call(
            &app,
            "PATCH",
            &last_path,
            Some(ORIGIN),
            Some(&uc),
            Some(&ut),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &last_path,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"role":"owner"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let path = format!("/api/admin/users/{}", other["id"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "PATCH",
            &path,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"role":"user","displayName":"Renamed colleague"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&bc),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/admin/groups",
            Some(ORIGIN),
            Some(&bc),
            Some(&bt),
            json!({"name":"revoked"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let path = format!("/api/admin/users/{}", ordinary["id"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "PATCH",
            &path,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"role":"admin"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&uc),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    // Self-demotion is permitted only when another enabled admin remains.
    assert_eq!(
        call(
            &app,
            "PATCH",
            &last_path,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/admin/users",
            None,
            Some(&ac),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    db.close().await;
}

#[tokio::test]
async fn concurrent_self_demotions_cannot_remove_every_administrator() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (ac, at, a) = sign_in(&app, "admin", PASSWORD).await;
    let b = managed_user(&app, &ac, &at, "second-admin", "admin", false).await;
    let (bc, bt, _) = sign_in(&app, "second-admin", PASSWORD).await;
    let ap = format!("/api/admin/users/{}", a["id"].as_str().unwrap());
    let bp = format!("/api/admin/users/{}", b["id"].as_str().unwrap());
    let (left, right) = tokio::join!(
        call(
            &app,
            "PATCH",
            &ap,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"role":"user"})
        ),
        call(
            &app,
            "PATCH",
            &bp,
            Some(ORIGIN),
            Some(&bc),
            Some(&bt),
            json!({"role":"user"})
        )
    );
    assert_eq!(
        [left.0, right.0]
            .iter()
            .filter(|s| **s == StatusCode::NO_CONTENT)
            .count(),
        1
    );
    assert_eq!(
        [left.0, right.0]
            .iter()
            .filter(|s| **s == StatusCode::BAD_REQUEST)
            .count(),
        1
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE enabled AND role='admin'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    db.close().await;
}

#[tokio::test]
async fn forced_password_change_blocks_normal_access_until_rotation() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (ac, at, _) = sign_in(&app, "admin", PASSWORD).await;
    let user = managed_user(&app, &ac, &at, "must-change", "user", true).await;
    let (status, headers, body) = call(
        &app,
        "POST",
        "/api/auth/login",
        Some(ORIGIN),
        None,
        None,
        json!({"username":"must-change","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["passwordChangeRequired"], true);
    let cookie = response_cookie(&headers);
    let csrf = body["csrfToken"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/profile",
            Some(ORIGIN),
            Some(&cookie),
            Some(csrf),
            json!({"displayName":"blocked"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/session",
            None,
            Some(&cookie),
            None,
            Value::Null
        )
        .await
        .2["passwordChangeRequired"],
        true
    );
    let (status, headers, body) = call(
        &app,
        "POST",
        "/api/auth/password",
        Some(ORIGIN),
        Some(&cookie),
        Some(csrf),
        json!({"currentPassword":PASSWORD,"newPassword":"a changed strong password 2026"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["passwordChangeRequired"], false);
    let next = response_cookie(&headers);
    let next_csrf = body["csrfToken"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/profile",
            Some(ORIGIN),
            Some(&next),
            Some(next_csrf),
            json!({"displayName":"My new name"})
        )
        .await
        .2["user"]["display_name"],
        "My new name"
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&cookie),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let reset = format!("/api/admin/users/{}/password", user["id"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "POST",
            &reset,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"newPassword":PASSWORD,"forcePasswordChange":true})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&next),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    db.close().await;
}

#[tokio::test]
async fn sessions_are_private_revocable_and_do_not_expose_cookie_hashes() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (ac, at, _) = sign_in(&app, "admin", PASSWORD).await;
    let user = managed_user(&app, &ac, &at, "session-user", "user", false).await;
    managed_user(&app, &ac, &at, "other-user", "user", false).await;
    let (one, one_csrf, _) = sign_in(&app, "session-user", PASSWORD).await;
    let (two, two_csrf, _) = sign_in(&app, "session-user", PASSWORD).await;
    let (other, other_csrf, _) = sign_in(&app, "other-user", PASSWORD).await;
    let (status, _, listed) = call(
        &app,
        "GET",
        "/api/auth/sessions",
        None,
        Some(&one),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list = listed["sessions"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.iter().filter(|s| s["current"] == true).count(), 1);
    assert!(!listed.to_string().contains("token"));
    let own_id = list.iter().find(|s| s["current"] == true).unwrap()["id"]
        .as_str()
        .unwrap();
    let path = format!("/api/auth/sessions/{own_id}");
    assert_eq!(
        call(
            &app,
            "DELETE",
            &path,
            Some(ORIGIN),
            Some(&other),
            Some(&other_csrf),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/auth/sessions/revoke-others",
            Some(ORIGIN),
            Some(&one),
            Some(&one_csrf),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&two),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&one),
            None,
            Value::Null
        )
        .await
        .2["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let reset = format!(
        "/api/admin/users/{}/revoke-sessions",
        user["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &reset,
            Some(ORIGIN),
            Some(&other),
            Some(&other_csrf),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &reset,
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&one),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    // Revoking your current session also expires the browser cookie.
    let (three, three_csrf, _) = sign_in(&app, "session-user", PASSWORD).await;
    let data = call(
        &app,
        "GET",
        "/api/auth/sessions",
        None,
        Some(&three),
        None,
        Value::Null,
    )
    .await
    .2;
    let id = data["sessions"][0]["id"].as_str().unwrap();
    let (status, headers, _) = call(
        &app,
        "DELETE",
        &format!("/api/auth/sessions/{id}"),
        Some(ORIGIN),
        Some(&three),
        Some(&three_csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    let _ = two_csrf;
    db.close().await;
}

#[tokio::test]
async fn groups_membership_and_audit_are_admin_only_and_paginated() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (ac, at, _) = sign_in(&app, "admin", PASSWORD).await;
    let user = managed_user(&app, &ac, &at, "group-user", "user", false).await;
    let (uc, ut, _) = sign_in(&app, "group-user", PASSWORD).await;
    for path in ["/api/admin/groups", "/api/admin/audit"] {
        assert_eq!(
            call(&app, "GET", path, None, Some(&uc), None, Value::Null)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/admin/groups",
            Some(ORIGIN),
            Some(&uc),
            Some(&ut),
            json!({"name":"forbidden"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, _, body) = call(
        &app,
        "POST",
        "/api/admin/groups",
        Some(ORIGIN),
        Some(&ac),
        Some(&at),
        json!({"name":"Classroom","description":"First course"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["group"]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/admin/groups",
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"name":"classroom"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let member = format!(
        "/api/admin/groups/{id}/members/{}",
        user["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &member,
            Some(ORIGIN),
            Some(&uc),
            Some(&ut),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            call(
                &app,
                "PUT",
                &member,
                Some(ORIGIN),
                Some(&ac),
                Some(&at),
                Value::Null
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
    }
    let group = call(
        &app,
        "GET",
        &format!("/api/admin/groups/{id}?limit=1"),
        None,
        Some(&ac),
        None,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(group["group"]["memberCount"], 1);
    assert_eq!(group["members"][0]["id"], user["id"]);
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/admin/groups/{id}"),
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"description":"Updated"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/admin/groups/{id}"),
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_group_members")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let first = call(
        &app,
        "GET",
        "/api/admin/audit?limit=2",
        None,
        Some(&ac),
        None,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(first["events"].as_array().unwrap().len(), 2);
    let cursor = first["nextCursor"].as_str().unwrap();
    let second = call(
        &app,
        "GET",
        &format!("/api/admin/audit?limit=2&before={cursor}"),
        None,
        Some(&ac),
        None,
        Value::Null,
    )
    .await
    .2;
    for a in first["events"].as_array().unwrap() {
        for b in second["events"].as_array().unwrap() {
            assert_ne!(a["id"], b["id"]);
        }
    }
    assert!(!first.to_string().contains(PASSWORD));
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/admin/audit?limit=201",
            None,
            Some(&ac),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    db.close().await;
}

#[tokio::test]
async fn account_links_are_single_use_hashed_expiring_and_bound_to_one_user() {
    let Some(db) = Database::new().await else {
        return;
    };
    let auth = Auth::new(db.pool.clone(), config()).await.unwrap();
    let app = auth.router();
    let (ac, at, _) = sign_in(&app, "admin", PASSWORD).await;
    let (status, headers, invite) = call(
        &app,
        "POST",
        "/api/admin/invitations",
        Some(ORIGIN),
        Some(&ac),
        Some(&at),
        json!({"username":"invited-user","displayName":"Invited person"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let token = invite["url"]
        .as_str()
        .unwrap()
        .split("#activate/")
        .nth(1)
        .unwrap();
    let uid = invite["user"]["id"].as_str().unwrap();
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM account_access_tokens")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(stored, digest(token));
    assert_ne!(stored, token.as_bytes());
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/auth/activate",
            Some("https://attacker.test"),
            None,
            None,
            json!({"token":token,"newPassword":PASSWORD})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (left, right) = tokio::join!(
        call(
            &app,
            "POST",
            "/api/auth/activate",
            Some(ORIGIN),
            None,
            None,
            json!({"token":token,"newPassword":PASSWORD})
        ),
        call(
            &app,
            "POST",
            "/api/auth/activate",
            Some(ORIGIN),
            None,
            None,
            json!({"token":token,"newPassword":PASSWORD})
        )
    );
    assert_eq!(
        [left.0, right.0]
            .iter()
            .filter(|s| **s == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [left.0, right.0]
            .iter()
            .filter(|s| **s == StatusCode::BAD_REQUEST)
            .count(),
        1
    );
    let success = if left.0 == StatusCode::OK {
        left
    } else {
        right
    };
    assert_eq!(success.2["user"]["id"], uid);
    assert_eq!(success.2["passwordChangeRequired"], false);
    let cookie = response_cookie(&success.1);
    let csrf = success.2["csrfToken"].as_str().unwrap();
    let path = format!("/api/admin/users/{uid}/access-link");
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            Some(ORIGIN),
            Some(&cookie),
            Some(csrf),
            json!({"kind":"reset"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let reset = call(
        &app,
        "POST",
        &path,
        Some(ORIGIN),
        Some(&ac),
        Some(&at),
        json!({"kind":"reset"}),
    )
    .await
    .2;
    let expired = reset["url"]
        .as_str()
        .unwrap()
        .split("#activate/")
        .nth(1)
        .unwrap();
    sqlx::query("UPDATE account_access_tokens SET expires_at=NOW()-INTERVAL '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/auth/activate",
            Some(ORIGIN),
            None,
            None,
            json!({"token":expired,"newPassword":PASSWORD})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let reset = call(
        &app,
        "POST",
        &path,
        Some(ORIGIN),
        Some(&ac),
        Some(&at),
        json!({"kind":"reset"}),
    )
    .await
    .2;
    let next = reset["url"]
        .as_str()
        .unwrap()
        .split("#activate/")
        .nth(1)
        .unwrap();
    let (status, _, body) = call(
        &app,
        "POST",
        "/api/auth/activate",
        Some(ORIGIN),
        None,
        None,
        json!({"token":next,"newPassword":"new password from reset link"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["id"], uid);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/auth/sessions",
            None,
            Some(&cookie),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let audit = call(
        &app,
        "GET",
        "/api/admin/audit",
        None,
        Some(&ac),
        None,
        Value::Null,
    )
    .await
    .2
    .to_string();
    for secret in [
        token,
        expired,
        next,
        PASSWORD,
        "new password from reset link",
    ] {
        assert!(!audit.contains(secret));
    }
    db.close().await;
}

#[tokio::test]
async fn oidc_group_mapping_changes_only_managed_memberships_and_never_roles() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let manual = Uuid::new_v4();
    let mapped = Uuid::new_v4();
    for (id, name) in [(manual, "Default course"), (mapped, "SSO students")] {
        sqlx::query("INSERT INTO user_groups(id,name) VALUES($1,$2)")
            .bind(id)
            .bind(name)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let mut cfg = config();
    cfg.oidc = Some(OidcConfig {
        issuer: issuer.origin.clone(),
        client_id: "chat".into(),
        client_secret: "secret".into(),
        allow_signup: true,
        provider_usernames: false,
        scopes: parse_scopes(DEFAULT_OIDC_SCOPES).unwrap(),
    });
    let mut auth = Auth::new(db.pool.clone(), cfg).await.unwrap();
    auth.group_policy = Arc::new(
        group_policy::GroupPolicy::parse(
            Some(&manual.to_string()),
            None,
            Some(&json!({"students":[mapped,manual]}).to_string()),
            true,
        )
        .unwrap(),
    );
    auth.group_policy.validate(&db.pool).await.unwrap();
    let app = auth.router();
    let (browser, path) = issuer.flow(&app, "groups", "group-student").await;
    let (status, headers, _) =
        call(&app, "GET", &path, None, Some(&browser), None, Value::Null).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&response_cookie(&headers)),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["user"]["role"], "user");
    let user: Uuid = serde_json::from_value(session["user"]["id"].clone()).unwrap();
    let rows:Vec<(Uuid,bool)>=sqlx::query_as("SELECT group_id,managed_oidc FROM user_group_members WHERE user_id=$1 ORDER BY managed_oidc").bind(user).fetch_all(&db.pool).await.unwrap();
    assert_eq!(rows, vec![(manual, false), (mapped, true)]);
    let (browser, path) = issuer.flow(&app, "valid", "group-student").await;
    assert_eq!(
        call(&app, "GET", &path, None, Some(&browser), None, Value::Null)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT group_id FROM user_group_members WHERE user_id=$1")
            .bind(user)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(remaining, vec![manual]);
    // Invalid claim shapes fail the entire sign-in without changing groups.
    let (browser, path) = issuer.flow(&app, "bad-groups", "group-student").await;
    assert_eq!(
        call(&app, "GET", &path, None, Some(&browser), None, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    // A manual admin grant wins when this group later disappears from the IdP.
    let (browser, path) = issuer.flow(&app, "groups", "group-student").await;
    assert_eq!(
        call(&app, "GET", &path, None, Some(&browser), None, Value::Null)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let (ac, at, _) = sign_in(&app, "admin", PASSWORD).await;
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/admin/groups/{mapped}/members/{user}"),
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (browser, path) = issuer.flow(&app, "valid", "group-student").await;
    assert_eq!(
        call(&app, "GET", &path, None, Some(&browser), None, Value::Null)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_group_members WHERE user_id=$1 AND NOT managed_oidc",
    )
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    let local = managed_user(&app, &ac, &at, "default-group-user", "user", false).await;
    let local: Uuid = serde_json::from_value(local["id"].clone()).unwrap();
    let group: Uuid =
        sqlx::query_scalar("SELECT group_id FROM user_group_members WHERE user_id=$1")
            .bind(local)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(group, manual);
    // An unredeemed administrator invitation cannot be the only fallback admin.
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/admin/invitations",
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"username":"pending-admin","role":"admin"})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&ac),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!(
                "/api/admin/users/{}",
                session["user"]["id"].as_str().unwrap()
            ),
            Some(ORIGIN),
            Some(&ac),
            Some(&at),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    task.abort();
    db.close().await;
}

#[test]
fn group_policy_rejects_unconfigured_oidc_and_unbounded_mappings() {
    assert!(group_policy::GroupPolicy::parse(None, None, Some("{}"), false).is_err());
    assert!(group_policy::GroupPolicy::parse(Some("not-uuid"), None, None, false).is_err());
    assert!(
        group_policy::GroupPolicy::parse(None, None, Some(r#"{"admins":"admin"}"#), true).is_err()
    );
    assert!(group_policy::GroupPolicy::parse(None, Some(""), Some("{}"), true).is_err());
}

#[test]
fn client_addresses_trust_forwarded_hops_only_from_configured_proxies() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    let forwarded = |values: &[&str]| {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        }
        headers
    };
    let direct = |address: &str| client_key(&[], Some(ip(address)), &HeaderMap::new());
    let trusted = parse_trusted_proxies(Some(" 10.20.0.0/16, fd00::1 ")).unwrap();
    assert!(parse_trusted_proxies(None).unwrap().is_empty());
    for invalid in [
        "10.0.0.0/33",
        "::/129",
        "10.0.0.1/",
        "proxy.example",
        "10.0.0.0/x",
        "::ffff:10.0.0.0/95",
    ] {
        assert!(parse_trusted_proxies(Some(invalid)).is_err(), "{invalid}");
    }
    assert_eq!(
        parse_trusted_proxies(Some("::ffff:10.0.0.0/104")).unwrap(),
        [(ip("10.0.0.0"), 8)]
    );
    let spoofed = forwarded(&["203.0.113.9, 198.51.100.7"]);
    assert_eq!(
        client_key(&[], Some(ip("10.20.1.5")), &spoofed),
        direct("10.20.1.5"),
        "forwarded headers are ignored unless a proxy is configured"
    );
    assert_eq!(
        client_key(&trusted, Some(ip("192.0.2.4")), &spoofed),
        direct("192.0.2.4"),
        "an untrusted peer cannot choose its throttling key"
    );
    assert_eq!(
        client_key(
            &trusted,
            Some(ip("10.20.1.5")),
            &forwarded(&["203.0.113.9, 198.51.100.7", "10.20.3.3"])
        ),
        direct("198.51.100.7"),
        "the nearest untrusted hop is the client; hops to its left are ignored"
    );
    assert_eq!(
        client_key(&trusted, Some(ip("fd00::1")), &forwarded(&["198.51.100.7"])),
        direct("198.51.100.7")
    );
    assert_eq!(
        client_key(
            &trusted,
            Some(ip("10.20.1.5")),
            &forwarded(&["198.51.100.7, unknown"])
        ),
        direct("10.20.1.5"),
        "a malformed hop stops at the last trusted address"
    );
    assert_eq!(direct("::ffff:192.0.2.4"), direct("192.0.2.4"));
    assert_eq!(direct("2001:db8:1:2::a"), direct("2001:db8:1:2:ffff::1"));
    assert_ne!(direct("2001:db8:1:2::a"), direct("2001:db8:1:3::a"));
    assert_ne!(direct("192.0.2.4"), direct("192.0.2.5"));
    assert_eq!(client_key(&trusted, None, &spoofed), "unknown");
}

#[test]
fn loopback_preset_and_client_report_explain_the_chain() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    let forwarded = |value: &[u8]| {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_bytes(value).unwrap());
        headers
    };
    assert_eq!(
        parse_trusted_proxies(Some("Loopback, 192.0.2.10")).unwrap(),
        [
            (ip("127.0.0.0"), 8),
            (ip("::1"), 128),
            (ip("192.0.2.10"), 32)
        ]
    );
    assert!(parse_trusted_proxies(Some("private")).is_err());
    let loopback = parse_trusted_proxies(Some("loopback")).unwrap();
    assert_eq!(
        client_key(
            &loopback,
            Some(ip("::ffff:127.0.0.1")),
            &forwarded(b"198.51.100.7")
        ),
        client_key(&[], Some(ip("198.51.100.7")), &HeaderMap::new())
    );
    let pods = parse_trusted_proxies(Some("10.20.0.0/16")).unwrap();
    let status = |trusted: &[(IpAddr, u8)], peer: &str, headers: &HeaderMap| {
        client_report(trusted, Some(ip(peer)), headers)["status"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let none = HeaderMap::new();
    assert_eq!(status(&[], "198.51.100.7", &none), "direct");
    assert_eq!(status(&[], "172.18.0.1", &none), "local-peer");
    assert_eq!(status(&[], "fd12::1", &none), "local-peer");
    assert_eq!(
        status(&[], "10.20.2.229", &forwarded(b"198.51.100.7")),
        "untrusted-proxy"
    );
    assert_eq!(status(&pods, "10.20.2.229", &none), "missing-header");
    assert_eq!(
        status(&pods, "10.20.2.229", &forwarded(b"198.51.100.7, unknown")),
        "malformed-header"
    );
    assert_eq!(
        status(&pods, "10.20.2.229", &forwarded(b"198.51.100.\x80")),
        "malformed-header"
    );
    assert_eq!(
        status(&pods, "10.20.2.229", &forwarded(b"")),
        "malformed-header"
    );
    assert_eq!(
        status(&pods, "10.20.2.229", &forwarded(b"10.20.2.1")),
        "chain-ends-trusted"
    );
    let report = client_report(
        &pods,
        Some(ip("10.20.2.229")),
        &forwarded(b"203.0.113.9, 198.51.100.7"),
    );
    assert_eq!(report["status"], "forwarded");
    assert_eq!(report["resolved"], "198.51.100.7");
    assert_eq!(report["peer"], "10.20.2.229");
    assert_eq!(
        report["forwardedFor"],
        json!(["203.0.113.9", "198.51.100.7"])
    );
    assert_eq!(client_report(&pods, None, &none)["status"], "unknown");
}

#[tokio::test]
async fn login_limits_are_per_client_and_refund_verified_passwords() {
    let Some(db) = Database::new().await else {
        return;
    };
    let mut cfg = config();
    cfg.login_rate_per_minute = 30;
    let auth = Auth::new(db.pool.clone(), cfg).await.unwrap();
    avoid_minute_boundary().await;
    let attacker = Client("attacker".into());
    let other = Client("other".into());
    let limited = |result: Result<(), ApiError>| {
        result.is_err_and(|error| error.0 == StatusCode::TOO_MANY_REQUESTS)
    };
    let attempts = |key: String| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i32>("SELECT attempts FROM login_throttles WHERE key=$1")
                .bind(key)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let (account_key, other_key) = failure_keys(&other, "admin");
    for _ in 0..3 {
        auth.throttle("login", &other, Some("admin")).await.unwrap();
    }
    // Attempts are counted before any (here skipped) password check.
    for _ in 0..CLIENT_ACCOUNT_FAILURES {
        auth.throttle("login", &attacker, Some("admin"))
            .await
            .unwrap();
    }
    assert!(limited(
        auth.throttle("login", &attacker, Some("admin")).await
    ));
    // A client stopped by its own limit does not spend the all-client backstop.
    assert_eq!(
        attempts(account_key.clone()).await,
        3 + CLIENT_ACCOUNT_FAILURES
    );
    auth.throttle("login", &attacker, Some("another-user"))
        .await
        .unwrap();
    auth.throttle("login", &other, Some("admin")).await.unwrap();
    // Requests without a peer address share the "unknown" client. A verified
    // sign-in refunds only its own attempt, so earlier guesses still count.
    let unknown = Client("unknown".into());
    for _ in 1..CLIENT_ACCOUNT_FAILURES {
        auth.throttle("login", &unknown, Some("admin"))
            .await
            .unwrap();
    }
    sign_in(&auth.router(), "admin", PASSWORD).await;
    assert_eq!(
        attempts(failure_keys(&unknown, "admin").1).await,
        CLIENT_ACCOUNT_FAILURES - 1
    );
    let counted = attempts(account_key.clone()).await;
    assert_eq!(counted, 4 + 2 * CLIENT_ACCOUNT_FAILURES - 1);
    for index in counted..ACCOUNT_FAILURES {
        auth.throttle("login", &Client(format!("bot-{index}")), Some("admin"))
            .await
            .unwrap();
    }
    assert!(limited(auth.throttle("login", &other, Some("admin")).await));
    // The backstop rejection returns the attempt it took from the client limit.
    assert_eq!(attempts(other_key).await, 4);
    assert_eq!(attempts(account_key).await, ACCOUNT_FAILURES);
    auth.throttle("login", &other, Some("another-user"))
        .await
        .unwrap();
    // Each client has its own request budget, and OIDC redirects use another bucket.
    let burst = Client("burst".into());
    for _ in 0..30 {
        auth.throttle("login", &burst, None).await.unwrap();
    }
    assert!(limited(auth.throttle("login", &burst, None).await));
    auth.throttle("oidc", &burst, None).await.unwrap();
    auth.throttle("login", &Client("quiet".into()), None)
        .await
        .unwrap();
    db.close().await;
}

#[tokio::test]
async fn concurrent_attempts_cannot_overrun_the_account_limits() {
    let Some(db) = Database::new().await else {
        return;
    };
    let mut cfg = config();
    cfg.login_rate_per_minute = 1000;
    let auth = Auth::new(db.pool.clone(), cfg).await.unwrap();
    avoid_minute_boundary().await;
    let passed = |clients: Vec<Client>| {
        let auth = auth.clone();
        async move {
            futures_util::future::join_all(clients.iter().map(|client| {
                let auth = auth.clone();
                async move { auth.throttle("login", client, Some("admin")).await.is_ok() }
            }))
            .await
            .into_iter()
            .filter(|ok| *ok)
            .count() as i32
        }
    };
    let one_client = (0..3 * CLIENT_ACCOUNT_FAILURES)
        .map(|_| Client("attacker".into()))
        .collect();
    assert_eq!(passed(one_client).await, CLIENT_ACCOUNT_FAILURES);
    // A few more addresses fill the all-client limit.
    let fillers = (ACCOUNT_FAILURES - CLIENT_ACCOUNT_FAILURES) / CLIENT_ACCOUNT_FAILURES;
    let filling = (0..fillers)
        .flat_map(|index| {
            (0..CLIENT_ACCOUNT_FAILURES).map(move |_| Client(format!("filler-{index}")))
        })
        .collect();
    assert_eq!(
        passed(filling).await,
        ACCOUNT_FAILURES - CLIENT_ACCOUNT_FAILURES
    );
    // A client with no failures gets exactly one attempt past the all-client limit,
    // however many it sends at once.
    let fresh = (0..3 * CLIENT_ACCOUNT_FAILURES)
        .map(|_| Client("fresh".into()))
        .collect();
    assert_eq!(passed(fresh).await, 1);
    // First attempts from new addresses have their own account limit, so rotating
    // through addresses adds only so many guesses a minute...
    let bots = || {
        (0..2 * ACCOUNT_FIRST_TRIES)
            .map(|index| Client(format!("bot-{index}")))
            .collect::<Vec<_>>()
    };
    // (The first attempt of each client above was one too.)
    assert_eq!(passed(bots()).await, ACCOUNT_FIRST_TRIES - fillers - 2);
    // ...and once failing there, none of them gets another.
    assert_eq!(passed(bots()).await, 0);
    db.close().await;
}

#[tokio::test]
async fn failing_clients_cannot_lock_out_a_client_that_is_not_failing() {
    let Some(db) = Database::new().await else {
        return;
    };
    let mut cfg = config();
    cfg.login_rate_per_minute = 1000;
    let auth = Auth::new(db.pool.clone(), cfg).await.unwrap();
    avoid_minute_boundary().await;
    let limited = |result: Result<(), ApiError>| {
        result.is_err_and(|error| error.0 == StatusCode::TOO_MANY_REQUESTS)
    };
    // A few addresses spend the account's whole budget for this minute, each
    // staying under its own per-client limit.
    let per_bot = CLIENT_ACCOUNT_FAILURES / 2;
    let bots = (0..ACCOUNT_FAILURES / per_bot)
        .map(|index| Client(format!("bot-{index}")))
        .collect::<Vec<_>>();
    for bot in &bots {
        for _ in 0..per_bot {
            auth.throttle("login", bot, Some("admin")).await.unwrap();
        }
    }
    // The failing clients are stopped by the account-wide limit...
    for bot in &bots {
        assert!(limited(auth.throttle("login", bot, Some("admin")).await));
    }
    // ...but the real person, from an address that has not been failing, still
    // signs in.
    sign_in(&auth.router(), "admin", PASSWORD).await;
    // A wrong guess from a new address is let through once, then that address is
    // failing too and the account limit applies to it.
    let newcomer = Client("newcomer".into());
    auth.throttle("login", &newcomer, Some("admin"))
        .await
        .unwrap();
    assert!(limited(
        auth.throttle("login", &newcomer, Some("admin")).await
    ));
    // Being stopped keeps it failing: it does not get another free attempt.
    assert!(limited(
        auth.throttle("login", &newcomer, Some("admin")).await
    ));
    // Nor does retrying while stopped restart the quiet time it must wait out.
    let key = failing_key(&newcomer, "admin");
    let window = |pool: PgPool, key: String| async move {
        sqlx::query_scalar::<_, i64>("SELECT window_start FROM login_throttles WHERE key=$1")
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    sqlx::query("UPDATE login_throttles SET window_start = window_start - 3 WHERE key=$1")
        .bind(&key)
        .execute(&db.pool)
        .await
        .unwrap();
    let quiet_since = window(db.pool.clone(), key.clone()).await;
    for _ in 0..3 {
        assert!(limited(
            auth.throttle("login", &newcomer, Some("admin")).await
        ));
    }
    assert_eq!(window(db.pool.clone(), key).await, quiet_since);
    db.close().await;
}

#[tokio::test]
async fn access_links_cannot_add_passwords_after_oidc_linking_and_replace_the_browser_session() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let mut cfg = config();
    cfg.oidc = Some(OidcConfig {
        issuer: issuer.origin.clone(),
        client_id: "chat".into(),
        client_secret: "secret".into(),
        allow_signup: false,
        provider_usernames: false,
        scopes: parse_scopes(DEFAULT_OIDC_SCOPES).unwrap(),
    });
    let auth = Auth::new(db.pool.clone(), cfg).await.unwrap();
    let app = auth.router();
    let (ac, at, _) = sign_in(&app, "admin", PASSWORD).await;
    let invite = |username: &'static str| {
        let (app, ac, at) = (app.clone(), ac.clone(), at.clone());
        async move {
            let (status, _, body) = call(
                &app,
                "POST",
                "/api/admin/invitations",
                Some(ORIGIN),
                Some(&ac),
                Some(&at),
                json!({"username":username}),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            let token = body["url"].as_str().unwrap().split("#activate/").nth(1);
            (
                token.unwrap().to_owned(),
                Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap(),
            )
        }
    };
    let activate = |token: String, cookie: Option<String>| {
        let app = app.clone();
        async move {
            call(
                &app,
                "POST",
                "/api/auth/activate",
                Some(ORIGIN),
                cookie.as_deref(),
                None,
                json!({"token":token,"newPassword":"invited user password"}),
            )
            .await
            .0
        }
    };
    let local_password = |id: Uuid| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT password_hash IS NOT NULL FROM users WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    let (token, linked) = invite("linked-user").await;
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/admin/oidc-identities",
        Some(ORIGIN),
        Some(&ac),
        Some(&at),
        json!({"userId":linked,"subject":"linked-subject"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM account_access_tokens WHERE user_id=$1")
            .bind(linked)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(pending, 0, "linking OIDC revokes pending account links");
    assert_eq!(activate(token, None).await, StatusCode::BAD_REQUEST);
    assert!(!local_password(linked).await);

    // Redemption rechecks the account even if a link survives an identity change.
    let (token, direct) = invite("direct-user").await;
    sqlx::query(
        "INSERT INTO oidc_identities (issuer, subject, user_id) VALUES ($1, 'direct-subject', $2)",
    )
    .bind(&issuer.origin)
    .bind(direct)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(activate(token, None).await, StatusCode::BAD_REQUEST);
    assert!(!local_password(direct).await);

    let (token, invited) = invite("browser-user").await;
    let (previous, _, _) = sign_in(&app, "admin", PASSWORD).await;
    assert_eq!(
        activate(token, Some(previous.clone())).await,
        StatusCode::OK
    );
    assert!(local_password(invited).await);
    let sessions = |cookie: String| {
        let app = app.clone();
        async move {
            call(
                &app,
                "GET",
                "/api/auth/sessions",
                None,
                Some(&cookie),
                None,
                Value::Null,
            )
            .await
            .0
        }
    };
    assert_eq!(
        sessions(previous).await,
        StatusCode::UNAUTHORIZED,
        "activation revokes the session the browser presented"
    );
    assert_eq!(sessions(ac).await, StatusCode::OK);
    task.abort();
    db.close().await;
}

fn forward_config(issuer: &FakeIssuer) -> AuthConfig {
    let mut cfg = config();
    cfg.oidc = Some(OidcConfig {
        issuer: issuer.origin.clone(),
        client_id: "chat".into(),
        client_secret: "secret".into(),
        allow_signup: true,
        provider_usernames: false,
        scopes: parse_scopes("openid profile offline_access").unwrap(),
    });
    cfg.forward_tokens = true;
    cfg.secret_key = Some(SecretKey::from_bytes([9u8; 32]));
    cfg
}

#[tokio::test]
async fn oidc_usernames_follow_the_provider_only_when_asked_and_free() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let mut cfg = config();
    cfg.oidc = Some(OidcConfig {
        issuer: issuer.origin.clone(),
        client_id: "chat".into(),
        client_secret: "secret".into(),
        allow_signup: true,
        provider_usernames: false,
        scopes: parse_scopes(DEFAULT_OIDC_SCOPES).unwrap(),
    });
    let generated_only = Auth::new(db.pool.clone(), cfg.clone())
        .await
        .unwrap()
        .router();
    cfg.oidc.as_mut().unwrap().provider_usernames = true;
    let app = Auth::new(db.pool.clone(), cfg).await.unwrap().router();
    // Off by default: the provider's username is not trusted to name accounts.
    let frank = oidc_user(&generated_only, &issuer, "username=frank", "frank-subject").await;
    assert!(frank.username.starts_with("oidc_"), "{}", frank.username);
    let frank = oidc_user(&app, &issuer, "username=frank", "frank-subject").await;
    assert_eq!(
        frank.username, "frank",
        "a generated name takes the provider's once on"
    );

    let carol = oidc_user(&app, &issuer, "username=Carol", "carol-subject").await;
    assert_eq!(
        carol.username, "carol",
        "a free provider username names the account"
    );
    assert_eq!(carol.display_name, "OIDC Student");
    for _ in 0..2 {
        // Taken: falls back, and a later sign-in still works and leaves it alone.
        let other = oidc_user(&app, &issuer, "username=carol", "other-subject").await;
        assert!(other.username.starts_with("oidc_"), "{}", other.username);
    }
    for (mode, subject) in [
        ("valid", "admin-name-subject"),
        ("username=not a name!", "invalid-subject"),
        ("username=oidc_0123456789abcdef", "reserved-subject"),
        ("no-username", "unnamed-subject"),
    ] {
        let user = oidc_user(&app, &issuer, mode, subject).await;
        assert!(
            user.username.starts_with("oidc_"),
            "{mode}: {}",
            user.username
        );
    }
    // The provider renames carol: Chat follows, and the old name is free again.
    let caroline = oidc_user(&app, &issuer, "username=caroline", "carol-subject").await;
    assert_eq!(
        (caroline.id, caroline.username.as_str()),
        (carol.id, "caroline")
    );
    let other = oidc_user(&app, &issuer, "username=carol", "other-subject").await;
    assert_eq!(
        other.username, "carol",
        "the account on its generated name takes it"
    );
    // The provider gives carol-subject a name another account holds: rather than keep
    // showing a stale name, the account goes back to a generated one.
    let back = oidc_user(&app, &issuer, "username=carol", "carol-subject").await;
    assert_eq!(back.id, carol.id);
    assert!(back.username.starts_with("oidc_"), "{}", back.username);
    // A name Chat did not take from the provider is never changed.
    sqlx::query(
        "UPDATE users SET username = 'chosen', username_from_provider = FALSE WHERE id = $1",
    )
    .bind(frank.id)
    .execute(&db.pool)
    .await
    .unwrap();
    let kept = oidc_user(&app, &issuer, "username=zed", "frank-subject").await;
    assert_eq!((kept.id, kept.username.as_str()), (frank.id, "chosen"));
    // The provider stops sending a usable name: the name it gave is released.
    let named = oidc_user(&app, &issuer, "username=yara", "yara-subject").await;
    assert_eq!(named.username, "yara");
    let unnamed = oidc_user(&app, &issuer, "no-username", "yara-subject").await;
    assert!(
        unnamed.username.starts_with("oidc_"),
        "{}",
        unnamed.username
    );
    // An account with two identities at the provider keeps its name instead of
    // swapping between theirs.
    let named = oidc_user(&app, &issuer, "username=yara", "yara-subject").await;
    assert_eq!(named.username, "yara");
    sqlx::query(
        "INSERT INTO oidc_identities (issuer, subject, user_id) VALUES ($1, 'second-yara', $2)",
    )
    .bind(&issuer.origin)
    .bind(named.id)
    .execute(&db.pool)
    .await
    .unwrap();
    let second = oidc_user(&app, &issuer, "username=yara2", "second-yara").await;
    assert_eq!((second.id, second.username.as_str()), (named.id, "yara"));
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn a_username_taken_during_a_rename_leaves_the_sign_in_usable() {
    let Some(db) = Database::new().await else {
        return;
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, role) VALUES ($1, $2, 'Renamed', 'user')",
    )
    .bind(id)
    .bind(format!("oidc_{}", id.simple()))
    .execute(&db.pool)
    .await
    .unwrap();
    // Another transaction holds the name without committing yet, so the rename's
    // NOT EXISTS sees nothing and its UPDATE waits on the unique index.
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, role) VALUES ($1, 'zed', 'Zed', 'user')",
    )
    .bind(Uuid::new_v4())
    .execute(&mut *blocker)
    .await
    .unwrap();
    let pool = db.pool.clone();
    let rename = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        let renamed = oidc::take_username(&mut tx, id, "zed", true).await.unwrap();
        let usable: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (renamed, usable)
    });
    // Release the name only once the rename is waiting on it.
    let mut waiting = false;
    for _ in 0..100 {
        waiting = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_locks WHERE NOT granted)")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        if waiting {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(waiting, "the rename must be waiting on the held name");
    blocker.commit().await.unwrap();
    let (renamed, usable) = rename.await.unwrap();
    assert_eq!((renamed, usable), (None, 1));
    let (username, from_provider): (String, bool) =
        sqlx::query_as("SELECT username, username_from_provider FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        (username, from_provider),
        (format!("oidc_{}", id.simple()), false)
    );
    db.close().await;
}

/// Signs in through the provider with the given mode and returns the person.
async fn oidc_user(app: &Router, issuer: &FakeIssuer, mode: &str, subject: &str) -> Identity {
    let (browser, path) = issuer.flow(app, mode, subject).await;
    let (status, headers, _) =
        call(app, "GET", &path, None, Some(&browser), None, Value::Null).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{mode}");
    let (_, _, session) = call(
        app,
        "GET",
        "/api/session",
        None,
        Some(&response_cookie(&headers)),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], true, "{session}");
    serde_json::from_value(session["user"].clone()).unwrap()
}

/// Signs in through the provider and returns the session cookie, its CSRF token and
/// the person.
async fn oidc_session(
    app: &Router,
    issuer: &FakeIssuer,
    subject: &str,
) -> (String, String, Identity) {
    let (browser, path) = issuer.flow(app, "valid", subject).await;
    let (status, headers, _) =
        call(app, "GET", &path, None, Some(&browser), None, Value::Null).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let cookie = response_cookie(&headers);
    let (_, _, session) = call(
        app,
        "GET",
        "/api/session",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], true, "{session}");
    let csrf = session["csrfToken"].as_str().unwrap().to_owned();
    (
        cookie,
        csrf,
        serde_json::from_value(session["user"].clone()).unwrap(),
    )
}

/// A gateway that lists one model, fails chats, and records each Authorization.
async fn fake_gateway() -> (
    Url,
    Arc<tokio::sync::Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let seen: Arc<tokio::sync::Mutex<Vec<String>>> = Arc::default();
    let record = |seen: Arc<tokio::sync::Mutex<Vec<String>>>, headers: HeaderMap| async move {
        let value = headers[header::AUTHORIZATION].to_str().unwrap().to_owned();
        seen.lock().await.push(value);
    };
    let app = Router::new()
        .route(
            "/v1/models",
            get(
                move |State(seen): State<Arc<tokio::sync::Mutex<Vec<String>>>>,
                      headers: HeaderMap| async move {
                    record(seen, headers).await;
                    Json(json!({"data":[{"id":"fixture-model"}]}))
                },
            ),
        )
        .route(
            "/v1/chat/completions",
            post(
                move |State(seen): State<Arc<tokio::sync::Mutex<Vec<String>>>>,
                      headers: HeaderMap| async move {
                    record(seen, headers).await;
                    StatusCode::INTERNAL_SERVER_ERROR
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen, task)
}

async fn signing_key() -> String {
    use rsa::pkcs8::EncodePrivateKey;
    tokio::task::spawn_blocking(|| {
        rsa::RsaPrivateKey::new(&mut OsRng, 2048)
            .unwrap()
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .to_string()
    })
    .await
    .unwrap()
}

async fn expire_soon(pool: &PgPool, filter: &str) {
    sqlx::query(&format!(
        "UPDATE oidc_grants SET access_expires_at = NOW() + INTERVAL '30 seconds' {filter}"
    ))
    .execute(pool)
    .await
    .unwrap();
}

#[test]
fn forwarding_and_passwords_off_need_their_settings() {
    let oidc = || OidcConfig {
        issuer: "https://id.example.com".into(),
        client_id: "chat".into(),
        client_secret: "secret".into(),
        allow_signup: false,
        provider_usernames: false,
        scopes: parse_scopes("profile offline_access").unwrap(),
    };
    assert_eq!(oidc().scopes, ["openid", "profile", "offline_access"]);
    assert!(parse_scopes("openid \"quoted\"").is_err());
    let mut forward = config();
    forward.forward_tokens = true;
    forward.oidc = Some(oidc());
    assert!(forward.clone().validated().is_err(), "needs a secret key");
    forward.secret_key = Some(SecretKey::from_bytes([1u8; 32]));
    forward.session_hours = 2160;
    assert!(forward.clone().validated().is_ok());
    forward.oidc.as_mut().unwrap().scopes = parse_scopes("profile").unwrap();
    assert!(forward.clone().validated().is_err(), "needs offline_access");
    forward.oidc = None;
    assert!(forward.validated().is_err(), "needs OIDC");
    let mut long = config();
    long.session_hours = 2160;
    assert!(
        long.validated().is_err(),
        "long sessions only with forwarded tokens"
    );

    let mut closed = config();
    closed.password_sign_in = false;
    assert!(closed.clone().validated().is_err(), "needs OIDC");
    closed.oidc = Some(oidc());
    assert!(
        closed.clone().validated().is_err(),
        "a bootstrap password is refused, not ignored"
    );
    closed.bootstrap_username = None;
    closed.bootstrap_password = None;
    assert!(closed.validated().is_ok());
}

#[tokio::test]
async fn forwarded_provider_tokens_reach_the_gateway_and_rotate_before_expiry() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let mut cfg = forward_config(&issuer);
    cfg.session_hours = 2160;
    let auth = Auth::new(db.pool.clone(), cfg).await.unwrap();
    let app = auth.router();
    let (_, _, user) = oidc_session(&app, &issuer, "student").await;
    // The grant decides how long the sign-in lasts, not the ID token's 10 minutes.
    let lifetime = |user: Uuid| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, f64>(
                "SELECT EXTRACT(EPOCH FROM (expires_at-created_at))::FLOAT8 FROM auth_sessions WHERE user_id=$1",
            )
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let long = lifetime(user.id).await;
    assert!(long > 2000.0 * 3600.0, "{long}");
    let access = issuer.access.lock().await.clone();
    let refresh = issuer.refresh.lock().await.clone();
    let (sealed_access, sealed_refresh): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT access_token, refresh_token FROM oidc_grants WHERE user_id=$1")
            .bind(user.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(!String::from_utf8_lossy(&sealed_access).contains(&access[0]));
    assert!(!String::from_utf8_lossy(&sealed_refresh).contains(&refresh[0]));

    let (base, seen, gateway_task) = fake_gateway().await;
    let pem = signing_key().await;
    let gateway =
        crate::gateway::Gateway::new(base.clone(), pem.as_bytes(), ORIGIN.into(), "obleth".into())
            .unwrap()
            .forward_tokens(auth.grants().unwrap());
    let last = || async { seen.lock().await.last().cloned().unwrap() };
    gateway.models(&user).await.unwrap();
    assert_eq!(last().await, format!("Bearer {}", access[0]));

    // A local account still gets Chat's signed token.
    let (_, _, admin) = sign_in(&app, "admin", PASSWORD).await;
    let admin: Identity = serde_json::from_value(admin).unwrap();
    // A password session has no grant to end it, so it stays within the old cap.
    let short = lifetime(admin.id).await;
    assert!(short <= 168.0 * 3600.0 + 1.0, "{short}");
    gateway.models(&admin).await.unwrap();
    let signed = last().await;
    assert!(signed.starts_with("Bearer ey") && !access.iter().any(|a| signed.ends_with(a)));

    // Near expiry the token is refreshed first, and the refresh token rotates.
    expire_soon(&db.pool, "").await;
    gateway.models(&user).await.unwrap();
    expire_soon(&db.pool, "").await;
    gateway.models(&user).await.unwrap();
    gateway.models(&user).await.unwrap();
    let access = issuer.access.lock().await.clone();
    let refresh = issuer.refresh.lock().await.clone();
    assert_eq!(
        *issuer.presented.lock().await,
        [refresh[0].clone(), refresh[1].clone()]
    );
    assert_eq!(last().await, format!("Bearer {}", access[2]));

    // Without a signing key, local accounts can't reach the models at all.
    let forwarding = crate::gateway::Gateway::forwarding(base)
        .unwrap()
        .forward_tokens(auth.grants().unwrap());
    assert_eq!(
        forwarding.models(&admin).await.unwrap_err().0,
        StatusCode::FORBIDDEN
    );
    forwarding.models(&user).await.unwrap();
    assert_eq!(last().await, format!("Bearer {}", access[2]));

    // Someone with an identity never falls back to Chat's token.
    let calls = seen.lock().await.len();
    sqlx::query("DELETE FROM auth_sessions WHERE user_id=$1")
        .bind(user.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let error = gateway.models(&user).await.unwrap_err();
    assert_eq!(
        (error.0, error.1.as_str()),
        (StatusCode::UNAUTHORIZED, grants::SIGN_IN_AGAIN)
    );
    assert_eq!(seen.lock().await.len(), calls);
    gateway_task.abort();
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn two_replicas_present_a_refresh_token_only_once() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let first = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let second = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let (_, _, user) = oidc_session(&first.router(), &issuer, "student").await;
    expire(&db.pool, "1 second").await;
    // Both start before either finishes; with the access token expired, the one
    // that didn't claim the refresh waits for it.
    issuer
        .refresh_delay
        .store(300, std::sync::atomic::Ordering::SeqCst);
    let (one, two) = (first.grants().unwrap(), second.grants().unwrap());
    let (a, b) = tokio::join!(
        one.access_token(user.id, None),
        two.access_token(user.id, None)
    );
    let header = |token: Option<grants::ProviderToken>| token.unwrap().authorization().unwrap();
    let (a, b) = (header(a.unwrap()), header(b.unwrap()));
    let access = issuer.access.lock().await.clone();
    assert_eq!(a, b);
    assert_eq!(a, format!("Bearer {}", access[1]).as_str());
    assert_eq!(
        *issuer.presented.lock().await,
        [issuer.refresh.lock().await[0].clone()]
    );

    // While the access token still lasts, the other uses it instead of waiting.
    expire_soon(&db.pool, "").await;
    let started = std::time::Instant::now();
    let (a, b) = tokio::join!(one.access_token(user.id, None), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let token = two.access_token(user.id, None).await;
        (token, started.elapsed())
    });
    let (b, waited) = b;
    let access = issuer.access.lock().await.clone();
    assert_eq!(header(a.unwrap()), format!("Bearer {}", access[2]).as_str());
    assert_eq!(header(b.unwrap()), format!("Bearer {}", access[1]).as_str());
    assert!(waited < Duration::from_millis(300), "{waited:?}");
    assert_eq!(issuer.presented.lock().await.len(), 2);
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn a_refused_grant_ends_its_sign_in_and_tokens_then_another_takes_over() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let grants = auth.grants().unwrap();
    let (_, _, user) = oidc_session(&app, &issuer, "student").await;
    let (laptop, laptop_csrf, _) = oidc_session(&app, &issuer, "student").await;
    let (status, _, made) = call(
        &app,
        "POST",
        "/api/auth/tokens",
        Some(ORIGIN),
        Some(&laptop),
        Some(&laptop_csrf),
        json!({"name":"laptop","days":1}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let (newest, newest_session): (Uuid, Uuid) =
        sqlx::query_as("SELECT id, session_id FROM oidc_grants ORDER BY refreshed_at DESC LIMIT 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    expire_soon(&db.pool, &format!("WHERE id='{newest}'")).await;

    // A provider that can't answer keeps the grant, and its access token serves
    // until it expires; then there is none, and the provider isn't asked again yet.
    *issuer.refresh_mode.lock().await = "unavailable";
    let token = grants.access_token(user.id, None).await.unwrap().unwrap();
    let access = issuer.access.lock().await.clone();
    assert_eq!(
        token.authorization().unwrap(),
        format!("Bearer {}", access[1]).as_str()
    );
    sqlx::query(
        "UPDATE oidc_grants SET access_expires_at = NOW() - INTERVAL '1 second' WHERE id=$1",
    )
    .bind(newest)
    .execute(&db.pool)
    .await
    .unwrap();
    let Err(error) = grants.access_token(user.id, None).await else {
        panic!("an unavailable provider gives no token");
    };
    assert_eq!(error.0, StatusCode::BAD_GATEWAY);
    assert_eq!(issuer.presented.lock().await.len(), 1);
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM oidc_grants")
            .fetch_one(&db.pool)
            .await
            .unwrap()
    };
    assert_eq!(count().await, 2);

    // A refusal ends that sign-in and the token made in it; the other grant serves.
    retry_now(&db.pool).await;
    *issuer.refresh_mode.lock().await = "invalid_grant";
    let token = grants.access_token(user.id, None).await.unwrap().unwrap();
    let access = issuer.access.lock().await.clone();
    assert_eq!(
        token.authorization().unwrap(),
        format!("Bearer {}", access[0]).as_str()
    );
    assert_eq!(count().await, 1);
    let ended: bool =
        sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM auth_sessions WHERE id=$1)")
            .bind(newest_session)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(ended);
    let revoked: bool = sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM api_tokens")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(revoked, "the token made in the ended sign-in ends with it");
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_revocations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(queued, 0, "a refused token needs no revocation");
    let audited: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action='sign_in_ended'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(audited, 1);
    let (_, _, session) = call(
        &app,
        "GET",
        "/api/session",
        None,
        Some(&laptop),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(session["authenticated"], false);

    // With no grant left, the person must sign in again.
    expire_soon(&db.pool, "").await;
    let Err(error) = grants.access_token(user.id, None).await else {
        panic!("no grant is left");
    };
    assert_eq!(
        (error.0, error.1.as_str()),
        (StatusCode::UNAUTHORIZED, grants::SIGN_IN_AGAIN)
    );
    assert_eq!(count().await, 0);
    task.abort();
    db.close().await;
}

/// As if the pause after a failed refresh had passed.
async fn retry_now(pool: &PgPool) {
    sqlx::query("UPDATE oidc_grants SET refresh_failed_at = NULL")
        .execute(pool)
        .await
        .unwrap();
}

async fn expire(pool: &PgPool, ago: &str) {
    sqlx::query(&format!(
        "UPDATE oidc_grants SET access_expires_at = NOW() - INTERVAL '{ago}'"
    ))
    .execute(pool)
    .await
    .unwrap();
}

async fn authenticated(app: &Router, cookie: &str) -> (StatusCode, Value) {
    let (status, _, session) = call(
        app,
        "GET",
        "/api/session",
        None,
        Some(cookie),
        None,
        Value::Null,
    )
    .await;
    (status, session["authenticated"].clone())
}

#[tokio::test]
async fn a_sign_in_ends_on_any_request_once_the_provider_refuses_its_grant() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let (cookie, csrf, _) = oidc_session(&app, &issuer, "student").await;
    let (status, _, made) = call(
        &app,
        "POST",
        "/api/auth/tokens",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        json!({"name":"laptop","days":1}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");

    // A live access token needs no refresh; an expired one is refreshed first.
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(true))
    );
    assert!(issuer.presented.lock().await.is_empty());
    expire(&db.pool, "1 minute").await;
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(true))
    );
    assert_eq!(issuer.presented.lock().await.len(), 1);

    // A provider that can't answer is trusted for a while after expiry, then not.
    *issuer.refresh_mode.lock().await = "unavailable";
    expire(&db.pool, "1 minute").await;
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(true))
    );
    // The provider isn't asked again for a while after it failed.
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(true))
    );
    assert_eq!(issuer.presented.lock().await.len(), 2);
    expire(&db.pool, "20 minutes").await;
    assert_eq!(
        authenticated(&app, &cookie).await.0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(issuer.presented.lock().await.len(), 2);
    retry_now(&db.pool).await;
    assert_eq!(
        authenticated(&app, &cookie).await.0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(issuer.presented.lock().await.len(), 3);

    // A request that never reaches the models still ends a refused sign-in, and the
    // API token made in it ends too.
    retry_now(&db.pool).await;
    *issuer.refresh_mode.lock().await = "invalid_grant";
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(false))
    );
    let (sessions, live_tokens): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM auth_sessions), (SELECT COUNT(*) FROM api_tokens WHERE revoked_at IS NULL)",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((sessions, live_tokens), (0, 0));
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn a_refusal_of_chats_client_ends_no_sign_in() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let grants = auth.grants().unwrap();
    let (cookie, csrf, user) = oidc_session(&app, &issuer, "student").await;
    let (_, _, _) = oidc_session(&app, &issuer, "student").await;
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/auth/tokens",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        json!({"name":"laptop","days":1}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    *issuer.refresh_mode.lock().await = "invalid_client";
    expire(&db.pool, "1 second").await;
    let Err(error) = grants.access_token(user.id, None).await else {
        panic!("a refused client gives no token");
    };
    assert_eq!(error.0, StatusCode::BAD_GATEWAY);
    expire(&db.pool, "1 minute").await;
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(true))
    );
    let (grants_left, sessions, live_tokens): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM oidc_grants), (SELECT COUNT(*) FROM auth_sessions), \
         (SELECT COUNT(*) FROM api_tokens WHERE revoked_at IS NULL)",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((grants_left, sessions, live_tokens), (2, 2, 1));
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn each_sign_in_carries_its_own_grant() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let grants = auth.grants().unwrap();
    let (_, _, user) = oidc_session(&app, &issuer, "student").await;
    let (phone_cookie, _, _) = oidc_session(&app, &issuer, "student").await;
    let sessions: Vec<Uuid> =
        sqlx::query_scalar("SELECT session_id FROM oidc_grants ORDER BY created_at, id")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    let (laptop, phone) = (sessions[0], sessions[1]);
    let token = |session| {
        let grants = grants.clone();
        async move { grants.access_token(user.id, Some(session)).await }
    };
    let access = issuer.access.lock().await.clone();
    let header = |t: Option<grants::ProviderToken>| {
        t.unwrap()
            .authorization()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        header(token(laptop).await.unwrap()),
        format!("Bearer {}", access[0])
    );
    assert_eq!(
        header(token(phone).await.unwrap()),
        format!("Bearer {}", access[1])
    );

    // The provider ends the laptop's sign-in only; the laptop's calls stop and its
    // session ends, while the phone goes on.
    let laptop_refresh = issuer.refresh.lock().await[0].clone();
    issuer.live.lock().await.remove(&laptop_refresh);
    expire_soon(&db.pool, "").await;
    let Err(error) = token(laptop).await else {
        panic!("the laptop's grant was ended");
    };
    assert_eq!(
        (error.0, error.1.as_str()),
        (StatusCode::UNAUTHORIZED, grants::SIGN_IN_AGAIN)
    );
    let laptop_left: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE id=$1)")
            .bind(laptop)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(!laptop_left);
    header(token(phone).await.unwrap());
    assert_eq!(
        authenticated(&app, &phone_cookie).await,
        (StatusCode::OK, json!(true))
    );
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn a_caller_that_goes_away_does_not_lose_a_rotated_token() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let grants = auth.grants().unwrap();
    let (_, _, user) = oidc_session(&auth.router(), &issuer, "student").await;
    expire_soon(&db.pool, "").await;
    issuer
        .refresh_delay
        .store(300, std::sync::atomic::Ordering::SeqCst);
    // The caller is dropped while the provider rotates the token.
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        grants.access_token(user.id, None)
    )
    .await
    .is_err());
    // Shutdown waits for it.
    grants.finish().await;
    let saved: bool = sqlx::query_scalar(
        "SELECT refreshing_until IS NULL AND access_expires_at > NOW() + INTERVAL '5 minutes' FROM oidc_grants",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(saved);
    issuer
        .refresh_delay
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let token = grants.access_token(user.id, None).await.unwrap().unwrap();
    let access = issuer.access.lock().await.clone();
    assert_eq!(
        token.authorization().unwrap(),
        format!("Bearer {}", access[1]).as_str()
    );
    assert_eq!(issuer.presented.lock().await.len(), 1);
    let leased: bool = sqlx::query_scalar("SELECT refreshing_until IS NOT NULL FROM oidc_grants")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(!leased);
    task.abort();
    db.close().await;
}

/// Starts a refresh on `holder` that the provider answers only after a second, and
/// lets its lease lapse once the provider has the token, as if the holder had
/// stalled. Returns the refresh under way.
async fn stall_refresh(
    db: &Database,
    issuer: &FakeIssuer,
    holder: grants::Grants,
    user: Uuid,
) -> tokio::task::JoinHandle<Result<Option<grants::ProviderToken>, ApiError>> {
    let presented = issuer.presented.lock().await.len();
    issuer
        .refresh_delay
        .store(1000, std::sync::atomic::Ordering::SeqCst);
    let stalled = tokio::spawn(async move { holder.access_token(user, None).await });
    while issuer.presented.lock().await.len() == presented {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    issuer
        .refresh_delay
        .store(0, std::sync::atomic::Ordering::SeqCst);
    sqlx::query("UPDATE oidc_grants SET refreshing_until = NOW()")
        .execute(&db.pool)
        .await
        .unwrap();
    stalled
}

#[tokio::test]
async fn a_lapsed_lease_ends_its_sign_in_without_presenting_the_token_again() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let first = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let second = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = first.router();
    let (cookie, _, user) = oidc_session(&app, &issuer, "student").await;
    let (one, two) = (first.grants().unwrap(), second.grants().unwrap());

    // The holder may have had the token rotated, so the other replica ends the
    // sign-in instead of presenting it. The holder's late answer finds the grant
    // ended and revokes the token it got.
    expire(&db.pool, "1 second").await;
    let stalled = stall_refresh(&db, &issuer, one.clone(), user.id).await;
    let Err(error) = two.access_token(user.id, None).await else {
        panic!("the sign-in ended");
    };
    assert_eq!(
        (error.0, error.1.as_str()),
        (StatusCode::UNAUTHORIZED, grants::SIGN_IN_AGAIN)
    );
    assert_eq!(
        stalled.await.unwrap().err().unwrap().1.as_str(),
        grants::SIGN_IN_AGAIN
    );
    one.finish().await;
    let (refresh, presented) = (
        issuer.refresh.lock().await.clone(),
        issuer.presented.lock().await.clone(),
    );
    assert_eq!(presented, [refresh[0].clone()]);
    assert_eq!(*issuer.revoked.lock().await, [refresh[1].clone()]);
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_revocations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        queued, 0,
        "the token that may be rotated out is not sent anywhere"
    );
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(false))
    );
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn a_token_sent_without_an_answer_is_presented_again_only_for_a_while() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let grants = auth.grants().unwrap();
    let (cookie, _, user) = oidc_session(&app, &issuer, "student").await;
    let header = |token: Option<grants::ProviderToken>| {
        token
            .unwrap()
            .authorization()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    let state = || async {
        sqlx::query_as::<_, (bool, bool)>(
            "SELECT presented_at IS NOT NULL, refreshing_until IS NOT NULL FROM oidc_grants",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap()
    };

    // An answer that can't be read leaves the token in doubt; the holder asks
    // again at once, and an answer settles it.
    issuer.garble.store(1, std::sync::atomic::Ordering::SeqCst);
    expire(&db.pool, "1 second").await;
    let token = grants.access_token(user.id, None).await.unwrap();
    assert_eq!(
        header(token),
        format!("Bearer {}", issuer.access.lock().await[1])
    );
    let refresh = issuer.refresh.lock().await.clone();
    assert_eq!(
        *issuer.presented.lock().await,
        [refresh[0].clone(), refresh[0].clone()]
    );
    assert_eq!(state().await, (false, false));

    // An error answer means the provider did not take it: no doubt is kept.
    *issuer.refresh_mode.lock().await = "unavailable";
    expire(&db.pool, "1 second").await;
    assert_eq!(
        grants.access_token(user.id, None).await.err().unwrap().0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(issuer.presented.lock().await.len(), 3);
    assert_eq!(state().await, (false, false));

    // A token sent without an answer longer ago than the window, after the holder
    // gave up or stopped, is never presented again: the sign-in ends.
    retry_now(&db.pool).await;
    *issuer.refresh_mode.lock().await = "ok";
    sqlx::query("UPDATE oidc_grants SET presented_at = NOW() - INTERVAL '50 seconds'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        grants.access_token(user.id, None).await.err().unwrap().1,
        grants::SIGN_IN_AGAIN
    );
    assert_eq!(issuer.presented.lock().await.len(), 3);
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(false))
    );
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_revocations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(queued, 0);

    // A provider that rotated the token and lost its answer refuses it when asked
    // again within the window, and that ends the sign-in too.
    let (cookie, _, _) = oidc_session(&app, &issuer, "student").await;
    *issuer.refresh_mode.lock().await = "lost";
    expire(&db.pool, "1 second").await;
    assert_eq!(
        grants.access_token(user.id, None).await.err().unwrap().1,
        grants::SIGN_IN_AGAIN
    );
    let presented = issuer.presented.lock().await.clone();
    assert_eq!(presented.len(), 5);
    assert_eq!(presented[3], presented[4]);
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(false))
    );
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn shutdown_starts_no_refresh_and_stops_presenting_tokens() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let grants = auth.grants().unwrap();
    let (_, _, user) = oidc_session(&auth.router(), &issuer, "student").await;

    // A refresh whose answer can't be read is not asked again once shutdown began;
    // its token keeps the time it was sent, for another replica to settle.
    issuer.garble.store(1, std::sync::atomic::Ordering::SeqCst);
    issuer
        .refresh_delay
        .store(300, std::sync::atomic::Ordering::SeqCst);
    expire(&db.pool, "1 second").await;
    let refreshing = tokio::spawn({
        let grants = grants.clone();
        async move { grants.access_token(user.id, None).await }
    });
    while issuer.presented.lock().await.is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    grants.close();
    assert_eq!(
        refreshing.await.unwrap().err().unwrap().0,
        StatusCode::BAD_GATEWAY
    );
    grants.finish().await;
    assert_eq!(issuer.presented.lock().await.len(), 1);
    let (doubt, leased): (bool, bool) = sqlx::query_as(
        "SELECT presented_at IS NOT NULL, refreshing_until IS NOT NULL FROM oidc_grants",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((doubt, leased), (true, false));

    // No new refresh starts: a token that still lasts serves, an expired one fails.
    expire_soon(&db.pool, "").await;
    let token = grants.access_token(user.id, None).await.unwrap().unwrap();
    assert_eq!(
        token.authorization().unwrap(),
        format!("Bearer {}", issuer.access.lock().await[0]).as_str()
    );
    expire(&db.pool, "1 second").await;
    assert_eq!(
        grants.access_token(user.id, None).await.err().unwrap().0,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(issuer.presented.lock().await.len(), 1);

    // Another replica settles it inside the window.
    let other = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    issuer
        .refresh_delay
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let token = other
        .grants()
        .unwrap()
        .access_token(user.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        token.authorization().unwrap(),
        format!("Bearer {}", issuer.access.lock().await[1]).as_str()
    );
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn a_slow_provider_holds_no_request_up_inside_the_grace() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let grants = auth.grants().unwrap();
    let (cookie, _, _) = oidc_session(&app, &issuer, "student").await;
    issuer
        .refresh_delay
        .store(5000, std::sync::atomic::Ordering::SeqCst);
    expire(&db.pool, "1 minute").await;
    let started = std::time::Instant::now();
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(true))
    );
    let waited = started.elapsed();
    assert!(waited < Duration::from_millis(4500), "{waited:?}");
    // The refresh goes on and saves its token.
    grants.finish().await;
    let saved: bool = sqlx::query_scalar(
        "SELECT refreshing_until IS NULL AND access_expires_at > NOW() + INTERVAL '5 minutes' FROM oidc_grants",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(saved);
    assert_eq!(issuer.presented.lock().await.len(), 1);
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn requests_waiting_on_a_failed_refresh_do_not_ask_the_provider_again() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let grants = auth.grants().unwrap();
    let (cookie, _, user) = oidc_session(&app, &issuer, "student").await;
    expire(&db.pool, "1 second").await;
    *issuer.refresh_mode.lock().await = "unavailable";
    issuer
        .refresh_delay
        .store(300, std::sync::atomic::Ordering::SeqCst);
    let call = || {
        let grants = grants.clone();
        async move { grants.access_token(user.id, None).await }
    };
    let (a, b, c) = tokio::join!(call(), call(), call());
    for result in [a, b, c] {
        assert_eq!(result.err().unwrap().0, StatusCode::BAD_GATEWAY);
    }
    assert_eq!(issuer.presented.lock().await.len(), 1);
    // Later requests fail at once, and sign-ins in the grace go on, without asking.
    let started = std::time::Instant::now();
    assert_eq!(call().await.err().unwrap().0, StatusCode::BAD_GATEWAY);
    assert_eq!(
        authenticated(&app, &cookie).await,
        (StatusCode::OK, json!(true))
    );
    assert!(started.elapsed() < Duration::from_millis(300));
    assert_eq!(issuer.presented.lock().await.len(), 1);
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn signing_out_does_not_need_the_provider() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let (cookie, csrf, _) = oidc_session(&app, &issuer, "student").await;
    let (other, _, _) = oidc_session(&app, &issuer, "student").await;
    let (third, _, _) = oidc_session(&app, &issuer, "student").await;
    let (_, _, listed) = call(
        &app,
        "GET",
        "/api/auth/sessions",
        None,
        Some(&cookie),
        None,
        Value::Null,
    )
    .await;
    let other_id = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["current"] == false)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    *issuer.refresh_mode.lock().await = "unavailable";
    expire(&db.pool, "20 minutes").await;
    assert_eq!(
        authenticated(&app, &cookie).await.0,
        StatusCode::BAD_GATEWAY
    );
    let presented = issuer.presented.lock().await.len();
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/auth/sessions/{other_id}"),
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/auth/sessions/revoke-others",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/auth/logout",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(issuer.presented.lock().await.len(), presented);
    for cookie in [&cookie, &other, &third] {
        assert_eq!(
            authenticated(&app, cookie).await,
            (StatusCode::OK, json!(false))
        );
    }
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn ended_sign_ins_are_not_refreshed_and_previews_follow_the_password_rule() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let grants = auth.grants().unwrap();
    oidc_session(&app, &issuer, "student").await;
    let session: Uuid = sqlx::query_scalar("SELECT session_id FROM oidc_grants")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    expire(&db.pool, "1 minute").await;
    sqlx::query("UPDATE auth_sessions SET expires_at = NOW() - INTERVAL '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    // The session's own checks refuse it; its grant waits for the sweep.
    assert!(grants.confirm(session).await.unwrap());
    assert!(issuer.presented.lock().await.is_empty());

    // With passwords off, a preview of a password session is refused too.
    sign_in(&app, "admin", PASSWORD).await;
    let password: Uuid =
        sqlx::query_scalar("SELECT id FROM auth_sessions WHERE NOT single_sign_on")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(session_owner(&db.pool, password, true)
        .await
        .unwrap()
        .is_some());
    assert!(session_owner(&db.pool, password, false)
        .await
        .unwrap()
        .is_none());
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn a_failed_sign_in_revokes_its_grant_and_accounts_link_one_identity() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let mut cfg = forward_config(&issuer);
    cfg.oidc.as_mut().unwrap().allow_signup = false;
    let app = Auth::new(db.pool.clone(), cfg).await.unwrap().router();
    let (browser, path) = issuer.flow(&app, "valid", "stranger").await;
    let (status, _, _) = call(&app, "GET", &path, None, Some(&browser), None, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let issued = issuer.refresh.lock().await[0].clone();
    let mut revoked = false;
    for _ in 0..50 {
        if issuer.revoked.lock().await.contains(&issued) {
            revoked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(revoked, "the grant of a failed sign-in is revoked");

    let (ac, at, admin) = sign_in(&app, "admin", PASSWORD).await;
    let link = |subject: &'static str| {
        let (app, ac, at, id) = (app.clone(), ac.clone(), at.clone(), admin["id"].clone());
        async move {
            call(
                &app,
                "POST",
                "/api/admin/oidc-identities",
                Some(ORIGIN),
                Some(&ac),
                Some(&at),
                json!({"userId":id,"subject":subject}),
            )
            .await
            .0
        }
    };
    assert_eq!(link("admin-subject").await, StatusCode::CREATED);
    assert_eq!(link("someone-else").await, StatusCode::CONFLICT);
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn ending_a_sign_in_revokes_its_refresh_token_at_the_provider() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let app = auth.router();
    let revoked = |n: usize| {
        let issuer = issuer.clone();
        async move {
            for _ in 0..100 {
                if issuer.revoked.lock().await.len() >= n {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            issuer.revoked.lock().await.clone()
        }
    };

    let (cookie, csrf, user) = oidc_session(&app, &issuer, "student").await;
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/auth/logout",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let refresh = issuer.refresh.lock().await.clone();
    assert_eq!(revoked(1).await, [refresh[0].clone()]);

    // A session that expired is swept and revoked too.
    oidc_session(&app, &issuer, "student").await;
    sqlx::query("UPDATE auth_sessions SET expires_at = NOW() - INTERVAL '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    auth.grants().unwrap().sweep().await;
    let refresh = issuer.refresh.lock().await.clone();
    assert_eq!(revoked(2).await[1], refresh[1]);

    // And so is every sign-in of an account an administrator disables.
    oidc_session(&app, &issuer, "student").await;
    let (admin, admin_csrf, _) = sign_in(&app, "admin", PASSWORD).await;
    let (status, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/admin/users/{}", user.id),
        Some(ORIGIN),
        Some(&admin),
        Some(&admin_csrf),
        json!({"enabled":false}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let refresh = issuer.refresh.lock().await.clone();
    assert_eq!(revoked(3).await[2], refresh[2]);
    for _ in 0..100 {
        let left: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM oidc_grants) + (SELECT COUNT(*) FROM oidc_revocations)",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_revocations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn turning_passwords_off_closes_every_password_path() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let mut cfg = forward_config(&issuer);
    cfg.forward_tokens = false;
    // While passwords are on, the bootstrap administrator is linked to single sign-on.
    let open = Auth::new(db.pool.clone(), cfg.clone())
        .await
        .unwrap()
        .router();
    let (ac, at, admin) = sign_in(&open, "admin", PASSWORD).await;
    let (status, _, _) = call(
        &open,
        "POST",
        "/api/admin/oidc-identities",
        Some(ORIGIN),
        Some(&ac),
        Some(&at),
        json!({"userId":admin["id"],"subject":"admin-subject"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let member = managed_user(&open, &ac, &at, "member", "user", true).await;
    let (status, _, _) = call(
        &open,
        "POST",
        "/api/auth/tokens",
        Some(ORIGIN),
        Some(&ac),
        Some(&at),
        json!({"name":"made with a password","days":1}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    cfg.password_sign_in = false;
    assert!(
        Auth::new(db.pool.clone(), cfg.clone()).await.is_err(),
        "a bootstrap password is refused"
    );
    cfg.bootstrap_username = None;
    cfg.bootstrap_password = None;
    let app = Auth::new(db.pool.clone(), cfg).await.unwrap().router();
    let (status, _, body) = call(
        &app,
        "POST",
        "/api/auth/login",
        Some(ORIGIN),
        None,
        None,
        json!({"username":"admin","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // Sign-ins made with a password, and their API tokens, ended at startup.
    assert_eq!(
        authenticated(&app, &ac).await,
        (StatusCode::OK, json!(false))
    );
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens WHERE revoked_at IS NULL")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(live, 0);
    // One made meanwhile by a replica that still allows passwords is refused.
    let (late, _, _) = sign_in(&open, "member", PASSWORD).await;
    assert_eq!(
        authenticated(&app, &late).await,
        (StatusCode::OK, json!(false))
    );
    let (_, _, session) = call(&app, "GET", "/api/session", None, None, None, Value::Null).await;
    assert_eq!(
        (
            session["passwordSignIn"].clone(),
            session["oidcEnabled"].clone()
        ),
        (json!(false), json!(true))
    );
    let (cookie, csrf, _) = oidc_session(&app, &issuer, "admin-subject").await;
    let member_id = member["id"].as_str().unwrap();
    for (method, path, body) in [
        (
            "POST",
            "/api/auth/password".to_string(),
            json!({"currentPassword":PASSWORD,"newPassword":"another long password 1"}),
        ),
        (
            "POST",
            format!("/api/admin/users/{member_id}/password"),
            json!({"newPassword":"another long password 1"}),
        ),
        (
            "POST",
            "/api/admin/users".to_string(),
            json!({"username":"with-password","password":PASSWORD}),
        ),
        (
            "POST",
            "/api/admin/invitations".to_string(),
            json!({"username":"invited"}),
        ),
        (
            "POST",
            format!("/api/admin/users/{member_id}/access-link"),
            json!({"kind":"reset"}),
        ),
        (
            "POST",
            "/api/auth/activate".to_string(),
            json!({"token":random_token(),"newPassword":"another long password 1"}),
        ),
    ] {
        let (status, _, body) = call(
            &app,
            method,
            &path,
            Some(ORIGIN),
            Some(&cookie),
            Some(&csrf),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }
    // Accounts are made without a password and linked to single sign-on.
    let (status, _, created) = call(
        &app,
        "POST",
        "/api/admin/users",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        json!({"username":"sso-only","forcePasswordChange":true}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/admin/oidc-identities",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        json!({"userId":created["id"],"subject":"sso-only-subject"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (local, forced): (bool, bool) = sqlx::query_as("SELECT password_hash IS NOT NULL, password_change_required FROM users WHERE username='sso-only'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(!local && !forced);
    let forced: bool =
        sqlx::query_scalar("SELECT password_change_required FROM users WHERE username='member'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(!forced, "no one waits on a password change they can't make");
    task.abort();
    db.close().await;
}

#[tokio::test]
async fn auto_mode_stops_when_its_owner_has_no_grant_left() {
    let Some(db) = Database::new().await else {
        return;
    };
    let (issuer, task) = FakeIssuer::start().await;
    let auth = Auth::new(db.pool.clone(), forward_config(&issuer))
        .await
        .unwrap();
    let (base, seen, gateway_task) = fake_gateway().await;
    let gateway = crate::gateway::Gateway::forwarding(base)
        .unwrap()
        .forward_tokens(auth.grants().unwrap());
    let key = SecretKey::from_bytes([9u8; 32]);
    let mut state = crate::AppState::new(auth, db.pool.clone(), Some(gateway), 4);
    state.followups_enabled = false;
    state.connections = Some(
        crate::connections::Connections::with_keys(db.pool.clone(), None, Some(key), true).unwrap(),
    );
    let app = crate::router(state.clone(), "web".into());
    let (cookie, csrf, user) = oidc_session(&app, &issuer, "student").await;
    let (status, _, created) = call(
        &app,
        "POST",
        "/api/conversations",
        Some(ORIGIN),
        Some(&cookie),
        Some(&csrf),
        json!({"title":"Overnight","model":"fixture-model"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["conversation"]["id"].as_str().unwrap().to_owned();
    // The turn stops with a gateway failure, which Auto mode would nudge.
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/conversations/{id}/messages"))
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &cookie)
        .header("x-csrf-token", &csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"content":"run all night","model":"fixture-model"}).to_string(),
        ))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))));
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap();
    let access = issuer.access.lock().await.clone();
    assert!(seen
        .lock()
        .await
        .iter()
        .all(|h| *h == format!("Bearer {}", access[0])));
    let messages = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages")
            .fetch_one(&db.pool)
            .await
            .unwrap()
    };
    let before = messages().await;
    sqlx::query(
        "UPDATE conversations SET auto_approve=TRUE, updated_at=NOW()-INTERVAL '5 minutes'",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    // The provider ends the grant overnight.
    *issuer.refresh_mode.lock().await = "invalid_grant";
    expire_soon(&db.pool, "").await;
    crate::autopilot::tick(&state).await.unwrap();
    let nudges: i32 = sqlx::query_scalar("SELECT auto_nudges FROM conversations WHERE id=$1::uuid")
        .bind(&id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        nudges,
        crate::autopilot::MAX_NUDGES,
        "Auto mode stops instead of nudging again"
    );
    assert_eq!(messages().await, before);
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_sessions WHERE user_id=$1")
        .bind(user.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    gateway_task.abort();
    task.abort();
    db.close().await;
}
