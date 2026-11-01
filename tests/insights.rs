//! Usage insights: quiet chats are labelled once as their owner, temporary chats never,
//! personal chats keep no topic line, and administrators see counts and workstreams but
//! never a chat. Requires a disposable DATABASE_URL.
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
    auth::{Auth, AuthConfig},
    gateway::Gateway,
    insights, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";

struct Session {
    id: Uuid,
    cookie: String,
    csrf: String,
}

async fn session(pool: &PgPool, role: &str) -> Session {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Insights test',$3)")
        .bind(id)
        .bind(format!("insights-{}", id.simple()))
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

/// A gateway that labels a chat mentioning a diary as personal and anything else as
/// research, names every task the same, and finds one workstream.
async fn gateway(asked: Arc<AtomicUsize>) -> Gateway {
    async fn verdicts(
        State(asked): State<Arc<AtomicUsize>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        asked.fetch_add(1, Ordering::SeqCst);
        if body["questions"]["workstream"].is_object() {
            return Json(json!({"verdicts": {"workstream": {"value": "w00", "confidence": 0.99}}}));
        }
        let text = body["state"]["user_messages"].to_string();
        let kind = if text.contains("diary") {
            "personal"
        } else {
            "research"
        };
        Json(json!({"model": "judge", "verdicts": {
            "use": {"value": kind}, "activity": {"value": "data_analysis"}, "field": {"value": "life_health"},
            "output_type": {"value": "analysis"},
            "seeks_harm": {"value": false, "probabilities": {"true": 0.01, "false": 0.99}},
            "integrity": {"value": false, "probabilities": {"true": 0.2, "false": 0.8}},
        }}))
    }
    async fn completions(Json(body): Json<Value>) -> Json<Value> {
        let content = if body["messages"][0]["content"]
            .as_str()
            .unwrap_or("")
            .contains("workstream list")
        {
            "<think>grouping</think>[{\"name\": \"Analyzing protein folding experiments\", \"description\": \"Lab data analysis\"}]"
        } else {
            "\"Analyzing protein folding data.\""
        };
        Json(json!({"choices": [{"message": {"role": "assistant", "content": content}}]}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock = Router::new()
        .route("/v1/verdicts", post(verdicts).with_state(asked))
        .route("/v1/chat/completions", post(completions));
    tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let pem = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(Default::default())
        .unwrap();
    Gateway::new(
        format!("http://{address}").parse().unwrap(),
        pem.as_bytes(),
        "http://localhost".into(),
        "obleth".into(),
    )
    .unwrap()
}

async fn fixture(pool: &PgPool, asked: Arc<AtomicUsize>) -> (AppState, Router) {
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
    let state = AppState::new(auth, pool.clone(), Some(gateway(asked).await), 4);
    (state.clone(), insights::router(state))
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

/// A chat whose person's messages were sent `minutes` ago.
async fn chat(
    pool: &PgPool,
    owner: Uuid,
    messages: &[&str],
    minutes: i32,
    temporary: bool,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversations(id,owner_id,title,model,temporary,updated_at)
         VALUES($1,$2,'Chat','judge',$3,NOW()-make_interval(mins=>$4))",
    )
    .bind(id)
    .bind(owner)
    .bind(temporary)
    .bind(minutes)
    .execute(pool)
    .await
    .unwrap();
    for (ordinal, text) in messages.iter().enumerate() {
        message(pool, id, ordinal as i32, text, minutes).await;
    }
    id
}

async fn message(pool: &PgPool, chat: Uuid, ordinal: i32, text: &str, minutes: i32) {
    sqlx::query(
        "INSERT INTO messages(id,conversation_id,ordinal,role,content,status,created_at)
         VALUES($1,$2,$3,'user',$4,'completed',NOW()-make_interval(mins=>$5))",
    )
    .bind(Uuid::new_v4())
    .bind(chat)
    .bind(ordinal)
    .bind(text)
    .bind(minutes)
    .execute(pool)
    .await
    .unwrap();
}

async fn turn_on(app: &Router, admin: &Session) {
    let (status, body) = call(
        app,
        admin,
        "PUT",
        "/api/admin/insights/settings",
        json!({"enabled": true, "model": "judge", "reportModel": null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrations = "./migrations")]
async fn quiet_chats_are_labelled_once_and_temporary_ones_never(pool: PgPool) {
    let asked = Arc::new(AtomicUsize::new(0));
    let (state, app) = fixture(&pool, asked.clone()).await;
    let admin = session(&pool, "admin").await;
    let person = session(&pool, "user").await;
    let research = chat(
        &pool,
        person.id,
        &["Fit a model to my folding assay results"],
        120,
        false,
    )
    .await;
    let personal = chat(
        &pool,
        person.id,
        &["Help me write my diary entry"],
        120,
        false,
    )
    .await;
    let secret = chat(
        &pool,
        person.id,
        &["Why does this fail? token = 'hf_abcdefghijklmnopqrstuvwxyz0123'"],
        120,
        false,
    )
    .await;
    let temporary = chat(&pool, person.id, &["A temporary question"], 120, true).await;
    let recent = chat(&pool, person.id, &["Still talking"], 5, false).await;

    // Off by default: nothing is read.
    assert_eq!(insights::tick(&state).await.unwrap(), 0);
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    turn_on(&app, &admin).await;
    assert_eq!(insights::tick(&state).await.unwrap(), 3);

    let row = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Option<String>, Option<String>, Vec<String>, Option<chrono::DateTime<chrono::Utc>>)>(
                "SELECT kind, topic, flags, labeled_at FROM conversation_labels WHERE conversation_id=$1",
            )
            .bind(id)
            .fetch_optional(&pool)
            .await
            .unwrap()
        }
    };
    let (kind, topic, flags, _) = row(research).await.unwrap();
    assert_eq!(kind.as_deref(), Some("research"));
    assert_eq!(topic.as_deref(), Some("Analyzing protein folding data"));
    assert!(flags.is_empty());
    let (kind, topic, _, _) = row(personal).await.unwrap();
    assert_eq!(kind.as_deref(), Some("personal"));
    assert_eq!(topic, None, "a personal chat keeps no topic line");
    assert_eq!(row(secret).await.unwrap().2, vec!["secrets"]);
    assert!(row(temporary).await.is_none());
    assert!(row(recent).await.is_none());

    // Labelled chats wait for a new message.
    assert_eq!(insights::tick(&state).await.unwrap(), 0);
    message(&pool, research, 1, "And plot the residuals", 40).await;
    assert_eq!(insights::tick(&state).await.unwrap(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn administrators_see_counts_and_workstreams_never_chats(pool: PgPool) {
    let asked = Arc::new(AtomicUsize::new(0));
    let (state, app) = fixture(&pool, asked).await;
    let admin = session(&pool, "admin").await;
    let person = session(&pool, "user").await;
    let (status, _) = call(&app, &person, "GET", "/api/admin/insights", Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    turn_on(&app, &admin).await;
    chat(
        &pool,
        person.id,
        &["Fit a model to my folding assay results"],
        120,
        false,
    )
    .await;
    chat(&pool, person.id, &["Plot the folding curves"], 90, false).await;
    chat(
        &pool,
        person.id,
        &["Help me write my diary entry"],
        120,
        false,
    )
    .await;
    assert_eq!(insights::tick(&state).await.unwrap(), 3);

    let (status, summary) = call(
        &app,
        &admin,
        "GET",
        "/api/admin/insights?days=28",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["sessions"], 3);
    assert_eq!(summary["people"], 1);
    assert_eq!(
        summary["kinds"][0],
        json!({"name": "research", "sessions": 2, "tokens": 0})
    );
    assert!(summary["report"].is_null());
    let text = summary.to_string();
    assert!(
        !text.contains("folding assay") && !text.contains("diary"),
        "{text}"
    );

    // A group of one shows totals only, and gets no workstreams.
    let group = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups(id,name) VALUES($1,'Lab')")
        .bind(group)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_group_members(group_id,user_id) VALUES($1,$2)")
        .bind(group)
        .bind(person.id)
        .execute(&pool)
        .await
        .unwrap();
    let (_, small) = call(
        &app,
        &admin,
        "GET",
        &format!("/api/admin/insights?days=28&group={group}"),
        Value::Null,
    )
    .await;
    assert_eq!(small["tooFew"], true);
    assert!(small["kinds"].is_null());
    let (status, _) = call(
        &app,
        &admin,
        "POST",
        "/api/admin/insights/reports",
        json!({"days": 28, "group": group}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, started) = call(
        &app,
        &admin,
        "POST",
        "/api/admin/insights/reports",
        json!({"days": 28}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let id = started["report"]["id"].as_str().unwrap().to_owned();
    let mut report = Value::Null;
    for _ in 0..100 {
        let (_, body) = call(
            &app,
            &admin,
            "GET",
            &format!("/api/admin/insights/reports/{id}"),
            Value::Null,
        )
        .await;
        report = body["report"].clone();
        if report["status"] != "building" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(report["status"], "ready", "{report}");
    let streams = report["workstreams"].as_array().unwrap();
    let named = streams
        .iter()
        .find(|w| w["name"] == "Analyzing protein folding experiments")
        .unwrap();
    assert_eq!(named["sessions"], 2);
    assert_eq!(named["rows"][0]["topic"], "Analyzing protein folding data");
    let unlisted = streams.iter().find(|w| w["private"] == true).unwrap();
    assert_eq!(unlisted["sessions"], 1);
    assert_eq!(unlisted["rows"], json!([]));
    assert!(!report.to_string().contains("diary"));
    let (_, again) = call(
        &app,
        &admin,
        "GET",
        "/api/admin/insights?days=28",
        Value::Null,
    )
    .await;
    assert_eq!(again["report"]["id"], json!(id));
}
