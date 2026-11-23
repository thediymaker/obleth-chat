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
    knowledge::{chat::Question, extract::Tools, Knowledge, ObjectStore},
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
/// Every text the embeddings fixture was asked for, across the tests in this file.
static EMBEDDED: Mutex<Vec<String>> = Mutex::new(Vec::new());

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
    let input = body["input"].as_array().unwrap();
    EMBEDDED
        .lock()
        .unwrap()
        .extend(input.iter().map(|t| t.as_str().unwrap().to_owned()));
    let data: Vec<Value> = input
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
    } else if input.contains("evaluation split") {
        // Two by their kind, two by the scope the model gave.
        json!({"add":[
            {"text":"You evaluate on the 2024 held-out split","kind":"work","topics":[]},
            {"text":"You prefer answers with tables","kind":"preference","topics":[]},
            {"text":"You write all your code in Python","kind":"work","scope":"everywhere","topics":[]},
            {"text":"You report results to the funding agency","kind":"about","scope":"project","topics":[]}
        ],"update":[],"forget":[]})
        .to_string()
    } else if input.contains("baseline is gone") {
        let id = input
            .lines()
            .find(|l| l.contains("baseline"))
            .and_then(|l| l.split(':').next())
            .unwrap_or("m9")
            .to_owned();
        json!({"add":[{"text":"You compare against the new baseline","kind":"work","topics":[]}],
            "update":[{"id":id.clone(),"text":"You dropped the old baseline"}],"forget":[id]})
        .to_string()
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

/// Reads the chats that wait, as if they had been quiet long enough.
async fn quiet(state: &AppState) -> usize {
    memory::read_quiet(state, Duration::ZERO).await.unwrap()
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
    assert_eq!(quiet(&state).await, 0);
    enable(&pool).await;
    // Nothing asked to be remembered: the chat is read once it is quiet.
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    assert_eq!(quiet(&state).await, 1);
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
    assert_eq!(quiet(&state).await, 0);
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
async fn a_request_to_remember_is_read_at_once_and_other_chats_wait_until_quiet(pool: PgPool) {
    let (state, _, seen) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let user = identity(&person);
    let (asked, reply) = chat(
        &pool,
        person.id,
        &["Remember that I study hydrology"],
        false,
    )
    .await;
    assert!(memory::after_reply(&state, &user, asked, reply).await);
    settled(&pool, asked).await;
    assert_eq!(seen.lock().unwrap().len(), 1);
    // An ordinary chat is not read after its reply, nor before it has been quiet.
    let (id, reply) = chat(&pool, person.id, &["I study hydrology"], false).await;
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    assert_eq!(memory::read_quiet(&state, memory::QUIET).await.unwrap(), 0);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(quiet(&state).await, 1);
    assert_eq!(seen.lock().unwrap().len(), 2);
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
    chat(&pool, person.id, &["I study hydrology"], false).await;
    assert_eq!(quiet(&state).await, 1);
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
    let (id, _) = chat(
        &pool,
        person.id,
        &["I study hydrology; the cluster had an outage"],
        false,
    )
    .await;
    assert_eq!(quiet(&state).await, 1);
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
    let (id, reply) = chat(&pool, person.id, &["Remember that I study hydrology"], true).await;
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    assert_eq!(quiet(&state).await, 0);
    let (status, _) = call(
        &app,
        &person,
        "PUT",
        "/api/memory/settings",
        json!({"useMemory": true, "learn": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (id, reply) = chat(
        &pool,
        person.id,
        &["Remember that I study hydrology"],
        false,
    )
    .await;
    assert!(!memory::after_reply(&state, &user, id, reply).await);
    assert_eq!(quiet(&state).await, 0);
    assert!(contents(&pool, person.id).await.is_empty());
    // Off by default for everyone: the person who never chose is not read either.
    sqlx::query("UPDATE memory_settings SET default_on = FALSE WHERE id")
        .execute(&pool)
        .await
        .unwrap();
    let other = session(&pool, "user").await;
    let (id, reply) = chat(&pool, other.id, &["Remember that I study hydrology"], false).await;
    assert!(!memory::after_reply(&state, &identity(&other), id, reply).await);
    assert_eq!(quiet(&state).await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn new_messages_update_and_forget_but_pinned_memories_stay(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
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
    chat(&pool, person.id, &["I switched our lab to PyTorch"], false).await;
    assert_eq!(quiet(&state).await, 1);
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
    let question = Question::new(&[(
        "user".to_owned(),
        "Which hydrology dataset should I start with?".to_owned(),
    )]);
    let found = memory::recall(&state, &gateway, &user, id, &question)
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
        memory::recall(&state, &gateway, &user, temporary, &question)
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
    // Chat still learns from the chat, so the reply is told how remembering works,
    // with nothing recalled.
    let recall = memory::recall(&state, &gateway, &user, id, &question)
        .await
        .unwrap();
    assert!(!recall.recalled());
}

/// How many texts the embeddings fixture was asked for that contain `marker`.
fn embedded(marker: &str) -> usize {
    EMBEDDED
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.contains(marker))
        .count()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_turn_embeds_its_question_once_for_knowledge_and_memory(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let gateway = state.gateway.clone().unwrap();
    let knowledge = state.knowledge.clone().unwrap();
    // Without memories or collections, nothing is embedded.
    let newcomer = session(&pool, "user").await;
    let (empty, _) = chat(&pool, newcomer.id, &[], false).await;
    let marker = format!("watershed-{}", Uuid::new_v4().simple());
    let question = Question::new(&[("user".to_owned(), format!("Which {marker} model?"))]);
    let user = identity(&newcomer);
    let (turn, recalled) = tokio::join!(
        knowledge.prepare(
            &gateway,
            &user,
            empty,
            &[],
            &question,
            true,
            false,
            None,
            None
        ),
        memory::recall(&state, &gateway, &user, empty, &question)
    );
    assert!(turn.is_none() && !recalled.is_some_and(|r| r.recalled()));
    assert_eq!(embedded(&marker), 0);
    // With both, the question is embedded once and both use it.
    let person = session(&pool, "user").await;
    call(
        &app,
        &person,
        "POST",
        "/api/memory",
        json!({"content": "You study the hydrology of a watershed", "kind": "about", "pinned": false, "topics": []}),
    )
    .await;
    let collection = Uuid::new_v4();
    sqlx::query("INSERT INTO knowledge_collections (id, owner_id, name) VALUES ($1, $2, 'Papers')")
        .bind(collection)
        .bind(person.id)
        .execute(&pool)
        .await
        .unwrap();
    let (id, _) = chat(&pool, person.id, &[], false).await;
    let marker = format!("watershed-{}", Uuid::new_v4().simple());
    let question = Question::new(&[(
        "user".to_owned(),
        format!("What changes the hydrology of a {marker} watershed?"),
    )]);
    let user = identity(&person);
    let attached = [collection];
    let (turn, recalled) = tokio::join!(
        knowledge.prepare(&gateway, &user, id, &attached, &question, true, false, None, None),
        memory::recall(&state, &gateway, &user, id, &question)
    );
    assert!(turn.is_some());
    assert!(recalled
        .unwrap()
        .prompt()
        .contains("hydrology of a watershed"));
    assert_eq!(embedded(&marker), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_embedding_backlog_waits_while_memory_is_off_and_uses_its_index(pool: PgPool) {
    let (state, _, _) = fixture(&pool).await;
    let gateway = state.gateway.clone().unwrap();
    let knowledge = state.knowledge.clone().unwrap();
    let person = session(&pool, "user").await;
    sqlx::query("INSERT INTO memories (id, owner_id, kind, content, origin) VALUES ($1, $2, 'about', 'You map river basins', 'added')")
        .bind(Uuid::new_v4())
        .bind(person.id)
        .execute(&pool)
        .await
        .unwrap();
    let attempts = || async {
        sqlx::query_scalar::<_, i16>("SELECT embedding_attempts FROM memories WHERE owner_id = $1")
            .bind(person.id)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    // Memory is off: the worker leaves the backlog alone.
    assert_eq!(
        memory::embed_missing(&pool, &knowledge, &gateway)
            .await
            .unwrap(),
        0
    );
    assert_eq!(attempts().await, 0);
    enable(&pool).await;
    assert_eq!(
        memory::embed_missing(&pool, &knowledge, &gateway)
            .await
            .unwrap(),
        1
    );
    let embedded: bool =
        sqlx::query_scalar("SELECT embedding IS NOT NULL FROM memories WHERE owner_id = $1")
            .bind(person.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(embedded);
    // The backlog query can use its partial index.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN SELECT id FROM memories WHERE embedding IS NULL AND embedding_attempts < 3 ORDER BY created_at LIMIT 16",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert!(plan.join("\n").contains("memories_pending_idx"), "{plan:?}");
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
    let (status, refused) = call(
        &app,
        &person,
        "POST",
        "/api/memory",
        json!({"content": "You teach CHM 501"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused["error"]["code"], "feature_off");
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

/// A project of `owner`'s, shared with a new group that `members` are in when given.
async fn project(pool: &PgPool, owner: Uuid, name: &str, members: &[Uuid]) -> Uuid {
    let id = Uuid::new_v4();
    let group = if members.is_empty() {
        None
    } else {
        let group = Uuid::new_v4();
        sqlx::query("INSERT INTO user_groups(id,name) VALUES($1,$2)")
            .bind(group)
            .bind(format!("Lab {}", group.simple()))
            .execute(pool)
            .await
            .unwrap();
        for member in members {
            sqlx::query("INSERT INTO user_group_members(group_id,user_id) VALUES($1,$2)")
                .bind(group)
                .bind(member)
                .execute(pool)
                .await
                .unwrap();
        }
        Some(group)
    };
    sqlx::query("INSERT INTO projects(id,owner_id,name,color,scope,group_id) VALUES($1,$2,$3,'green',$4,$5)")
        .bind(id)
        .bind(owner)
        .bind(name)
        .bind(if group.is_some() { "group" } else { "personal" })
        .bind(group)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// A chat of `owner`'s in `project`, with one exchange per message.
async fn project_chat(
    pool: &PgPool,
    owner: Uuid,
    project: Uuid,
    messages: &[&str],
) -> (Uuid, Uuid) {
    let (id, _) = chat(pool, owner, &[], false).await;
    sqlx::query("UPDATE conversations SET project_id = $2 WHERE id = $1")
        .bind(id)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    let mut reply = Uuid::nil();
    for text in messages {
        reply = exchange(pool, id, text).await;
    }
    (id, reply)
}

/// The system prompt and input of each learning pass the model was asked for, in order.
fn passes(seen: &Seen) -> Vec<(String, String)> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|b| {
            b["messages"][0]["content"]
                .as_str()
                .unwrap_or("")
                .starts_with("You keep the memory")
        })
        .map(|b| {
            (
                b["messages"][0]["content"].as_str().unwrap().to_owned(),
                b["messages"][1]["content"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Adds a memory by hand; returns its id.
async fn remember(app: &Router, user: &Session, body: Value) -> String {
    let (status, added) = call(app, user, "POST", "/api/memory", body).await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    added["memory"]["id"].as_str().unwrap().to_owned()
}

/// The memory texts a reply in `chat` is given for `text`.
async fn recalled(state: &AppState, user: &Session, chat: Uuid, text: &str) -> Vec<String> {
    let gateway = state.gateway.clone().unwrap();
    let question = Question::new(&[("user".to_owned(), text.to_owned())]);
    let found = memory::recall(state, &gateway, &identity(user), chat, &question).await;
    let mut texts: Vec<String> = found
        .map(|r| {
            r.entry()["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["text"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default();
    texts.sort();
    texts
}

async fn project_of(pool: &PgPool, memory: &str) -> Option<Uuid> {
    sqlx::query_scalar("SELECT project_id FROM memories WHERE content = $1")
        .bind(memory)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_chat_in_a_project_gets_general_memories_and_that_projects_only(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let rivers = project(&pool, person.id, "Rivers", &[]).await;
    let glaciers = project(&pool, person.id, "Glaciers", &[]).await;
    for (text, pinned, project) in [
        ("You like short answers", true, None),
        ("You study hydrology at a university", false, None),
        (
            "Rivers is pinned: use the gauge dataset",
            true,
            Some(rivers),
        ),
        (
            "Rivers hydrology uses the 2024 gauge split",
            false,
            Some(rivers),
        ),
        (
            "Glaciers is pinned: use the ice cores",
            true,
            Some(glaciers),
        ),
        ("Glaciers hydrology uses melt radar", false, Some(glaciers)),
    ] {
        remember(
            &app,
            &person,
            json!({"content": text, "kind": "work", "pinned": pinned, "topics": [], "projectId": project}),
        )
        .await;
    }
    let ask = "Which hydrology dataset should I use?";
    let (in_rivers, _) = project_chat(&pool, person.id, rivers, &[]).await;
    assert_eq!(
        recalled(&state, &person, in_rivers, ask).await,
        vec![
            "Rivers hydrology uses the 2024 gauge split",
            "Rivers is pinned: use the gauge dataset",
            "You like short answers",
            "You study hydrology at a university",
        ]
    );
    let (in_glaciers, _) = project_chat(&pool, person.id, glaciers, &[]).await;
    assert_eq!(
        recalled(&state, &person, in_glaciers, ask).await,
        vec![
            "Glaciers hydrology uses melt radar",
            "Glaciers is pinned: use the ice cores",
            "You like short answers",
            "You study hydrology at a university",
        ]
    );
    let (outside, _) = chat(&pool, person.id, &[], false).await;
    assert_eq!(
        recalled(&state, &person, outside, ask).await,
        vec![
            "You like short answers",
            "You study hydrology at a university"
        ]
    );
    // Only general ones besides pinned: no unpinned general memory left still finds
    // the project's unpinned one in its project.
    let general: String =
        sqlx::query_scalar("SELECT id::text FROM memories WHERE content LIKE 'You study%'")
            .fetch_one(&pool)
            .await
            .unwrap();
    call(
        &app,
        &person,
        "DELETE",
        &format!("/api/memory/{general}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        recalled(&state, &person, outside, ask).await,
        vec!["You like short answers"]
    );
    assert!(recalled(&state, &person, in_rivers, ask)
        .await
        .contains(&"Rivers hydrology uses the 2024 gauge split".to_owned()));
}

#[sqlx::test(migrations = "./migrations")]
async fn learning_in_a_project_chat_keeps_work_in_the_project_and_the_person_everywhere(
    pool: PgPool,
) {
    let (state, _, seen) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let user = identity(&person);
    let rivers = project(&pool, person.id, "Rivers", &[]).await;
    let (id, reply) = project_chat(
        &pool,
        person.id,
        rivers,
        &["Remember that the evaluation split is the 2024 held-out one"],
    )
    .await;
    assert!(memory::after_reply(&state, &user, id, reply).await);
    settled(&pool, id).await;
    // By kind: work stays, a preference goes everywhere; the model's scope wins over kind.
    assert_eq!(
        project_of(&pool, "You evaluate on the 2024 held-out split").await,
        Some(rivers)
    );
    assert_eq!(
        project_of(&pool, "You prefer answers with tables").await,
        None
    );
    assert_eq!(
        project_of(&pool, "You write all your code in Python").await,
        None
    );
    assert_eq!(
        project_of(&pool, "You report results to the funding agency").await,
        Some(rivers)
    );
    // The model was told the chat's project and asked for each memory's scope.
    let system = passes(&seen)[0].0.clone();
    assert!(system.contains("project \"Rivers\""), "{system}");
    assert!(system.contains("\"scope\": \"project\""), "{system}");
    // The reply's card says where each went and which project it may go to.
    let activity: Value = sqlx::query_scalar("SELECT activity FROM messages WHERE id = $1")
        .bind(reply)
        .fetch_one(&pool)
        .await
        .unwrap();
    let items = activity
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "learned")
        .unwrap()["items"]
        .clone();
    let item = |text: &str| {
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["text"] == text)
            .unwrap()
            .clone()
    };
    let work = item("You evaluate on the 2024 held-out split");
    assert_eq!(work["projectId"], json!(rivers));
    assert_eq!(work["chatProjectId"], json!(rivers));
    let preference = item("You prefer answers with tables");
    assert_eq!(preference["projectId"], Value::Null);
    assert_eq!(preference["chatProjectId"], json!(rivers));

    // Outside a project everything is general, the card offers no choice and the model
    // is not asked for a scope.
    let other = session(&pool, "user").await;
    let (id, reply) = chat(
        &pool,
        other.id,
        &["Remember that the evaluation split is the 2024 one"],
        false,
    )
    .await;
    assert!(memory::after_reply(&state, &identity(&other), id, reply).await);
    settled(&pool, id).await;
    let scoped: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memories WHERE owner_id = $1 AND project_id IS NOT NULL",
    )
    .bind(other.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(scoped, 0);
    assert_eq!(contents(&pool, other.id).await.len(), 4);
    let system = passes(&seen)[1].0.clone();
    assert!(!system.contains("scope"), "{system}");
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
    assert!(learned["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i.get("chatProjectId").is_none()));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_chat_in_one_project_never_updates_or_forgets_another_projects_memory(pool: PgPool) {
    let (state, app, seen) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let rivers = project(&pool, person.id, "Rivers", &[]).await;
    let glaciers = project(&pool, person.id, "Glaciers", &[]).await;
    remember(
        &app,
        &person,
        json!({"content": "You compare against the old baseline", "kind": "work", "topics": [], "projectId": rivers}),
    )
    .await;
    // In Glaciers the Rivers memory is not shown to the model, so it is neither updated
    // nor forgotten; the new one is not a repeat of it either.
    let (id, _) = project_chat(&pool, person.id, glaciers, &["The old baseline is gone"]).await;
    assert_eq!(quiet(&state).await, 1);
    let shown = passes(&seen)[0].1.clone();
    assert!(
        !shown.contains("You compare against the old baseline"),
        "{shown}"
    );
    assert_eq!(
        project_of(&pool, "You compare against the old baseline").await,
        Some(rivers)
    );
    assert_eq!(
        project_of(&pool, "You compare against the new baseline").await,
        Some(glaciers)
    );
    settled(&pool, id).await;
    // In Rivers it is: updated and then forgotten in the same pass, so forgotten.
    project_chat(&pool, person.id, rivers, &["The old baseline is gone"]).await;
    assert_eq!(quiet(&state).await, 1);
    let shown = passes(&seen)[1].1.clone();
    assert!(
        shown.contains("You compare against the old baseline"),
        "{shown}"
    );
    assert!(!shown.contains("new baseline"), "{shown}");
    let left: Vec<String> = contents(&pool, person.id).await;
    assert!(
        !left.contains(&"You compare against the old baseline".to_owned()),
        "{left:?}"
    );
    // The Glaciers memory is still there, and Rivers got its own copy.
    let copies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memories WHERE content = 'You compare against the new baseline'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(copies, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn memories_are_added_to_and_moved_between_projects_the_person_may_use(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let colleague = session(&pool, "user").await;
    let stranger = session(&pool, "user").await;
    let mine = project(&pool, person.id, "Rivers", &[]).await;
    let theirs = project(&pool, stranger.id, "Private work", &[]).await;
    // The colleague's project, shared with a group the person is in.
    let shared = project(&pool, colleague.id, "Lab paper", &[person.id]).await;
    // A project the person may not use is not found.
    let (status, _) = call(
        &app,
        &person,
        "POST",
        "/api/memory",
        json!({"content": "You use radar", "projectId": theirs}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, added) = call(
        &app,
        &person,
        "POST",
        "/api/memory",
        json!({"content": "You use radar", "kind": "work", "topics": [], "projectId": mine}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let m = &added["memory"];
    assert_eq!(
        (
            m["projectId"].clone(),
            m["projectName"].clone(),
            m["projectColor"].clone()
        ),
        (json!(mine), json!("Rivers"), json!("green"))
    );
    assert_eq!(m["projectUnavailable"], false);
    let id = m["id"].as_str().unwrap().to_owned();
    let path = format!("/api/memory/{id}");
    // A change without projectId leaves it where it is.
    let (_, changed) = call(&app, &person, "PATCH", &path, json!({"pinned": true})).await;
    assert_eq!(changed["memory"]["projectId"], json!(mine));
    // Moved to a project the person may not use: not found, and nothing changes.
    let (status, _) = call(&app, &person, "PATCH", &path, json!({"projectId": theirs})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Someone else can't move it.
    let (status, _) = call(&app, &stranger, "PATCH", &path, json!({"projectId": null})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Everywhere, then into the shared project.
    let (_, changed) = call(&app, &person, "PATCH", &path, json!({"projectId": null})).await;
    assert_eq!(changed["memory"]["projectId"], Value::Null);
    assert_eq!(changed["memory"]["projectUnavailable"], false);
    let (status, changed) = call(&app, &person, "PATCH", &path, json!({"projectId": shared})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(changed["memory"]["projectName"], "Lab paper");
    // The project's owner sees no memory of the person's, and is never given it.
    let (_, page) = call(&app, &colleague, "GET", "/api/memory", Value::Null).await;
    assert!(page["memories"].as_array().unwrap().is_empty());
    let (theirs_in_shared, _) = project_chat(&pool, colleague.id, shared, &[]).await;
    assert!(
        recalled(&state, &colleague, theirs_in_shared, "Do I use radar?")
            .await
            .is_empty()
    );
    let (mine_in_shared, _) = project_chat(&pool, person.id, shared, &[]).await;
    assert_eq!(
        recalled(&state, &person, mine_in_shared, "Do I use radar?").await,
        vec!["You use radar"]
    );
    // After leaving the group the memory is still listed, in no project they can use,
    // given to no chat, and can be moved out.
    sqlx::query("DELETE FROM user_group_members WHERE user_id = $1")
        .bind(person.id)
        .execute(&pool)
        .await
        .unwrap();
    let (_, page) = call(&app, &person, "GET", "/api/memory", Value::Null).await;
    let listed = &page["memories"][0];
    assert_eq!(listed["projectId"], Value::Null);
    assert_eq!(listed["projectName"], Value::Null);
    assert_eq!(listed["projectUnavailable"], true);
    assert!(recalled(&state, &person, mine_in_shared, "Do I use radar?")
        .await
        .is_empty());
    let (outside, _) = chat(&pool, person.id, &[], false).await;
    assert!(recalled(&state, &person, outside, "Do I use radar?")
        .await
        .is_empty());
    let (status, _) = call(&app, &person, "PATCH", &path, json!({"projectId": shared})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, changed) = call(&app, &person, "PATCH", &path, json!({"projectId": mine})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(changed["memory"]["projectUnavailable"], false);
}

#[sqlx::test(migrations = "./migrations")]
async fn deleting_a_project_makes_its_memories_general(pool: PgPool) {
    let (state, app, _) = fixture(&pool).await;
    enable(&pool).await;
    let person = session(&pool, "user").await;
    let rivers = project(&pool, person.id, "Rivers", &[]).await;
    remember(
        &app,
        &person,
        json!({"content": "You use radar", "kind": "work", "topics": [], "projectId": rivers}),
    )
    .await;
    let (outside, _) = chat(&pool, person.id, &[], false).await;
    assert!(recalled(&state, &person, outside, "Do I use radar?")
        .await
        .is_empty());
    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(rivers)
        .execute(&pool)
        .await
        .unwrap();
    let (_, page) = call(&app, &person, "GET", "/api/memory", Value::Null).await;
    assert_eq!(page["memories"][0]["projectId"], Value::Null);
    assert_eq!(page["memories"][0]["projectUnavailable"], false);
    assert_eq!(
        recalled(&state, &person, outside, "Do I use radar?").await,
        vec!["You use radar"]
    );
}
