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
    }
}

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
        let search_path = format!("SET search_path TO {schema}");
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
        sqlx::raw_sql(include_str!("../../../migrations/001_auth.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/004_accounts.sql"))
            .execute(&pool)
            .await
            .unwrap();
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
    assert_eq!(body, json!({"authenticated": false, "oidcEnabled": false}));
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
        };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(|State(s): State<FakeIssuer>| async move {
                Json(json!({"issuer":s.origin,"authorization_endpoint":format!("{}/authorize",s.origin),"token_endpoint":format!("{}/token",s.origin),"jwks_uri":format!("{}/jwks",s.origin),"response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["RS256"],"token_endpoint_auth_methods_supported":["client_secret_basic"],"code_challenge_methods_supported":["S256"]}))
            }))
            .route("/jwks", get(|State(s): State<FakeIssuer>| async move { Json(s.jwks) }))
            .route("/token", post(fake_token))
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
        _ => (),
    }
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-key".into());
    let mut token = jsonwebtoken::encode(&header, &claims, &state.key).unwrap();
    if mode == "signature" {
        let prefix = token.rsplit_once('.').unwrap().0;
        token = format!("{prefix}.{}", URL_SAFE_NO_PAD.encode([0u8; 256]));
    }
    Json(json!({"access_token":"unused-test-access-token","token_type":"Bearer","expires_in":600,"id_token":token})).into_response()
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
    let many_clients = (0..2 * ACCOUNT_FAILURES)
        .map(|index| Client(format!("bot-{index}")))
        .collect();
    assert_eq!(
        passed(many_clients).await,
        ACCOUNT_FAILURES - CLIENT_ACCOUNT_FAILURES
    );
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
