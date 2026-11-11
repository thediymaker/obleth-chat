//! Memory: after a reply, new messages are read as their owner and kept as short
//! memories linked to topics; later replies get the memories that fit; temporary chats,
//! people who turned memory off and servers with it off are never read; people see,
//! change and delete every memory. Requires a disposable DATABASE_URL.
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig, Identity, Role},
    gateway::Gateway,
    knowledge::{extract::Tools, Knowledge, ObjectStore},
    memory, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
type Seen = Arc<Mutex<Vec<Value>>>;

struct Session {
    id: Uuid,
    cookie: String,
    csrf: String,
}

async fn session(pool: &PgPool, role: &str) -> Session {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Memory test',$3)")
        .bind(id)
        .bind(format!("memory-{}", id.simple()))
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()));
    sqlx::query("INSERT INTO auth_sessions(token_hash,user_id,expires_at)VALUES($1,$2,NOW()+INTERVAL '1 hour')")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(pool).await.unwrap();
    Session {
        id,
        cookie: format!("obleth_chat_session={token}"),
        csrf: URL_SAFE_NO_PAD.encode(Sha256::digest(
            format!("obleth-chat-csrf:{token}").as_bytes(),
        )),
    }
}

fn identity(user: &Session) -> Identity {
    Identity {
        id: user.id,
        username: "memory".into(),
        display_name: "Memory test".into(),
        role: Role::User,
        session: None,
    }
}

/// Words hashed into a unit vector, so texts that share words are close.
fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; 1024];
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3)
    {
        let h = Sha256::digest(word.to_lowercase().as_bytes());
        v[(h[0] as usize) << 2 | (h[1] as usize & 3)] += 1.0;
    }
    v
}

async fn embeddings(Json(body): Json<Value>) -> Json<Value> {
    let data: Vec<Value> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, t)| json!({"index":i,"embedding":embed(t.as_str().unwrap())}))
        .collect();
    Json(json!({"data":data}))
}

/// A learning model: remembers hydrology and PyTorch, moves TensorFlow to PyTorch, and
/// tries to forget a pinned memory; files added memories under a course.
async fn completions(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(body.clone());
    let system = body["messages"][0]["content"].as_str().unwrap_or("");
    let input = body["messages"][1]["content"].as_str().unwrap_or("");
    let content = if system.starts_with("You file a memory") {
        json!({"kind":"work","topics":[{"name":"CHM 501","kind":"course"}]}).to_string()
    } else if input.contains("hydrology") {
        format!(
            "<think>lasting facts</think>{}",
            json!({"add":[
                {"text":"You are a PhD student in hydrology","kind":"about","topics":[{"name":"Hydrology","kind":"field"}]},
                {"text":"You train flood models with PyTorch","kind":"work","topics":[{"name":"PyTorch","kind":"tool"},{"name":"Flood forecasting","kind":"project"}]},
                {"text":"you are a phd student in HYDROLOGY.","kind":"about","topics":[]},
                {"text":"You are using a file named floods.nc","kind":"work","topics":[]}
            ],"update":[],"forget":[]})
        )
    } else if input.contains("switched") {
        let id = |text: &str| {
            input
                .lines()
                .find(|l| l.contains(text))
                .and_then(|l| l.split(':').next())
                .unwrap_or("m9")
                .to_owned()
        };
        json!({"add":[],"update":[{"id":id("TensorFlow"),"text":"You use PyTorch, not TensorFlow"}],"forget":[id("Always answer in French")]}).to_string()
    } else {
        json!({"add":[],"update":[],"forget":[]}).to_string()
    };
    Json(json!({"choices":[{"message":{"role":"assistant","content":content}}]}))
}

/// A decisions service: a memory naming a file is a detail of the task, anything else
/// lasts; a chat that mentions an outage gets no answer.
async fn verdicts(State(asked): State<Seen>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    asked.lock().unwrap().push(body.clone());
    if body["state"].to_string().contains("outage") {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"message": "busy"}})),
        );
    }
    let verdicts: serde_json::Map<String, Value> = body["questions"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(id, q)| {
            let yes = if q["instructions"].as_str().unwrap().contains("file named") { 0.02 } else { 0.97 };
            (id.clone(), json!({"type":"boolean","value":yes > 0.5,"probabilities":{"true":yes,"false":1.0 - yes},"confidence":0.9}))
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({"model":"judge","verdicts":verdicts})),
    )
}

async fn fixture(pool: &PgPool) -> (AppState, Router, Seen) {
    fixture_asked(pool).await.0
}

async fn fixture_asked(pool: &PgPool) -> ((AppState, Router, Seen), Seen) {
    let asked: Seen = Arc::default();
    let seen: Seen = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock = Router::new()
        .route("/v1/embeddings", post(embeddings))
        .route(
            "/v1/chat/completions",
            post(completions).with_state(seen.clone()),
        )
        .route("/v1/verdicts", post(verdicts).with_state(asked.clone()));
    tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let pem = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(Default::default())
        .unwrap();
    let gateway = Gateway::new(
        format!("http://{address}").parse().unwrap(),
        pem.as_bytes(),
        "http://localhost".into(),
        "obleth".into(),
    )
    .unwrap();
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: ORIGIN.into(),
            session_hours: 12,
            login_rate_per_minute: 1000,
            ..AuthConfig::default()
        },
    )
    .await
    .unwrap();
    let mut state = AppState::new(auth, pool.clone(), Some(gateway), 4);
    let store = ObjectStore::new(
        format!("http://{address}").parse().unwrap(),
        "bucket".into(),
        "auto".into(),
        "key".into(),
        "secret".into(),
    );
    state.knowledge = Some(Knowledge::new(
        pool.clone(),
        store,
        Tools::from_env(),
        "fixture-embed".into(),
        1024 * 1024,
    ));
    ((state.clone(), memory::router(state), seen), asked)
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

async fn enable(pool: &PgPool) {
    sqlx::query("UPDATE memory_settings SET enabled = TRUE, model = 'keeper' WHERE id")
        .execute(pool)
        .await
        .unwrap();
}

/// A chat with one exchange per message; returns it and its last reply.
async fn chat(pool: &PgPool, owner: Uuid, messages: &[&str], temporary: bool) -> (Uuid, Uuid) {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversations(id,owner_id,title,model,temporary)VALUES($1,$2,'Chat','m',$3)",
    )
    .bind(id)
    .bind(owner)
    .bind(temporary)
    .execute(pool)
    .await
    .unwrap();
    let mut reply = Uuid::nil();
    for text in messages {
        reply = exchange(pool, id, text).await;
    }
    (id, reply)
}

async fn exchange(pool: &PgPool, chat: Uuid, text: &str) -> Uuid {
    let next: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM messages WHERE conversation_id = $1",
    )
    .bind(chat)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status)VALUES($1,$2,$3,'user',$4,'completed')")
        .bind(Uuid::new_v4()).bind(chat).bind(next).bind(text).execute(pool).await.unwrap();
    let reply = Uuid::new_v4();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status,generation_id)VALUES($1,$2,$3,'assistant','Noted.','completed',$4)")
        .bind(reply).bind(chat).bind(next + 1).bind(Uuid::new_v4()).execute(pool).await.unwrap();
    reply
}

/// Waits for a background pass to let go of the chat.
async fn settled(pool: &PgPool, chat: Uuid) {
    for _ in 0..100 {
        let held: Option<bool> = sqlx::query_scalar(
            "SELECT claimed_until IS NOT NULL FROM memory_reads WHERE conversation_id = $1",
        )
        .bind(chat)
        .fetch_optional(pool)
        .await
        .unwrap();
        if held == Some(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the memory pass never finished");
}

async fn contents(pool: &PgPool, owner: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT content FROM memories WHERE owner_id = $1 ORDER BY content")
        .bind(owner)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn replies_teach_memories_linked_to_topics(pool: PgPool) {
    let ((state, _, seen), judged) = fixture_asked(&pool).await;
    let person = session(&pool, "user").await;
    let user = identity(&person);
    let (id, reply) = chat(
        &pool,
        person.id,
        &["I'm a PhD student in hydrology training flood models in PyTorch"],
        false,
    )
    .await;
    // Off on the server: nothing is read.
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    enable(&pool).await;
    assert!(memory::after_reply(&state, &user, id, reply).await);
    settled(&pool, id).await;
    // The repeat, written differently, is kept once; the file name was decided not to last.
    assert_eq!(
        contents(&pool, person.id).await,
        vec![
            "You are a PhD student in hydrology",
            "You train flood models with PyTorch"
        ]
    );
    let topics: Vec<(String, String)> =
        sqlx::query_as("SELECT name, kind FROM memory_topics WHERE owner_id = $1 ORDER BY name")
            .bind(person.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        topics,
        vec![
            ("Flood forecasting".into(), "project".into()),
            ("Hydrology".into(), "field".into()),
            ("PyTorch".into(), "tool".into())
        ]
    );
    let embedded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memories WHERE owner_id = $1 AND embedding IS NOT NULL AND origin = 'learned' AND source_conversation_id = $2",
    )
    .bind(person.id)
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(embedded, 2);
    // The reply shows what was learned.
    let activity: Value = sqlx::query_scalar("SELECT activity FROM messages WHERE id = $1")
        .bind(reply)
        .fetch_one(&pool)
        .await
        .unwrap();
    let learned = activity
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "learned")
        .unwrap();
    assert_eq!(learned["items"].as_array().unwrap().len(), 2);
    assert_eq!(learned["items"][0]["change"], "added");
    // The model read only the person's words as theirs, as their owner.
    let asked = seen.lock().unwrap()[0].clone();
    assert_eq!(asked["model"], "keeper");
    assert!(asked["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("The person's newest messages"));
    // Nothing new since: no second pass.
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    let through: i32 =
        sqlx::query_scalar("SELECT read_through FROM memory_reads WHERE conversation_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(through, 1);
    // Each memory was asked about once, beside the person's words.
    let judged = judged.lock().unwrap().clone();
    assert_eq!(judged.len(), 1);
    assert_eq!(judged[0]["questions"].as_object().unwrap().len(), 4);
    assert!(judged[0]["state"]
        .to_string()
        .contains("PhD student in hydrology"));
}

#[sqlx::test(migrations = "./migrations")]
async fn without_the_memory_check_what_the_model_picks_is_kept(pool: PgPool) {
    let ((state, _, _), asked) = fixture_asked(&pool).await;
    enable(&pool).await;
    sqlx::query("UPDATE decisions SET check_memories = FALSE WHERE id")
        .execute(&pool)
        .await
        .unwrap();
    let person = session(&pool, "user").await;
    let (id, reply) = chat(&pool, person.id, &["I study hydrology"], false).await;
    assert!(memory::after_reply(&state, &identity(&person), id, reply).await);
    settled(&pool, id).await;
    assert!(contents(&pool, person.id)
        .await
        .contains(&"You are using a file named floods.nc".to_owned()));
    assert!(asked.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn without_a_decision_nothing_is_kept_and_the_chat_is_read_later(pool: PgPool) {
    let (state, _, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let (id, reply) = chat(
        &pool,
        person.id,
        &["I study hydrology; the cluster had an outage"],
        false,
    )
    .await;
    assert!(memory::after_reply(&state, &identity(&person), id, reply).await);
    settled(&pool, id).await;
    assert!(contents(&pool, person.id).await.is_empty());
    let (through, failed): (i32, bool) = sqlx::query_as(
        "SELECT read_through, failed_at IS NOT NULL FROM memory_reads WHERE conversation_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((through, failed), (-1, true));
}

#[sqlx::test(migrations = "./migrations")]
async fn temporary_chats_and_people_who_turned_learning_off_are_not_read(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let user = identity(&person);
    let (id, reply) = chat(&pool, person.id, &["I study hydrology"], true).await;
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    let (status, _) = call(
        &app,
        &person,
        "PUT",
        "/api/memory/settings",
        json!({"useMemory": true, "learn": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (id, reply) = chat(&pool, person.id, &["I study hydrology"], false).await;
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    assert!(contents(&pool, person.id).await.is_empty());
    // Off by default for everyone: the person who never chose is not read either.
    sqlx::query("UPDATE memory_settings SET default_on = FALSE WHERE id")
        .execute(&pool)
        .await
        .unwrap();
    let other = session(&pool, "user").await;
    let (id, reply) = chat(&pool, other.id, &["I study hydrology"], false).await;
    assert!(!memory::after_reply(&state, &identity(&other), id, reply).await);
}

#[sqlx::test(migrations = "./migrations")]
async fn new_messages_update_and_forget_but_pinned_memories_stay(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let user = identity(&person);
    for (text, pinned) in [
        ("You use TensorFlow", false),
        ("Always answer in French", true),
    ] {
        let (status, _) = call(
            &app,
            &person,
            "POST",
            "/api/memory",
            json!({"content": text, "kind": "preference", "pinned": pinned, "topics": ["Machine learning"]}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (id, reply) = chat(&pool, person.id, &["I switched our lab to PyTorch"], false).await;
    assert!(memory::after_reply(&state, &user, id, reply).await);
    settled(&pool, id).await;
    assert_eq!(
        contents(&pool, person.id).await,
        vec!["Always answer in French", "You use PyTorch, not TensorFlow"]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn replies_get_pinned_memories_and_the_ones_that_fit(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let user = identity(&person);
    for (text, pinned) in [
        ("You are a PhD student in hydrology", false),
        ("You bake sourdough on weekends", false),
        ("You like short answers", true),
    ] {
        call(
            &app,
            &person,
            "POST",
            "/api/memory",
            json!({"content": text, "kind": "about", "pinned": pinned, "topics": []}),
        )
        .await;
    }
    let gateway = state.gateway.clone().unwrap();
    let (id, _) = chat(&pool, person.id, &[], false).await;
    let messages = vec![(
        "user".to_owned(),
        "Which hydrology dataset should I start with?".to_owned(),
    )];
    let found = memory::recall(&state, &gateway, &user, id, &messages)
        .await
        .unwrap();
    let prompt = found.prompt();
    assert!(prompt.contains("- You like short answers"));
    assert!(prompt.contains("- You are a PhD student in hydrology"));
    assert!(!prompt.contains("sourdough"));
    let entry = found.entry();
    assert_eq!(entry["kind"], "recalled");
    assert_eq!(entry["items"].as_array().unwrap().len(), 2);
    let uses: i32 =
        sqlx::query_scalar("SELECT uses FROM memories WHERE content LIKE '%hydrology%'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(uses, 1);
    // Not in a temporary chat, and not for a person who turned memory off.
    let (temporary, _) = chat(&pool, person.id, &[], true).await;
    assert!(
        memory::recall(&state, &gateway, &user, temporary, &messages)
            .await
            .is_none()
    );
    call(
        &app,
        &person,
        "PUT",
        "/api/memory/settings",
        json!({"useMemory": false, "learn": true}),
    )
    .await;
    assert!(memory::recall(&state, &gateway, &user, id, &messages)
        .await
        .is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn people_see_change_and_delete_their_memories(pool: PgPool) {
    let (_, app, _) = fixture(&pool).await;
    let person = session(&pool, "user").await;
    let other = session(&pool, "user").await;
    let admin = session(&pool, "admin").await;
    // Off on the server: the page says so, and nothing can be added.
    let (_, page) = call(&app, &person, "GET", "/api/memory", Value::Null).await;
    assert_eq!(page["available"], false);
    let (status, _) = call(
        &app,
        &person,
        "POST",
        "/api/memory",
        json!({"content": "You teach CHM 501"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &app,
        &person,
        "PUT",
        "/api/admin/memory",
        json!({"enabled": true, "defaultOn": true, "model": "keeper"}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, saved) = call(
        &app,
        &admin,
        "PUT",
        "/api/admin/memory",
        json!({"enabled": true, "defaultOn": true, "model": ""}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{saved}");
    let (status, saved) = call(
        &app,
        &admin,
        "PUT",
        "/api/admin/memory",
        json!({"enabled": true, "defaultOn": true, "model": "keeper"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["model"], "keeper");
    // Added without topics: the model files it.
    let (status, added) = call(
        &app,
        &person,
        "POST",
        "/api/memory",
        json!({"content": "  You teach   CHM 501 this spring "}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let memory = added["memory"]["id"].as_str().unwrap().to_owned();
    assert_eq!(added["memory"]["content"], "You teach CHM 501 this spring");
    assert_eq!(added["memory"]["origin"], "added");
    let mut page = Value::Null;
    for _ in 0..100 {
        page = call(&app, &person, "GET", "/api/memory", Value::Null)
            .await
            .1;
        if page["topics"].as_array().is_some_and(|t| !t.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(page["available"], true);
    assert_eq!(page["memories"][0]["kind"], "work");
    assert_eq!(page["topics"][0]["name"], "CHM 501");
    assert_eq!(page["topics"][0]["kind"], "course");
    assert_eq!(page["topics"][0]["count"], 1);
    // Another person can't touch it.
    let (status, _) = call(
        &app,
        &other,
        "PATCH",
        &format!("/api/memory/{memory}"),
        json!({"pinned": true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Pin it, and move it to other topics; the old topic goes with its last memory.
    let (status, changed) = call(
        &app,
        &person,
        "PATCH",
        &format!("/api/memory/{memory}"),
        json!({"pinned": true, "content": "You teach CHM 501 and CHM 502", "topics": ["Teaching", "Chemistry"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(changed["memory"]["pinned"], true);
    assert_eq!(changed["memory"]["topics"].as_array().unwrap().len(), 2);
    let (_, page) = call(&app, &person, "GET", "/api/memory", Value::Null).await;
    let names: Vec<&str> = page["topics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Chemistry", "Teaching"]);
    // Renaming a topic to another's name merges them.
    let teaching = page["topics"][1]["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &app,
        &person,
        "PATCH",
        &format!("/api/memory/topics/{teaching}"),
        json!({"name": "chemistry"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, page) = call(&app, &person, "GET", "/api/memory", Value::Null).await;
    assert_eq!(page["topics"].as_array().unwrap().len(), 1);
    // Forgetting everything about a topic deletes its memories.
    let topic = page["topics"][0]["id"].as_str().unwrap().to_owned();
    let (status, gone) = call(
        &app,
        &person,
        "DELETE",
        &format!("/api/memory/topics/{topic}?memories=true"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gone["forgotten"], 1);
    assert!(contents(&pool, person.id).await.is_empty());
    // Delete one, then all.
    call(
        &app,
        &person,
        "POST",
        "/api/memory",
        json!({"content": "You use R", "kind": "work", "topics": ["R"]}),
    )
    .await;
    let (_, page) = call(&app, &person, "GET", "/api/memory", Value::Null).await;
    let id = page["memories"][0]["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &app,
        &person,
        "DELETE",
        &format!("/api/memory/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for text in ["You use Julia", "You use Rust"] {
        call(
            &app,
            &person,
            "POST",
            "/api/memory",
            json!({"content": text, "kind": "work", "topics": ["Languages"]}),
        )
        .await;
    }
    let (status, _) = call(&app, &person, "DELETE", "/api/memory", Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, page) = call(&app, &person, "GET", "/api/memory", Value::Null).await;
    assert!(page["memories"].as_array().unwrap().is_empty());
    assert!(page["topics"].as_array().unwrap().is_empty());
}
