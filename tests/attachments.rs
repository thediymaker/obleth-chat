//! Real database checks for private files, durable references, and concurrent quotas.
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use http_body_util::BodyExt;
use obleth_chat::{
    attachments::{self, ArchivedAttachment},
    auth::{Auth, AuthConfig},
    files::Files,
    AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aQq8AAAAASUVORK5CYII=";

struct Session {
    id: Uuid,
    cookie: String,
    csrf: String,
}
async fn fixture(pool: &PgPool) -> Router {
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
    // The composed router, so handler headers are checked as the site sends them.
    obleth_chat::router(AppState::new(auth, pool.clone(), None, 4), "web".into())
}
async fn session(pool: &PgPool) -> Session {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name)VALUES($1,$2,'File test')")
        .bind(id)
        .bind(format!("file_{id}"))
        .execute(pool)
        .await
        .unwrap();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()));
    sqlx::query("INSERT INTO auth_sessions(token_hash,user_id,expires_at)VALUES($1,$2,NOW()+INTERVAL '1 hour')")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(pool).await.unwrap();
    let csrf = URL_SAFE_NO_PAD.encode(Sha256::digest(
        format!("obleth-chat-csrf:{token}").as_bytes(),
    ));
    Session {
        id,
        cookie: format!("obleth_chat_session={token}"),
        csrf,
    }
}
async fn raw(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let result = app
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
    let status = result.status();
    let headers = result.headers().clone();
    let bytes = result
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}
async fn call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let (status, _, bytes) = raw(app, user, method, path, body).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn text_file(name: &str, content: &[u8]) -> Value {
    json!({"name":name,"mimeType":"text/plain","dataBase64":STANDARD.encode(content)})
}
async fn uploaded(app: &Router, user: &Session, body: Value) -> Uuid {
    let (status, body) = call(app, user, "POST", "/api/attachments", body).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body.get("dataBase64").is_none());
    body["id"].as_str().unwrap().parse().unwrap()
}
async fn message(pool: &PgPool, owner: Uuid) -> (Uuid, Uuid) {
    let conversation = Uuid::new_v4();
    let message = Uuid::new_v4();
    sqlx::query("INSERT INTO conversations(id,owner_id,title,model)VALUES($1,$2,'Files','test')")
        .bind(conversation)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status)VALUES($1,$2,0,'user','Read the file','completed')")
        .bind(message).bind(conversation).execute(pool).await.unwrap();
    (conversation, message)
}

#[sqlx::test(migrations = "./migrations")]
async fn uploads_are_private_downloads_and_require_csrf(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    let bob = session(&pool).await;
    let content = b"<script>alert('text only')</script>\n";
    let id=uploaded(&app,&alice,json!({"name":"example page.html","mimeType":"text/html","dataBase64":STANDARD.encode(content)})).await;
    let path = format!("/api/attachments/{id}");
    let (status, headers, bytes) = raw(&app, &alice, "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, content);
    assert_eq!(headers["content-type"], "text/plain; charset=utf-8");
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers["content-security-policy"]
        .to_str()
        .unwrap()
        .starts_with("sandbox;"));
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(
        call(&app, &bob, "GET", &path, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &bob, "DELETE", &path, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, listed) = call(&app, &bob, "GET", "/api/attachments", Value::Null).await;
    assert!(listed["attachments"].as_array().unwrap().is_empty());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/attachments")
                .header("origin", ORIGIN)
                .header("cookie", &alice.cookie)
                .header("content-type", "application/json")
                .body(Body::from(text_file("a.txt", b"hello").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(&app, &alice, "DELETE", &path, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn file_types_signatures_sizes_and_names_are_validated(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    let cases = [
        text_file("../escape.txt", b"ok"),
        text_file("x\r\nHeader.txt", b"ok"),
        text_file("vector.svg", b"<svg/>"),
        text_file("file.pdf", b"%PDF-1.7"),
        text_file("binary.txt", &[0, 1, 2]),
        text_file("latin1.txt", &[0xff]),
        json!({"name":"x.png","mimeType":"image/png","dataBase64":STANDARD.encode(b"not an image")}),
        json!({"name":"x.jpeg","mimeType":"image/jpeg","dataBase64":PNG}),
        json!({"name":"x.webp","mimeType":"image/webp","dataBase64":PNG}),
        json!({"name":"x.svg","mimeType":"image/svg+xml","dataBase64":STANDARD.encode(b"<svg/>")}),
        json!({"name":"remote.txt","mimeType":"text/plain","dataBase64":"https://example.com/file"}),
        text_file("large.txt", &vec![b'x'; attachments::MAX_TEXT_BYTES + 1]),
        json!({"name":"large.png","mimeType":"image/png","dataBase64":STANDARD.encode(vec![0;attachments::MAX_IMAGE_BYTES+1])}),
    ];
    for body in cases {
        assert_eq!(
            call(&app, &alice, "POST", "/api/attachments", body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let id = uploaded(
        &app,
        &alice,
        json!({"name":"pixel.png","mimeType":"image/png","dataBase64":PNG}),
    )
    .await;
    let (_, _, data) = raw(
        &app,
        &alice,
        "GET",
        &format!("/api/attachments/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(data, STANDARD.decode(PNG).unwrap());
    // The route's larger body limit overrides the application's normal 128 KiB limit.
    uploaded(
        &app,
        &alice,
        text_file("large source.py", &vec![b'x'; attachments::MAX_TEXT_BYTES]),
    )
    .await;
    let webp = "UklGRhoAAABXRUJQVlA4TA0AAAAvAAAAEAcQERGIiP4HAA==";
    uploaded(
        &app,
        &alice,
        json!({"name":"pixel.webp","mimeType":"image/webp","dataBase64":webp}),
    )
    .await;
}

#[sqlx::test(migrations = "./migrations")]
async fn simultaneous_uploads_cannot_exceed_file_or_byte_quotas(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    sqlx::query("INSERT INTO attachments(id,owner_id,name,mime_type,kind,size,data)SELECT gen_random_uuid(),$1,'seed.txt','text/plain','text',1,'x'::bytea FROM generate_series(1,99)")
        .bind(alice.id).execute(&pool).await.unwrap();
    let (a, b) = tokio::join!(
        call(
            &app,
            &alice,
            "POST",
            "/api/attachments",
            text_file("a.txt", b"ok")
        ),
        call(
            &app,
            &alice,
            "POST",
            "/api/attachments",
            text_file("b.txt", b"ok")
        )
    );
    let mut statuses = [a.0.as_u16(), b.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE owner_id=$1")
        .bind(alice.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 100);
    let bob = session(&pool).await;
    sqlx::query("INSERT INTO attachments(id,owner_id,name,mime_type,kind,size,data)SELECT gen_random_uuid(),$1,'seed.png','image/png','image',5242880-CASE WHEN n=10 THEN 3 ELSE 0 END,REPEAT('x',5242880-CASE WHEN n=10 THEN 3 ELSE 0 END)::bytea FROM generate_series(1,10) AS g(n)")
        .bind(bob.id).execute(&pool).await.unwrap();
    let (a, b) = tokio::join!(
        call(
            &app,
            &bob,
            "POST",
            "/api/attachments",
            text_file("a.txt", b"ok")
        ),
        call(
            &app,
            &bob,
            "POST",
            "/api/attachments",
            text_file("b.txt", b"ok")
        )
    );
    let mut statuses = [a.0.as_u16(), b.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    let bytes: i64 =
        sqlx::query_scalar("SELECT SUM(size)::BIGINT FROM attachments WHERE owner_id=$1")
            .bind(bob.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bytes, attachments::MAX_USER_BYTES - 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn references_enforce_ownership_capabilities_and_safe_deletion(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    let bob = session(&pool).await;
    let text = uploaded(
        &app,
        &alice,
        text_file("notes.md", b"Private UTF-8 text: \xe2\x9c\x93"),
    )
    .await;
    let image = uploaded(
        &app,
        &alice,
        json!({"name":"pixel.png","mimeType":"image/png","dataBase64":PNG}),
    )
    .await;
    let (conversation, message) = message(&pool, alice.id).await;
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        attachments::load_owner(
            &mut tx,
            &Files::new(pool.clone(), None),
            bob.id,
            &[text],
            true
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        attachments::load_owner(
            &mut tx,
            &Files::new(pool.clone(), None),
            alice.id,
            &[text, text],
            true
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        attachments::load_owner(
            &mut tx,
            &Files::new(pool.clone(), None),
            alice.id,
            &[image],
            false
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        attachments::link_message(&mut tx, bob.id, message, &[text])
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    attachments::link_message(&mut tx, alice.id, message, &[image, text])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(attachments::has_images(&pool, alice.id, conversation, &[])
        .await
        .unwrap());
    assert!(
        !attachments::has_images(&pool, bob.id, conversation, &[image])
            .await
            .unwrap()
    );
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        attachments::load_messages(
            &mut tx,
            &Files::new(pool.clone(), None),
            alice.id,
            &[message],
            false
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    let files = attachments::load_messages(
        &mut tx,
        &Files::new(pool.clone(), None),
        alice.id,
        &[message],
        true,
    )
    .await
    .unwrap();
    assert_eq!(files[&message][0].meta.id, image);
    assert_eq!(files[&message][1].meta.id, text);
    assert!(files[&message][0].content_part()["image_url"]["url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    assert!(files[&message][1]
        .text_context()
        .unwrap()
        .contains("Private UTF-8 text"));
    assert_eq!(
        files[&message][0].context_bytes(),
        files[&message][0].content_part().to_string().len()
    );
    let meta = attachments::metadata_for_messages(&mut tx, alice.id, &[message])
        .await
        .unwrap();
    assert!(!serde_json::to_string(&meta).unwrap().contains("dataBase64"));
    assert!(
        attachments::metadata_for_messages(&mut tx, bob.id, &[message])
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        call(
            &app,
            &alice,
            "DELETE",
            &format!("/api/attachments/{image}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    sqlx::query("DELETE FROM conversations WHERE id=$1")
        .bind(conversation)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            &alice,
            "DELETE",
            &format!("/api/attachments/{image}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn portable_archives_preserve_private_bytes_and_import_atomically(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    let bob = session(&pool).await;
    let id = uploaded(
        &app,
        &alice,
        text_file("source.rs", b"fn main() { println!(\"hello\"); }\n"),
    )
    .await;
    let (_, message) = message(&pool, alice.id).await;
    let mut tx = pool.begin().await.unwrap();
    attachments::link_message(&mut tx, alice.id, message, &[id])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let files = attachments::load_for_archive(
        &mut tx,
        &Files::new(pool.clone(), None),
        alice.id,
        &[message],
        1024,
    )
    .await
    .unwrap();
    let archive = files[&message][0].archive();
    assert!(attachments::load_for_archive(
        &mut tx,
        &Files::new(pool.clone(), None),
        alice.id,
        &[message],
        10
    )
    .await
    .is_err());
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let ids = attachments::import_in(
        &mut tx,
        &Files::new(pool.clone(), None),
        bob.id,
        std::slice::from_ref(&archive),
    )
    .await
    .unwrap();
    assert_ne!(ids[0], id);
    assert_eq!(
        attachments::import_in(
            &mut tx,
            &Files::new(pool.clone(), None),
            bob.id,
            std::slice::from_ref(&archive)
        )
        .await
        .unwrap(),
        ids
    );
    let duplicates = attachments::import_in(
        &mut tx,
        &Files::new(pool.clone(), None),
        bob.id,
        &[archive.clone(), archive.clone()],
    )
    .await
    .unwrap();
    assert_ne!(duplicates[0], duplicates[1]);
    tx.commit().await.unwrap();
    let (_, _, alice_bytes) = raw(
        &app,
        &alice,
        "GET",
        &format!("/api/attachments/{id}"),
        Value::Null,
    )
    .await;
    let (_, _, bob_bytes) = raw(
        &app,
        &bob,
        "GET",
        &format!("/api/attachments/{}", ids[0]),
        Value::Null,
    )
    .await;
    assert_eq!(alice_bytes, bob_bytes);
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE owner_id=$1")
        .bind(bob.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let new = ArchivedAttachment {
        name: "new.txt".into(),
        mime_type: "text/plain".into(),
        data_base64: STANDARD.encode(b"new"),
    };
    let bad = ArchivedAttachment {
        name: "bad.svg".into(),
        mime_type: "image/svg+xml".into(),
        data_base64: STANDARD.encode(b"<svg/>"),
    };
    assert!(attachments::import_in(
        &mut tx,
        &Files::new(pool.clone(), None),
        bob.id,
        &[new, bad]
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE owner_id=$1")
        .bind(bob.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    // Deferred file references allow deleting the account and all of its chats together.
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(alice.id)
        .execute(&pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn replayed_context_is_bounded_before_loading_file_bytes(pool: PgPool) {
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    let id = uploaded(
        &app,
        &alice,
        text_file("source.txt", &vec![b'x'; 60 * 1024]),
    )
    .await;
    let (_, first) = message(&pool, alice.id).await;
    let (_, second) = message(&pool, alice.id).await;
    let mut tx = pool.begin().await.unwrap();
    attachments::link_message(&mut tx, alice.id, first, &[id])
        .await
        .unwrap();
    attachments::link_message(&mut tx, alice.id, second, &[id])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        attachments::load_messages(
            &mut tx,
            &Files::new(pool.clone(), None),
            alice.id,
            &[first, second],
            true
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(attachments::load_for_archive(
        &mut tx,
        &Files::new(pool.clone(), None),
        alice.id,
        &[first, second],
        200 * 1024
    )
    .await
    .is_ok());
    let oversized = vec![Uuid::new_v4(); 201];
    assert!(attachments::load_messages(
        &mut tx,
        &Files::new(pool.clone(), None),
        alice.id,
        &oversized,
        true
    )
    .await
    .is_err());
    assert!(
        attachments::metadata_for_messages(&mut tx, alice.id, &oversized)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn store_branches_forks_and_archives_preserve_attached_files(pool: PgPool) {
    use obleth_chat::store::Store;
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    let bob = session(&pool).await;
    let text = uploaded(&app, &alice, text_file("source.rs", b"fn main() {}\n")).await;
    let image = uploaded(
        &app,
        &alice,
        json!({"name":"pixel.png","mimeType":"image/png","dataBase64":PNG}),
    )
    .await;
    let store = Store::new(pool.clone());
    let chat = store
        .create(alice.id, "Private files", "vision")
        .await
        .unwrap();
    let turn = store
        .start_turn_attached(
            alice.id,
            chat.id,
            "",
            "vision",
            "",
            0.7,
            256,
            None,
            1.0,
            "vision",
            &[text, image],
            true,
        )
        .await
        .unwrap();
    let user_message = turn.messages.last().unwrap().id;
    assert_eq!(turn.attachments[&user_message].len(), 2);
    store
        .finish_turn(
            alice.id,
            chat.id,
            turn.generation_id,
            "Read both files",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let saved = store.get(alice.id, chat.id).await.unwrap();
    assert_eq!(saved.messages[0].attachments[0].id, text);
    assert_eq!(saved.messages[0].attachments[1].id, image);
    let serialized = serde_json::to_string(&saved).unwrap();
    assert!(!serialized.contains("dataBase64"));
    assert!(!serialized.contains("fn main()"));
    // A later text-only model must reject historical images even with no new upload.
    assert_eq!(
        store
            .start_turn_attached(
                alice.id,
                chat.id,
                "Follow up",
                "text",
                "",
                0.7,
                256,
                None,
                1.0,
                "text",
                &[],
                false
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        store.get(alice.id, chat.id).await.unwrap().messages.len(),
        2
    );
    let branch = store
        .branch(alice.id, chat.id, turn.assistant_id)
        .await
        .unwrap();
    assert_ne!(branch.messages[0].id, user_message);
    assert_eq!(branch.messages[0].attachments[0].id, text);
    assert_eq!(branch.messages[0].attachments[1].id, image);
    let regeneration = store
        .fork_turn(alice.id, chat.id, turn.assistant_id, "regenerate", None)
        .await
        .unwrap();
    assert_eq!(regeneration.attachments.len(), 2);
    assert_eq!(regeneration.attachments[1].id, image);
    assert!(regeneration.conversation.messages.is_empty());
    let edited = store
        .fork_turn(
            alice.id,
            chat.id,
            user_message,
            "edit",
            Some("Inspect these"),
        )
        .await
        .unwrap();
    assert_eq!(edited.attachments[0].id, text);
    assert_eq!(edited.draft, "Inspect these");
    let archive = store.export(alice.id, &[chat.id]).await.unwrap();
    let encoded = serde_json::to_value(&archive).unwrap();
    assert_eq!(
        encoded["conversations"][0]["messages"][0]["attachments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        encoded["conversations"][0]["messages"][0]["attachments"][1]["dataBase64"],
        PNG
    );
    let imported = store.import(bob.id, archive).await.unwrap();
    assert_eq!(imported.len(), 1);
    let imported_chat = store.get(bob.id, imported[0].id).await.unwrap();
    assert_eq!(imported_chat.messages[0].attachments.len(), 2);
    let imported_image = imported_chat.messages[0].attachments[1].id;
    assert_ne!(imported_image, image);
    let (status, _, bytes) = raw(
        &app,
        &bob,
        "GET",
        &format!("/api/attachments/{imported_image}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, STANDARD.decode(PNG).unwrap());
    assert_eq!(
        call(
            &app,
            &alice,
            "GET",
            &format!("/api/attachments/{imported_image}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .start_turn_attached(
                bob.id,
                imported_chat.id,
                "Follow up",
                "text",
                "",
                0.7,
                256,
                None,
                1.0,
                "text",
                &[],
                false
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let followup = store
        .start_turn_attached(
            bob.id,
            imported_chat.id,
            "Follow up",
            "vision",
            "",
            0.7,
            256,
            None,
            1.0,
            "vision",
            &[],
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        followup.attachments.values().map(Vec::len).sum::<usize>(),
        2
    );
    store
        .finish_turn(
            bob.id,
            imported_chat.id,
            followup.generation_id,
            "Done",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn store_counts_replayed_files_and_rejects_foreign_ids_atomically(pool: PgPool) {
    use obleth_chat::store::Store;
    let app = fixture(&pool).await;
    let alice = session(&pool).await;
    let bob = session(&pool).await;
    let large = uploaded(&app, &alice, text_file("large.txt", &vec![b'x'; 70 * 1024])).await;
    let extra = uploaded(&app, &alice, text_file("extra.txt", &vec![b'y'; 40 * 1024])).await;
    let foreign = uploaded(&app, &bob, text_file("foreign.txt", b"Private Bob file")).await;
    let store = Store::new(pool.clone());
    let chat = store
        .create(alice.id, "Bounded files", "text")
        .await
        .unwrap();
    assert_eq!(
        store
            .start_turn_attached(
                alice.id,
                chat.id,
                "Inspect",
                "text",
                "",
                0.7,
                256,
                None,
                1.0,
                "text",
                &[foreign],
                false
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(store
        .get(alice.id, chat.id)
        .await
        .unwrap()
        .messages
        .is_empty());
    let turn = store
        .start_turn_attached(
            alice.id,
            chat.id,
            "Inspect",
            "text",
            "",
            0.7,
            256,
            None,
            1.0,
            "text",
            &[large],
            false,
        )
        .await
        .unwrap();
    store
        .finish_turn(
            alice.id,
            chat.id,
            turn.generation_id,
            "Done",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .start_turn_attached(
                alice.id,
                chat.id,
                "More context",
                "text",
                "",
                0.7,
                256,
                None,
                1.0,
                "text",
                &[extra],
                false
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let saved = store.get(alice.id, chat.id).await.unwrap();
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[0].attachments.len(), 1);
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE conversation_id=$1 AND status='pending'",
    )
    .bind(chat.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}
