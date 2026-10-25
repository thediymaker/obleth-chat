//! Files in object storage: attachments and generated images go to a mock S3 bucket,
//! are served through Chat, are copied when a shared chat is forked and are deleted
//! with their rows; files kept in Postgres move into the bucket once one is set.
//! Requires a disposable DATABASE_URL.
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{Request, StatusCode},
    response::{sse::Event, IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    files::{empty_trash, Files},
    gateway::Gateway,
    knowledge::ObjectStore,
    router, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";
const PIXEL: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aQq8AAAAASUVORK5CYII=";

type Objects = Arc<Mutex<HashMap<String, Vec<u8>>>>;
type Seen = Arc<Mutex<Vec<Value>>>;

async fn put_object(State(o): State<Objects>, Path(key): Path<String>, body: Bytes) -> StatusCode {
    o.lock().unwrap().insert(key, body.to_vec());
    StatusCode::OK
}
async fn get_object(State(o): State<Objects>, Path(key): Path<String>) -> impl IntoResponse {
    match o.lock().unwrap().get(&key) {
        Some(bytes) => (StatusCode::OK, bytes.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "<Error><Code>NoSuchKey</Code></Error>",
        )
            .into_response(),
    }
}
async fn delete_object(State(o): State<Objects>, Path(key): Path<String>) -> StatusCode {
    o.lock().unwrap().remove(&key);
    StatusCode::NO_CONTENT
}

/// A generated image: a PNG signature and a few hundred KiB of filler.
fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..300_000u32).map(|i| (i % 251) as u8));
    bytes
}

async fn chat(State(seen): State<Seen>, Json(body): Json<Value>) -> impl IntoResponse {
    seen.lock().unwrap().push(body);
    let image = format!(
        "\n\n![A moth](data:image/png;base64,{})",
        STANDARD.encode(png())
    );
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"content":"Here it is."}}]}).to_string()));
        yield Ok(Event::default().data(json!({"choices":[{"delta":{"content":image}}]}).to_string()));
        yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string()));
        yield Ok(Event::default().data("[DONE]"));
    };
    Sse::new(stream)
}

struct Fixture {
    /// Chat with the bucket.
    app: Router,
    /// Chat on the same database without a bucket.
    plain: Router,
    files: Files,
    store: ObjectStore,
    objects: Objects,
    seen: Seen,
}

async fn fixture(pool: PgPool) -> Fixture {
    let objects: Objects = Arc::default();
    let seen: Seen = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mocks = Router::new()
        .route(
            "/bucket/{*key}",
            get(get_object)
                .put(put_object)
                .delete(delete_object)
                .with_state(objects.clone()),
        )
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"fixture-model"}]})) }),
        )
        .route(
            "/model/info",
            get(|| async {
                Json(json!({"data":[{"model_name":"fixture-model","model_info":{"supports_vision":true}}]}))
            }),
        )
        .route("/v1/chat/completions", post(chat).with_state(seen.clone()));
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
    let store = ObjectStore::new(
        format!("http://{address}").parse().unwrap(),
        "bucket".into(),
        "auto".into(),
        "fixture-key".into(),
        "fixture-secret".into(),
    );
    let files = Files::new(pool.clone(), Some(store.clone()));
    let mut state = AppState::new(auth, pool, Some(gateway), 4);
    state.followups_enabled = false;
    let plain = router(state.clone(), "web".into());
    state.store = state.store.with_files(files.clone());
    Fixture {
        app: router(state, "web".into()),
        plain,
        files,
        store,
        objects,
        seen,
    }
}

type Session = (String, String);

fn req(method: &str, path: &str, body: Value, session: &Session) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("origin", ORIGIN)
        .header("cookie", &session.0)
        .header("x-csrf-token", &session.1)
        .header("content-type", "application/json")
        .body(Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    session: &Session,
) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(req(method, path, body, session))
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

async fn json_call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    session: &Session,
) -> Value {
    let (status, bytes) = call(app, method, path, body, session).await;
    assert!(
        status.is_success(),
        "{method} {path}: {status} {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn login(app: &Router, name: &str) -> Session {
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

async fn user_id(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Uploads the pixel, starts a chat with it and returns (chat, attachment, image).
async fn chat_with_files(app: &Router, session: &Session) -> (String, String, String) {
    let file = json_call(
        app,
        "POST",
        "/api/attachments",
        json!({"name":"pixel.png","mimeType":"image/png","dataBase64":PIXEL}),
        session,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let chat = json_call(
        app,
        "POST",
        "/api/conversations",
        json!({"title":"Files","model":"fixture-model"}),
        session,
    )
    .await["conversation"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = call(
        app,
        "POST",
        &format!("/api/conversations/{chat}/messages"),
        json!({"content":"Draw this","model":"fixture-model","attachmentIds":[file]}),
        session,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let detail = json_call(
        app,
        "GET",
        &format!("/api/conversations/{chat}"),
        Value::Null,
        session,
    )
    .await;
    let reply = detail["conversation"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|m| m["role"] == "assistant")
        .unwrap()
        .clone();
    assert_eq!(reply["status"], "completed", "{reply}");
    let content = reply["content"].as_str().unwrap();
    let start = content.find("/api/images/").expect(content) + "/api/images/".len();
    let image = content[start..start + 36].to_owned();
    (chat, file, image)
}

async fn body(app: &Router, path: &str, session: &Session) -> Vec<u8> {
    let (status, bytes) = call(app, "GET", path, Value::Null, session).await;
    assert_eq!(status, StatusCode::OK, "{path}");
    bytes
}

#[sqlx::test(migrations = "./migrations")]
async fn files_live_in_the_bucket_and_follow_their_rows(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let owner = user_id(&pool, "test-admin").await;
    let (chat, file, image) = chat_with_files(&f.app, &admin).await;
    let pixel = STANDARD.decode(PIXEL).unwrap();

    // Rows name their objects and hold no bytes; nothing is left pending.
    let (data, key, sha): (Option<Vec<u8>>, Option<String>, Vec<u8>) =
        sqlx::query_as("SELECT data, object_key, sha256 FROM attachments WHERE id = $1::uuid")
            .bind(&file)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(data, None);
    assert_eq!(
        key.as_deref(),
        Some(&*format!("attachments/{owner}/{file}"))
    );
    assert_eq!(sha, Sha256::digest(&pixel).to_vec());
    let image_key = format!("images/{owner}/{image}");
    let held: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT data FROM generated_images WHERE object_key = $1")
            .bind(&image_key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(held, None);
    assert_eq!(f.objects.lock().unwrap()[&image_key], png());
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM object_trash")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pending, 0);

    // The model saw the attachment, read back from the bucket.
    let sent = f.seen.lock().unwrap().last().unwrap().to_string();
    assert!(sent.contains(PIXEL), "{sent}");
    // Downloads come through Chat with their safety headers.
    let response = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/attachments/{file}"),
            Value::Null,
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.headers()["content-security-policy"],
        "sandbox; default-src 'none'"
    );
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
        pixel
    );
    assert_eq!(
        body(&f.app, &format!("/api/images/{image}"), &admin).await,
        png()
    );
    // Chat without the bucket can't read them.
    assert_eq!(
        call(
            &f.plain,
            "GET",
            &format!("/api/images/{image}"),
            Value::Null,
            &admin
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );

    // Shared readers get the files; a fork gets its own copies.
    json_call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"other","password":PASSWORD,"displayName":"Other","role":"user"}),
        &admin,
    )
    .await;
    json_call(
        &f.app,
        "PUT",
        &format!("/api/conversations/{chat}/share"),
        json!({"scope":"signed_in"}),
        &admin,
    )
    .await;
    let other = login(&f.app, "other").await;
    let reader = user_id(&pool, "other").await;
    assert_eq!(
        body(
            &f.app,
            &format!("/api/shared/{chat}/attachments/{file}"),
            &other
        )
        .await,
        pixel
    );
    assert_eq!(
        body(
            &f.app,
            &format!("/api/shared/{chat}/images/{image}"),
            &other
        )
        .await,
        png()
    );
    json_call(
        &f.app,
        "POST",
        &format!("/api/shared/{chat}/fork"),
        Value::Null,
        &other,
    )
    .await;
    let copies: Vec<(String, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT object_key, data FROM attachments WHERE owner_id = $1
         UNION ALL SELECT object_key, data FROM generated_images WHERE owner_id = $1",
    )
    .bind(reader)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(copies.len(), 2);
    for (key, data) in &copies {
        assert_eq!(*data, None);
        assert!(key.contains(&reader.to_string()), "{key}");
        assert!(f.objects.lock().unwrap().contains_key(key), "{key}");
    }

    // Deleting the rows deletes the objects; the fork's copies stay.
    json_call(
        &f.app,
        "DELETE",
        &format!("/api/conversations/{chat}"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/attachments/{file}"),
            Value::Null,
            &admin
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    empty_trash(&pool, &f.store).await;
    let objects = f.objects.lock().unwrap();
    assert_eq!(objects.len(), 2, "{:?}", objects.keys());
    for (key, _) in &copies {
        assert!(objects.contains_key(key), "{key}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn files_kept_in_postgres_move_into_the_bucket(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.plain, "test-admin").await;
    let (_, file, image) = chat_with_files(&f.plain, &admin).await;
    let held: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM attachments WHERE data IS NOT NULL AND object_key IS NULL)
              + (SELECT COUNT(*) FROM generated_images WHERE data IS NOT NULL AND object_key IS NULL)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(held, 2);
    assert!(f.objects.lock().unwrap().is_empty());

    let stop = CancellationToken::new();
    tokio::spawn(f.files.clone().worker(stop.clone()));
    let mut moved = false;
    for _ in 0..100 {
        let left: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM attachments WHERE data IS NOT NULL)
                  + (SELECT COUNT(*) FROM generated_images WHERE data IS NOT NULL)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if left == 0 {
            moved = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop.cancel();
    assert!(moved, "the worker moves every file");
    assert_eq!(f.objects.lock().unwrap().len(), 2);
    assert_eq!(
        body(&f.app, &format!("/api/attachments/{file}"), &admin).await,
        STANDARD.decode(PIXEL).unwrap()
    );
    assert_eq!(
        body(&f.app, &format!("/api/images/{image}"), &admin).await,
        png()
    );
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM object_trash")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pending, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn uploads_no_row_claims_are_deleted_after_a_grace_period(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let placed = f
        .files
        .place(
            "attachments/nobody/orphan".into(),
            b"orphan".to_vec(),
            "text/plain",
        )
        .await
        .unwrap();
    assert_eq!(placed.key.as_deref(), Some("attachments/nobody/orphan"));
    assert!(f
        .objects
        .lock()
        .unwrap()
        .contains_key("attachments/nobody/orphan"));

    // A row may still claim it, so it stays for now.
    empty_trash(&pool, &f.store).await;
    assert!(f
        .objects
        .lock()
        .unwrap()
        .contains_key("attachments/nobody/orphan"));

    sqlx::query("UPDATE object_trash SET created_at = NOW() - INTERVAL '20 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    empty_trash(&pool, &f.store).await;
    assert!(f.objects.lock().unwrap().is_empty());
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM object_trash")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
