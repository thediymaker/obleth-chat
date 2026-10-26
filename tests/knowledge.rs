//! Knowledge end to end: a mock S3 store, a mock gateway with deterministic
//! embeddings and a scripted chat model, the background indexer, search, sharing and
//! cited answers. Requires a disposable DATABASE_URL with pgvector available. PDF cases
//! run when `pdftotext` (or OBLETH_CHAT_PDFTOTEXT) is available.
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{Request, StatusCode},
    response::{sse::Event, IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    gateway::Gateway,
    knowledge::{extract::Tools, Knowledge, ObjectStore, Prompts},
    router, AppState,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:8085";
const PASSWORD: &str = "fixture-only-password-42";

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

/// Bag-of-words vectors: texts sharing words are close. Wider than Chat stores, so the
/// cut to 1024 dimensions is exercised too.
fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; 1536];
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
    {
        let hash = word
            .to_lowercase()
            .bytes()
            .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
        v[(hash % 1024) as usize] += 1.0;
    }
    v
}

async fn embeddings(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(body.clone());
    let data: Vec<Value> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, text)| json!({"index":i,"embedding":embed(text.as_str().unwrap())}))
        .collect();
    Json(json!({"data":data}))
}

/// First round: search the knowledge; then answer citing the first source.
async fn chat(State(seen): State<Seen>, Json(body): Json<Value>) -> axum::response::Response {
    seen.lock().unwrap().push(body.clone());
    // Like the real gateway, only its own model names are accepted; with Auto the
    // upstream reports its own name for the model it served.
    let model = body["model"].as_str().unwrap_or("").to_owned();
    if model != "auto" && model != "fixture-model" {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error":{"message":format!("model '{model}' is not registered")}})),
        )
            .into_response();
    }
    let ev = move |mut v: Value| {
        if model == "auto" {
            v["model"] = json!("fixture-model-fp8");
        }
        Event::default().data(v.to_string())
    };
    let searched = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool");
    let offers = body["tools"].as_array().is_some_and(|t| {
        t.iter()
            .any(|t| t["function"]["name"] == "search_knowledge")
    });
    let last = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .unwrap_or("")
        .to_owned();
    let saves = body["tools"]
        .as_array()
        .is_some_and(|t| t.iter().any(|t| t["function"]["name"] == "save_item"));
    let asks = body["tools"]
        .as_array()
        .is_some_and(|t| t.iter().any(|t| t["function"]["name"] == "ask_user"));
    let stream = async_stream::stream! {
        if asks && !searched && last.contains("Help me pick") {
            yield Ok::<_, Infallible>(ev(json!({"choices":[{"delta":{"content":"Let me ask rather than guess."}}]})));
            let args = json!({"questions":[{"header":"First task","question":"Which sounds most useful?","options":[{"label":"Find a lost thread","recommended":true},{"label":"Critique a document"}]}]});
            let calls = json!([{"index":0,"id":"call_ask","function":{"name":"ask_user","arguments":args.to_string()}}]);
            yield Ok(ev(json!({"choices":[{"delta":{"tool_calls":calls}}]})));
            yield Ok(ev(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})));
        } else if saves && !searched && last.contains("Remember") {
            let args = json!({"title":"tokens_per_second","kind":"code","language":"python","content":"def tokens_per_second(log):\n    return sum(r.out for r in log) / sum(r.decode for r in log)","tags":["#Benchmark","python"]});
            let calls = json!([{"index":0,"id":"call_save","function":{"name":"save_item","arguments":args.to_string()}}]);
            yield Ok::<_, Infallible>(ev(json!({"choices":[{"delta":{"tool_calls":calls}}]})));
            yield Ok(ev(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})));
        } else if offers && !searched && last.contains("Find") {
            let calls = json!([{"index":0,"id":"call_find","function":{"name":"search_knowledge","arguments":json!({"query":"throughput function"}).to_string()}}]);
            yield Ok::<_, Infallible>(ev(json!({"choices":[{"delta":{"tool_calls":calls}}]})));
            yield Ok(ev(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})));
        } else if offers && !searched && !last.contains("Remember") && !last.contains("Find") {
            let calls = json!([{"index":0,"id":"call_search","function":{"name":"search_knowledge","arguments":json!({"query":"Carnot engine efficiency limit"}).to_string()}}]);
            yield Ok::<_, Infallible>(ev(json!({"choices":[{"delta":{"tool_calls":calls}}]})));
            yield Ok(ev(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})));
        } else {
            yield Ok(ev(json!({"choices":[{"delta":{"content":"No engine beats the Carnot limit [1]."}}]})));
            yield Ok(ev(json!({"choices":[{"delta":{},"finish_reason":"stop"}]})));
        }
        yield Ok(Event::default().data("[DONE]"));
    };
    Sse::new(stream).into_response()
}

struct Fixture {
    app: Router,
    objects: Objects,
    embedded: Seen,
    chats: Seen,
    store_url: String,
    _worker: CancellationToken,
}

fn tools() -> Tools {
    Tools::from_env()
}

fn pdf_available() -> bool {
    std::process::Command::new(tools().pdftotext)
        .arg("-v")
        .output()
        .is_ok_and(|o| o.status.success() || !o.stderr.is_empty())
}

async fn fixture(pool: PgPool) -> Fixture {
    let objects: Objects = Arc::default();
    let embedded: Seen = Arc::default();
    let chats: Seen = Arc::default();
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
            get(|| async {
                Json(json!({"data":[
                    {"id":"fixture-model"},
                    {"id":"fixture-embed","mode":"embedding"},
                    {"id":"fixture-embed-2","mode":"embedding"}
                ]}))
            }),
        )
        .route(
            "/v1/embeddings",
            post(embeddings).with_state(embedded.clone()),
        )
        .route("/v1/chat/completions", post(chat).with_state(chats.clone()));
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
    let store_url = format!("http://{address}");
    let store = ObjectStore::new(
        store_url.parse().unwrap(),
        "bucket".into(),
        "auto".into(),
        "fixture-key".into(),
        "fixture-secret".into(),
    );
    let knowledge = Knowledge::new(
        pool.clone(),
        store,
        tools(),
        "fixture-embed".into(),
        4 * 1024 * 1024,
    )
    // Prefixes of short words, which the fixture's embeddings ignore.
    .with_prompts(Prompts {
        query: "q: ".into(),
        document: "d: ".into(),
    });
    let worker = CancellationToken::new();
    tokio::spawn(knowledge.clone().worker(gateway.clone(), worker.clone()));
    let mut state = AppState::new(auth, pool, Some(gateway), 4);
    state.followups_enabled = false;
    state.knowledge = Some(knowledge);
    Fixture {
        app: router(state, "web".into()),
        objects,
        embedded,
        chats,
        store_url,
        _worker: worker,
    }
}

type Session = (String, String);

fn req(
    method: &str,
    path: &str,
    body: Body,
    content_type: &str,
    session: &Session,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("origin", ORIGIN)
        .header("cookie", &session.0)
        .header("x-csrf-token", &session.1)
        .header("content-type", content_type)
        .body(body)
        .unwrap()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    session: &Session,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(req(
            method,
            path,
            Body::from(body.to_string()),
            "application/json",
            session,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn upload(
    app: &Router,
    collection: &str,
    name: &str,
    bytes: Vec<u8>,
    session: &Session,
) -> (StatusCode, Value) {
    let path = format!(
        "/api/knowledge/collections/{collection}/documents?name={}",
        url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>()
    );
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            &path,
            Body::from(bytes),
            "application/octet-stream",
            session,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn login(app: &Router, name: &str) -> Session {
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/auth/login",
            Body::from(json!({"username":name,"password":PASSWORD}).to_string()),
            "application/json",
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

/// Waits until every document in the collection has finished processing.
async fn settled(app: &Router, collection: &str, session: &Session) -> Vec<Value> {
    for _ in 0..150 {
        let (_, detail) = call(
            app,
            "GET",
            &format!("/api/knowledge/collections/{collection}"),
            Value::Null,
            session,
        )
        .await;
        let documents = detail["documents"].as_array().unwrap().clone();
        if documents
            .iter()
            .all(|d| !matches!(d["status"].as_str(), Some("queued" | "processing")))
        {
            return documents;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("documents never finished processing");
}

/// A minimal PDF with one text page per entry; an empty entry draws a line instead.
fn pdf(pages: &[&str]) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|i| format!("{} 0 R", 4 + 2 * i))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for (i, text) in pages.iter().enumerate() {
        let content = if text.is_empty() {
            "0 0 m 300 300 l S".to_string()
        } else {
            let lines: Vec<String> = text
                .split(". ")
                .map(|line| {
                    format!(
                        "({}) Tj T*",
                        line.replace('\\', "\\\\")
                            .replace('(', "\\(")
                            .replace(')', "\\)")
                    )
                })
                .collect();
            format!("BT /F1 10 Tf 14 TL 40 760 Td {} ET", lines.join(" "))
        };
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

const NOTES: &str = "# Heat engines\n\nA Carnot engine is the most efficient engine possible between two reservoirs. Its efficiency limit is one minus the cold temperature over the hot temperature.\n\n# Entropy\n\nIn a free expansion the entropy of the gas rises even though no heat flows, because entropy is a state function.";

#[sqlx::test(migrations = "./migrations")]
async fn documents_are_indexed_searched_and_shared_only_as_allowed(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let (status, created) = call(
        &f.app,
        "POST",
        "/api/knowledge/collections",
        json!({"name":"Thermodynamics"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let collection = created["collection"]["id"].as_str().unwrap().to_owned();

    let (status, _) = upload(
        &f.app,
        &collection,
        "notes.md",
        NOTES.as_bytes().to_vec(),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, refused) = upload(&f.app, &collection, "photo.jpg", vec![1, 2, 3], &admin).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let with_pdf = pdf_available();
    if with_pdf {
        let lecture = pdf(&[
            "Lecture seven covers the second law of thermodynamics. Heat flows from hot to cold on its own. This page introduces reservoirs and cycles in detail",
            "Refrigerators move heat from a cold reservoir to a hot one by doing work. Their coefficient of performance can exceed one. This is the second page of the lecture",
        ]);
        assert_eq!(
            upload(&f.app, &collection, "Lecture 07.pdf", lecture, &admin)
                .await
                .0,
            StatusCode::CREATED
        );
        assert_eq!(
            upload(&f.app, &collection, "scan.pdf", pdf(&["", ""]), &admin)
                .await
                .0,
            StatusCode::CREATED
        );
    }
    let documents = settled(&f.app, &collection, &admin).await;
    let by_name = |name: &str| {
        documents
            .iter()
            .find(|d| d["name"] == name)
            .unwrap()
            .clone()
    };
    let notes = by_name("notes.md");
    assert_eq!(notes["status"], "ready", "{notes}");
    assert!(notes["passages"].as_i64().unwrap() >= 1);
    assert!(f
        .embedded
        .lock()
        .unwrap()
        .iter()
        .all(|b| b["model"] == "fixture-embed"));
    if with_pdf {
        let lecture = by_name("Lecture 07.pdf");
        assert_eq!(lecture["status"], "ready", "{lecture}");
        assert_eq!(lecture["pages"], 2);
        assert_eq!(by_name("scan.pdf")["status"], "needs_ocr");
        let (_, found) = call(&f.app, "POST", "/api/knowledge/search", json!({"query":"refrigerators coefficient of performance","collectionIds":[collection]}), &admin).await;
        let top = &found["results"][0];
        assert_eq!(top["name"], "Lecture 07.pdf", "{found}");
        assert_eq!(top["page"], 2);
    }

    let (_, found) = call(
        &f.app,
        "POST",
        "/api/knowledge/search",
        json!({"query":"Carnot efficiency limit","collectionIds":[collection]}),
        &admin,
    )
    .await;
    // Passages and searches reach the model with their prefixes.
    let inputs: Vec<String> = f
        .embedded
        .lock()
        .unwrap()
        .iter()
        .flat_map(|b| b["input"].as_array().unwrap().clone())
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(inputs.iter().any(|i| i.starts_with("d: ")), "{inputs:?}");
    assert!(inputs.iter().any(|i| i.starts_with("q: ")), "{inputs:?}");
    assert!(inputs
        .iter()
        .all(|i| i.starts_with("d: ") || i.starts_with("q: ")));
    let top = &found["results"][0];
    assert_eq!(top["name"], "notes.md", "{found}");
    assert!(top["excerpt"].as_str().unwrap().contains("Carnot"));
    assert_eq!(top["matched"], "meaning + words");

    // The original opens through a short-lived signed link.
    let response = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!(
                "/api/knowledge/documents/{}/file",
                notes["id"].as_str().unwrap()
            ),
            Body::empty(),
            "application/json",
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().unwrap();
    assert!(
        location.starts_with(&format!("{}/bucket/knowledge/", f.store_url))
            && location.contains("X-Amz-Signature="),
        "{location}"
    );

    // Someone else sees nothing of a personal collection.
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"student","password":PASSWORD,"displayName":"Student","role":"user"}),
        &admin,
    )
    .await;
    assert!(status.is_success());
    let student = login(&f.app, "student").await;
    let (_, listed) = call(
        &f.app,
        "GET",
        "/api/knowledge/collections",
        Value::Null,
        &student,
    )
    .await;
    assert!(listed["collections"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/knowledge/collections/{collection}"),
            Value::Null,
            &student
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, found) = call(
        &f.app,
        "POST",
        "/api/knowledge/search",
        json!({"query":"Carnot","collectionIds":[collection]}),
        &student,
    )
    .await;
    assert!(found["results"].as_array().unwrap().is_empty());
    let response = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!(
                "/api/knowledge/documents/{}/file",
                notes["id"].as_str().unwrap()
            ),
            Body::empty(),
            "application/json",
            &student,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // A student cannot share with everyone; an administrator can.
    let (_, own) = call(
        &f.app,
        "POST",
        "/api/knowledge/collections",
        json!({"name":"Mine"}),
        &student,
    )
    .await;
    let own = own["collection"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &format!("/api/knowledge/collections/{own}"),
            json!({"scope":"everyone"}),
            &student
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &format!("/api/knowledge/collections/{collection}"),
            json!({"scope":"everyone"}),
            &admin
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, listed) = call(
        &f.app,
        "GET",
        "/api/knowledge/collections",
        Value::Null,
        &student,
    )
    .await;
    let shared = listed["collections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == collection.as_str())
        .unwrap()
        .clone();
    assert_eq!(shared["mine"], false);
    let (_, found) = call(
        &f.app,
        "POST",
        "/api/knowledge/search",
        json!({"query":"Carnot","collectionIds":[collection]}),
        &student,
    )
    .await;
    assert!(!found["results"].as_array().unwrap().is_empty());
    // Shared is read only.
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/knowledge/documents/{}", notes["id"].as_str().unwrap()),
            Value::Null,
            &student
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        upload(
            &f.app,
            &collection,
            "more.md",
            b"more text here".to_vec(),
            &student
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    // Deleting a document removes its file from storage.
    let stored = f.objects.lock().unwrap().len();
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/knowledge/documents/{}", notes["id"].as_str().unwrap()),
            Value::Null,
            &admin
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    for _ in 0..50 {
        if f.objects.lock().unwrap().len() < stored {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(f.objects.lock().unwrap().len(), stored - 1);
    let (_, found) = call(
        &f.app,
        "POST",
        "/api/knowledge/search",
        json!({"query":"Carnot efficiency limit","collectionIds":[collection]}),
        &admin,
    )
    .await;
    assert!(found["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["name"] != "notes.md"));
}

#[sqlx::test(migrations = "./migrations")]
async fn uploaded_files_never_open_as_active_content(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, created) = call(
        &f.app,
        "POST",
        "/api/knowledge/collections",
        json!({"name":"Pages"}),
        &admin,
    )
    .await;
    let collection = created["collection"]["id"].as_str().unwrap().to_owned();
    let page = b"<html><body><script>alert(document.domain)</script>Phishing page</body></html>";
    // Whatever type the uploader sends, web pages, text and SVG-typed text are stored
    // and served as plain-text downloads; only PDFs and images open in place.
    for (name, sent) in [
        ("page.html", "text/html"),
        ("notes.txt", "text/html"),
        ("drawing.txt", "image/svg+xml; x=json"),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/api/knowledge/collections/{collection}/documents?name={name}"),
                Body::from(page.to_vec()),
                sent,
                &admin,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED, "{name}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let document: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            document["document"]["mimeType"], "text/plain; charset=utf-8",
            "{name}"
        );
        let response = f
            .app
            .clone()
            .oneshot(req(
                "GET",
                &format!(
                    "/api/knowledge/documents/{}/file",
                    document["document"]["id"].as_str().unwrap()
                ),
                Body::empty(),
                "application/json",
                &admin,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = url::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
        let query: HashMap<String, String> = location.query_pairs().into_owned().collect();
        assert_eq!(
            query["response-content-type"], "text/plain; charset=utf-8",
            "{name}"
        );
        assert!(
            query["response-content-disposition"].starts_with(&format!(
                "attachment; filename=\"{name}\"; filename*=UTF-8''{name}"
            )),
            "{location}"
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_chat_with_a_collection_cites_its_passages(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, created) = call(
        &f.app,
        "POST",
        "/api/knowledge/collections",
        json!({"name":"Thermodynamics"}),
        &admin,
    )
    .await;
    let collection = created["collection"]["id"].as_str().unwrap().to_owned();
    upload(
        &f.app,
        &collection,
        "notes.md",
        NOTES.as_bytes().to_vec(),
        &admin,
    )
    .await;
    settled(&f.app, &collection, &admin).await;

    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Engines","model":"fixture-model"}),
        &admin,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let (status, set) = call(
        &f.app,
        "PUT",
        &format!("/api/conversations/{id}/knowledge"),
        json!({"collectionIds":[collection]}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(set["collectionIds"], json!([collection]));
    let response = f.app.clone().oneshot(req("POST", &format!("/api/conversations/{id}/messages"), Body::from(json!({"content":"What limits the efficiency of a Carnot engine?","model":"fixture-model"}).to_string()), "application/json", &admin)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let streamed = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(streamed.contains("\"kind\":\"sources\""), "{streamed}");

    let first = f.chats.lock().unwrap()[0].clone();
    let system = first["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("# From the person's documents") && system.contains("[1] notes.md"),
        "{system}"
    );
    assert!(first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "search_knowledge"));
    let second = f.chats.lock().unwrap()[1].clone();
    let tool = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    // The search finds the passage already in the prompt, and says so instead of repeating it.
    assert!(
        tool["content"]
            .as_str()
            .unwrap()
            .starts_with("No new passages: this search found only [1]"),
        "{tool}"
    );

    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{id}"),
        Value::Null,
        &admin,
    )
    .await;
    let reply = detail["conversation"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|m| m["role"] == "assistant")
        .unwrap()
        .clone();
    assert_eq!(reply["content"], "No engine beats the Carnot limit [1].");
    let activity = reply["activity"].as_array().unwrap();
    let sources = activity.iter().find(|e| e["kind"] == "sources").unwrap();
    assert_eq!(sources["items"][0]["n"], 1);
    assert_eq!(sources["items"][0]["name"], "notes.md");
    assert_eq!(sources["queries"].as_array().unwrap().len(), 2);
    assert!(activity.iter().any(|e| e["tool"] == "search_knowledge"
        && e["status"] == "completed"
        && e["kind"].is_null()));
    // Removed collections are no longer searched.
    call(
        &f.app,
        "DELETE",
        &format!("/api/knowledge/collections/{collection}"),
        Value::Null,
        &admin,
    )
    .await;
    let (_, set) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{id}/knowledge"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(set["collectionIds"], json!([]));
}

#[sqlx::test(migrations = "./migrations")]
async fn saved_items_are_kept_found_and_private(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    // Saved by hand, with tags cleaned.
    let (status, made) = call(&f.app, "POST", "/api/saved", json!({"kind":"link","title":"Gaudi tuning guide","url":"https://example.com/gaudi","body":"Batch sizes and HBM","tags":["#Research","gaudi"]}), &admin).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["item"]["tags"], json!(["research", "gaudi"]));
    assert_eq!(made["item"]["indexed"], true);
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/saved",
            json!({"kind":"link","title":"x","url":"javascript:alert(1)"}),
            &admin
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // "Remember this" in a chat: the model saves it, linked to the chat.
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Bench","model":"auto"}),
        &admin,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            Body::from(
                json!({"content":"Remember the throughput function","model":"auto"}).to_string(),
            ),
            "application/json",
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap();
    let (_, listed) = call(&f.app, "GET", "/api/saved?kind=code", Value::Null, &admin).await;
    let item = listed["items"][0].clone();
    assert_eq!(item["title"], "tokens_per_second", "{listed}");
    assert_eq!(item["tags"], json!(["benchmark", "python"]));
    assert_eq!(item["source"]["conversationId"], id.as_str());
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{id}"),
        Value::Null,
        &admin,
    )
    .await;
    let reply = detail["conversation"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|m| m["role"] == "assistant")
        .unwrap()
        .clone();
    let card = reply["activity"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "saved")
        .unwrap()
        .clone();
    assert_eq!(card["savedId"], item["id"]);
    assert!(
        !reply["activity"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "sources"),
        "no collections, no Sources card"
    );
    let (_, tagged) = call(
        &f.app,
        "GET",
        "/api/saved?tag=research",
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(tagged["items"].as_array().unwrap().len(), 1);
    assert!(tagged["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["tag"] == "python" && t["count"] == 1));

    // A later chat finds it.
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Later","model":"auto"}),
        &admin,
    )
    .await;
    let later = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{later}/messages"),
            Body::from(
                json!({"content":"Find what I saved about throughput","model":"auto"}).to_string(),
            ),
            "application/json",
            &admin,
        ))
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();
    let last = f.chats.lock().unwrap().last().unwrap().clone();
    // The upstream said fixture-model-fp8; the tool round asks for the gateway's name.
    assert_eq!(last["model"], "fixture-model");
    let tool = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap()
        .clone();
    assert!(
        tool["content"]
            .as_str()
            .unwrap()
            .contains("saved: tokens_per_second"),
        "{tool}"
    );
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{later}"),
        Value::Null,
        &admin,
    )
    .await;
    let reply = detail["conversation"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|m| m["role"] == "assistant")
        .unwrap()
        .clone();
    let sources = reply["activity"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "sources")
        .unwrap()
        .clone();
    assert_eq!(sources["items"][0]["saved"], true);
    assert_eq!(sources["collections"], json!(["your saved items"]));
    assert_eq!(reply["settings"]["model"], "fixture-model", "{reply}");

    // Someone else sees none of it.
    call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"other","password":PASSWORD,"displayName":"Other","role":"user"}),
        &admin,
    )
    .await;
    let other = login(&f.app, "other").await;
    let (_, theirs) = call(&f.app, "GET", "/api/saved", Value::Null, &other).await;
    assert!(theirs["items"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/saved/{}", item["id"].as_str().unwrap()),
            Value::Null,
            &other
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/saved/{}", item["id"].as_str().unwrap()),
            Value::Null,
            &other
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Undo deletes it.
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/saved/{}", item["id"].as_str().unwrap()),
            Value::Null,
            &admin
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn projects_give_their_chats_instructions_collections_and_saved_items(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"jen","password":PASSWORD,"displayName":"Jen","role":"user"}),
        &admin,
    )
    .await;
    call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"outsider","password":PASSWORD,"displayName":"Out","role":"user"}),
        &admin,
    )
    .await;
    let jen = login(&f.app, "jen").await;
    let outsider = login(&f.app, "outsider").await;
    let (_, me) = call(&f.app, "GET", "/api/session", Value::Null, &jen).await;
    let jen_id = me["user"]["id"].as_str().unwrap().to_owned();
    let (_, me) = call(&f.app, "GET", "/api/session", Value::Null, &admin).await;
    let admin_id = me["user"]["id"].as_str().unwrap().to_owned();
    let (_, group) = call(
        &f.app,
        "POST",
        "/api/admin/groups",
        json!({"name":"HPC lab"}),
        &admin,
    )
    .await;
    let group = group["group"]["id"].as_str().unwrap().to_owned();
    for user in [&jen_id, &admin_id] {
        assert!(call(
            &f.app,
            "PUT",
            &format!("/api/admin/groups/{group}/members/{user}"),
            Value::Null,
            &admin
        )
        .await
        .0
        .is_success());
    }

    // A personal collection used by the project, and the project shared with the group.
    let (_, made) = call(
        &f.app,
        "POST",
        "/api/knowledge/collections",
        json!({"name":"Run logs"}),
        &admin,
    )
    .await;
    let collection = made["collection"]["id"].as_str().unwrap().to_owned();
    upload(
        &f.app,
        &collection,
        "notes.md",
        NOTES.as_bytes().to_vec(),
        &admin,
    )
    .await;
    settled(&f.app, &collection, &admin).await;
    let (status, project) = call(&f.app, "POST", "/api/projects", json!({"name":"Gaudi benchmarks","instructions":"Always report tokens per second.","scope":"group","groupId":group}), &admin).await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project = project["project"]["id"].as_str().unwrap().to_owned();
    let (_, set) = call(
        &f.app,
        "PUT",
        &format!("/api/projects/{project}/collections"),
        json!({"collectionIds":[collection]}),
        &admin,
    )
    .await;
    assert_eq!(set["collectionIds"], json!([collection]));
    let (status, _) = call(&f.app, "POST", "/api/saved", json!({"kind":"note","title":"HBM limits batch size","body":"Flattens past 32","projectId":project}), &admin).await;
    assert_eq!(status, StatusCode::CREATED);

    // Jen sees the project and its saved item, but cannot change either.
    let (_, listed) = call(&f.app, "GET", "/api/projects", Value::Null, &jen).await;
    assert_eq!(listed["projects"][0]["mine"], false);
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/projects/{project}"),
        Value::Null,
        &jen,
    )
    .await;
    assert_eq!(detail["saved"][0]["title"], "HBM limits batch size");
    assert_eq!(detail["collections"][0]["name"], "Run logs");
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &format!("/api/projects/{project}"),
            json!({"name":"Mine now"}),
            &jen
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let item = detail["saved"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &format!("/api/saved/{item}"),
            json!({"title":"x"}),
            &jen
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // Someone outside the group sees none of it.
    let (_, theirs) = call(&f.app, "GET", "/api/projects", Value::Null, &outsider).await;
    assert!(theirs["projects"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/saved/{item}"),
            Value::Null,
            &outsider
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Jen's chat in the project gets its instructions and searches its collection.
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Engines","model":"fixture-model"}),
        &jen,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let (status, placed) = call(
        &f.app,
        "PUT",
        &format!("/api/conversations/{id}/project"),
        json!({"projectId":project}),
        &jen,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{placed}");
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            Body::from(
                json!({"content":"What limits a Carnot engine?","model":"fixture-model"})
                    .to_string(),
            ),
            "application/json",
            &jen,
        ))
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();
    let first = f
        .chats
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|c| c["messages"][1]["content"] == "What limits a Carnot engine?")
        .unwrap()
        .clone();
    let system = first["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("# Project: Gaudi benchmarks")
            && system.contains("Always report tokens per second."),
        "{system}"
    );
    assert!(system.contains("[1] notes.md"), "{system}");
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/projects/{project}"),
        Value::Null,
        &jen,
    )
    .await;
    assert_eq!(detail["chats"][0]["id"], id.as_str());
    let (_, admins) = call(
        &f.app,
        "GET",
        &format!("/api/projects/{project}"),
        Value::Null,
        &admin,
    )
    .await;
    assert!(
        admins["chats"].as_array().unwrap().is_empty(),
        "each person's chats stay theirs"
    );

    // The Groups page shows Jen the group, its people and what it shares; not outsiders.
    let (_, groups) = call(&f.app, "GET", "/api/groups", Value::Null, &jen).await;
    assert_eq!(groups["groups"][0]["name"], "HPC lab");
    assert_eq!(groups["groups"][0]["members"], 2);
    assert_eq!(groups["groups"][0]["projects"], 1);
    assert_eq!(groups["canManage"], false);
    let (_, shown) = call(
        &f.app,
        "GET",
        &format!("/api/groups/{group}"),
        Value::Null,
        &jen,
    )
    .await;
    assert_eq!(shown["projects"][0]["name"], "Gaudi benchmarks");
    assert_eq!(shown["projects"][0]["saved"], 1);
    assert_eq!(shown["members"].as_array().unwrap().len(), 2);
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/groups/{group}"),
            Value::Null,
            &outsider
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Leaving the group ends access; deleting the project keeps the chat.
    call(
        &f.app,
        "DELETE",
        &format!("/api/admin/groups/{group}/members/{jen_id}"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/projects/{project}"),
            Value::Null,
            &jen
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/projects/{project}"),
            Value::Null,
            &admin
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/conversations/{id}"),
            Value::Null,
            &jen
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_shared_project_shares_only_what_its_owner_may_share(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let mut ids = Vec::new();
    for name in ["jen", "kim"] {
        call(
            &f.app,
            "POST",
            "/api/admin/users",
            json!({"username":name,"password":PASSWORD,"displayName":name,"role":"user"}),
            &admin,
        )
        .await;
        let session = login(&f.app, name).await;
        let (_, me) = call(&f.app, "GET", "/api/session", Value::Null, &session).await;
        ids.push((session, me["user"]["id"].as_str().unwrap().to_owned()));
    }
    let (_, me) = call(&f.app, "GET", "/api/session", Value::Null, &admin).await;
    let admin_id = me["user"]["id"].as_str().unwrap().to_owned();
    let [(jen, jen_id), (kim, kim_id)] = <[_; 2]>::try_from(ids).ok().unwrap();
    let group = |name: &'static str, members: Vec<String>| {
        let app = f.app.clone();
        let admin = admin.clone();
        async move {
            let (_, made) = call(
                &app,
                "POST",
                "/api/admin/groups",
                json!({"name":name}),
                &admin,
            )
            .await;
            let id = made["group"]["id"].as_str().unwrap().to_owned();
            for user in members {
                call(
                    &app,
                    "PUT",
                    &format!("/api/admin/groups/{id}/members/{user}"),
                    Value::Null,
                    &admin,
                )
                .await;
            }
            id
        }
    };
    // Lab: Jen and Kim. Staff: Jen and the administrator.
    let lab = group("Lab", vec![jen_id.clone(), kim_id.clone()]).await;
    let staff = group("Staff", vec![jen_id.clone(), admin_id.clone()]).await;
    let collection = |body: Value, who: Session| {
        let app = f.app.clone();
        async move {
            let (_, made) = call(&app, "POST", "/api/knowledge/collections", body, &who).await;
            made["collection"]["id"].as_str().unwrap().to_owned()
        }
    };
    let answers = collection(
        json!({"name":"Exam answers","scope":"group","groupId":staff}),
        admin.clone(),
    )
    .await;
    let notes = collection(json!({"name":"Jen's notes"}), jen.clone()).await;

    // Jen shares a project with the lab and attaches both; she can open both.
    let (_, project) = call(
        &f.app,
        "POST",
        "/api/projects",
        json!({"name":"Lab work","scope":"group","groupId":lab}),
        &jen,
    )
    .await;
    let project = project["project"]["id"].as_str().unwrap().to_owned();
    let (_, set) = call(
        &f.app,
        "PUT",
        &format!("/api/projects/{project}/collections"),
        json!({"collectionIds":[answers, notes]}),
        &jen,
    )
    .await;
    assert_eq!(set["collectionIds"].as_array().unwrap().len(), 2);

    // Kim gets Jen's own collection through the project, never the staff one.
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/projects/{project}"),
        Value::Null,
        &kim,
    )
    .await;
    let names: Vec<&str> = detail["collections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Jen's notes"]);
    for (id, want) in [(&notes, StatusCode::OK), (&answers, StatusCode::NOT_FOUND)] {
        let (status, _) = call(
            &f.app,
            "GET",
            &format!("/api/knowledge/collections/{id}"),
            Value::Null,
            &kim,
        )
        .await;
        assert_eq!(status, want);
    }

    // Kim saves into the project; Jen, the owner, can take it out of the project.
    let (_, saved) = call(
        &f.app,
        "POST",
        "/api/saved",
        json!({"kind":"note","title":"Kim's idea","body":"Try it","projectId":project}),
        &kim,
    )
    .await;
    let item = saved["item"]["id"].as_str().unwrap().to_owned();
    let (_, seen) = call(
        &f.app,
        "GET",
        &format!("/api/saved/{item}"),
        Value::Null,
        &jen,
    )
    .await;
    assert_eq!(seen["item"]["ownerName"], "kim");
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/projects/{project}/saved/{item}"),
            Value::Null,
            &kim
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/projects/{project}/saved/{item}"),
            Value::Null,
            &jen
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, kept) = call(
        &f.app,
        "GET",
        &format!("/api/saved/{item}"),
        Value::Null,
        &kim,
    )
    .await;
    assert_eq!(
        (status, kept["item"]["projectId"].clone()),
        (StatusCode::OK, Value::Null)
    );

    // After Jen leaves the lab, saving her project keeps it shared there, but she
    // cannot share something new with it.
    call(
        &f.app,
        "DELETE",
        &format!("/api/admin/groups/{lab}/members/{jen_id}"),
        Value::Null,
        &admin,
    )
    .await;
    let (status, renamed) = call(
        &f.app,
        "PATCH",
        &format!("/api/projects/{project}"),
        json!({"name":"Lab work 2","scope":"group","groupId":lab}),
        &jen,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["project"]["groupId"], lab.as_str());
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/projects",
        json!({"name":"Another","scope":"group","groupId":lab}),
        &jen,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Administrators see every group, and may share with one they are not in.
    let (_, all) = call(&f.app, "GET", "/api/groups", Value::Null, &admin).await;
    let rows = all["groups"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|g| g["id"] == lab.as_str() && g["member"] == false));
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/groups/{lab}"),
            Value::Null,
            &admin
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/projects",
        json!({"name":"Admin's","scope":"group","groupId":lab}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Deleting the lab un-shares what it held.
    assert_eq!(
        call(
            &f.app,
            "DELETE",
            &format!("/api/admin/groups/{lab}"),
            Value::Null,
            &admin
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, mine) = call(
        &f.app,
        "GET",
        &format!("/api/projects/{project}"),
        Value::Null,
        &jen,
    )
    .await;
    assert_eq!(
        (
            mine["project"]["scope"].clone(),
            mine["project"]["groupId"].clone()
        ),
        (json!("personal"), Value::Null)
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/projects/{project}"),
            Value::Null,
            &kim
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_shared_chat_is_read_only_live_and_forkable(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    let mut people = Vec::new();
    for name in ["ana", "ben", "cal"] {
        call(
            &f.app,
            "POST",
            "/api/admin/users",
            json!({"username":name,"password":PASSWORD,"displayName":name,"role":"user"}),
            &admin,
        )
        .await;
        let session = login(&f.app, name).await;
        let (_, me) = call(&f.app, "GET", "/api/session", Value::Null, &session).await;
        people.push((session, me["user"]["id"].as_str().unwrap().to_owned()));
    }
    let [(ana, ana_id), (ben, ben_id), (cy, _)] = <[_; 3]>::try_from(people).ok().unwrap();
    let (_, made) = call(
        &f.app,
        "POST",
        "/api/admin/groups",
        json!({"name":"Pairs"}),
        &admin,
    )
    .await;
    let group = made["group"]["id"].as_str().unwrap().to_owned();
    for user in [&ana_id, &ben_id] {
        call(
            &f.app,
            "PUT",
            &format!("/api/admin/groups/{group}/members/{user}"),
            Value::Null,
            &admin,
        )
        .await;
    }

    // Ana's chat: one turn, with a file attached and an image in the reply.
    let (_, file) = call(
        &f.app,
        "POST",
        "/api/attachments",
        json!({"name":"plan.txt","mimeType":"text/plain","dataBase64":"cGxhbg=="}),
        &ana,
    )
    .await;
    let file = file["id"].as_str().unwrap().to_owned();
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Pairing","model":"fixture-model"}),
        &ana,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            Body::from(json!({"content":"Plan the refactor","model":"fixture-model","attachmentIds":[file]}).to_string()),
            "application/json",
            &ana,
        ))
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();
    let conversation: uuid::Uuid = id.parse().unwrap();
    let ana_uuid: uuid::Uuid = ana_id.parse().unwrap();
    let image = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO generated_images (id, owner_id, conversation_id, mime_type, size, data) VALUES ($1,$2,$3,'image/png',4,'\\x89504e47')")
        .bind(image).bind(ana_uuid).bind(conversation).execute(&pool).await.unwrap();
    sqlx::query("UPDATE messages SET content = content || $2 WHERE conversation_id = $1 AND role = 'assistant'")
        .bind(conversation).bind(format!("\n\n![chart](/api/images/{image})")).execute(&pool).await.unwrap();

    // Not shared: nobody else can open it.
    let shared = format!("/api/shared/{id}");
    assert_eq!(
        call(&f.app, "GET", &shared, Value::Null, &ben).await.0,
        StatusCode::NOT_FOUND
    );
    // Only the owner changes who sees it.
    let share = format!("/api/conversations/{id}/share");
    assert_eq!(
        call(&f.app, "PUT", &share, json!({"scope":"signed_in"}), &ben)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // Shared with the group: Ben reads it, with working links; Cy does not.
    let (status, set) = call(
        &f.app,
        "PUT",
        &share,
        json!({"scope":"group","groupId":group}),
        &ana,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set["share"]["groupName"], "Pairs");
    let (status, seen) = call(&f.app, "GET", &shared, Value::Null, &ben).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(
        (seen["mine"].clone(), seen["share"]["ownerName"].clone()),
        (json!(false), json!("ana"))
    );
    assert!(seen["conversation"].get("settings").is_none());
    let messages = seen["conversation"]["messages"].as_array().unwrap();
    assert_eq!(messages[0]["content"], "Plan the refactor");
    let link = messages[0]["attachments"][0]["url"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(link, format!("/api/shared/{id}/attachments/{file}"));
    assert!(messages[1]["content"]
        .as_str()
        .unwrap()
        .contains(&format!("/api/shared/{id}/images/{image}")));
    let get = |path: String, who: Session| {
        let app = f.app.clone();
        async move {
            app.oneshot(req("GET", &path, Body::empty(), "application/json", &who))
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(get(link.clone(), ben.clone()).await, StatusCode::OK);
    assert_eq!(
        get(format!("/api/shared/{id}/images/{image}"), ben.clone()).await,
        StatusCode::OK
    );
    assert_eq!(
        get(format!("/api/attachments/{file}"), ben.clone()).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&f.app, "GET", &shared, Value::Null, &cy).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(link.clone(), cy.clone()).await, StatusCode::NOT_FOUND);
    let (_, page) = call(
        &f.app,
        "GET",
        &format!("/api/groups/{group}"),
        Value::Null,
        &ben,
    )
    .await;
    assert_eq!(page["chats"][0]["title"], "Pairing");
    // Ben still cannot write to it.
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &format!("/api/conversations/{id}"),
            json!({"title":"Mine"}),
            &ben
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Anyone signed in with the link: Cy too. Ben forks it into his own chat.
    call(&f.app, "PUT", &share, json!({"scope":"signed_in"}), &ana).await;
    assert_eq!(
        call(&f.app, "GET", &shared, Value::Null, &cy).await.0,
        StatusCode::OK
    );
    let (status, forked) = call(&f.app, "POST", &format!("{shared}/fork"), Value::Null, &ben).await;
    assert_eq!(status, StatusCode::CREATED, "{forked}");
    let copy = forked["conversation"]["id"].as_str().unwrap().to_owned();
    let (status, mine) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{copy}"),
        Value::Null,
        &ben,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mine["conversation"]["title"], "Pairing · fork");
    let copied = mine["conversation"]["messages"].as_array().unwrap();
    assert_eq!(copied.len(), 2);
    let own_file = copied[0]["attachments"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(own_file, file);
    assert_eq!(
        get(format!("/api/attachments/{own_file}"), ben.clone()).await,
        StatusCode::OK
    );
    let text = copied[1]["content"].as_str().unwrap();
    assert!(!text.contains(&image.to_string()), "{text}");
    let own_image = text
        .split("/api/images/")
        .nth(1)
        .unwrap()
        .trim_end_matches(')')
        .to_owned();
    assert_eq!(
        get(format!("/api/images/{own_image}"), ben.clone()).await,
        StatusCode::OK
    );

    // Unsharing ends it for everyone else; the fork stays Ben's.
    call(&f.app, "PUT", &share, json!({"scope":"none"}), &ana).await;
    assert_eq!(
        call(&f.app, "GET", &shared, Value::Null, &cy).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/conversations/{copy}"),
            Value::Null,
            &ben
        )
        .await
        .0,
        StatusCode::OK
    );

    // Deleting the group unshares chats shared with it.
    call(
        &f.app,
        "PUT",
        &share,
        json!({"scope":"group","groupId":group}),
        &ana,
    )
    .await;
    call(
        &f.app,
        "DELETE",
        &format!("/api/admin/groups/{group}"),
        Value::Null,
        &admin,
    )
    .await;
    let (_, after) = call(&f.app, "GET", &share, Value::Null, &ana).await;
    assert_eq!(after["share"]["scope"], "none");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_reply_that_asks_questions_ends_at_the_card(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = login(&f.app, "test-admin").await;
    let (_, chat) = call(
        &f.app,
        "POST",
        "/api/conversations",
        json!({"title":"Pick","model":"auto"}),
        &admin,
    )
    .await;
    let id = chat["conversation"]["id"].as_str().unwrap().to_owned();
    let before = f.chats.lock().unwrap().len();
    let response = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/conversations/{id}/messages"),
            Body::from(json!({"content":"Help me pick a first task","model":"auto"}).to_string()),
            "application/json",
            &admin,
        ))
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();
    let calls = f.chats.lock().unwrap()[before..].to_vec();
    assert_eq!(calls.len(), 1, "the reply ends at the questions");
    assert!(calls[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "ask_user"));
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/conversations/{id}"),
        Value::Null,
        &admin,
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
    assert_eq!(reply["content"], "Let me ask rather than guess.");
    let asked = reply["activity"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "question")
        .unwrap()
        .clone();
    assert_eq!(asked["status"], "asked");
    assert_eq!(asked["questions"][0]["header"], "First task");
    assert_eq!(asked["questions"][0]["options"][0]["recommended"], true);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_project_shows_its_research_to_the_people_who_can_see_it(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"outsider","password":PASSWORD,"displayName":"Out","role":"user"}),
        &admin,
    )
    .await;
    let outsider = login(&f.app, "outsider").await;
    let (_, project) = call(
        &f.app,
        "POST",
        "/api/projects",
        json!({"name":"Gaudi benchmarks"}),
        &admin,
    )
    .await;
    let project = project["project"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/projects/{project}/research");
    let (status, empty) = call(&f.app, "GET", &path, Value::Null, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(empty["research"].is_null());

    // A tree with a baseline, a child and two runs, as the tools would leave it.
    let owner: uuid::Uuid = sqlx::query_scalar("SELECT owner_id FROM projects WHERE id=$1::uuid")
        .bind(&project)
        .fetch_one(&pool)
        .await
        .unwrap();
    let (research, base, child) = (
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
    );
    sqlx::query("INSERT INTO research(id,project_id,owner_id,goal,metric,direction,run_command) VALUES($1,$2::uuid,$3,'Faster decode','decode_tok_s','max','python bench.py')")
        .bind(research).bind(&project).bind(owner).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,title,branch,frozen) VALUES($1,$2,'Baseline','research/baseline',TRUE)")
        .bind(base).bind(research).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO experiments(id,research_id,parent_id,title,branch) VALUES($1,$2,$3,'Batch 48','research/batch-48-ab12')")
        .bind(child).bind(research).bind(base).execute(&pool).await.unwrap();
    let run = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status,metric,metrics,log_tail,finished_at) VALUES($1,$2,$3,1,'3f2a9c1d0e4b','completed',1512,'{\"decode_tok_s\":1512}','decode_tok_s=1512',NOW())")
        .bind(run).bind(research).bind(base).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status,diff,diff_stat) VALUES($1,$2,$3,2,'b7c3a20f1e2d','running','-batch: 16\n+batch: 48','1 file changed')")
        .bind(uuid::Uuid::new_v4()).bind(research).bind(child).execute(&pool).await.unwrap();

    let (_, shown) = call(&f.app, "GET", &path, Value::Null, &admin).await;
    assert_eq!(shown["research"]["metric"], "decode_tok_s");
    assert_eq!(shown["experiments"].as_array().unwrap().len(), 2);
    assert_eq!(shown["experiments"][1]["parentId"], base.to_string());
    assert_eq!(shown["runs"][0]["metric"], 1512.0);
    assert_eq!(shown["runs"][1]["status"], "running");
    let (_, detail) = call(
        &f.app,
        "GET",
        &format!("/api/research/runs/{run}"),
        Value::Null,
        &admin,
    )
    .await;
    assert_eq!(detail["run"]["log"], "decode_tok_s=1512");
    assert_eq!(detail["run"]["canCancel"], false, "it finished");

    // Someone outside the project sees none of it.
    assert_eq!(
        call(&f.app, "GET", &path, Value::Null, &outsider).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/research/runs/{run}"),
            Value::Null,
            &outsider
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Deleting the project deletes its research.
    call(
        &f.app,
        "DELETE",
        &format!("/api/projects/{project}"),
        Value::Null,
        &admin,
    )
    .await;
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM research_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn changing_the_embedding_model_reindexes_everything(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin = login(&f.app, "test-admin").await;
    call(
        &f.app,
        "POST",
        "/api/admin/users",
        json!({"username":"student","password":PASSWORD,"displayName":"Student","role":"user"}),
        &admin,
    )
    .await;
    let student = login(&f.app, "student").await;
    let (_, created) = call(
        &f.app,
        "POST",
        "/api/knowledge/collections",
        json!({"name":"Thermodynamics"}),
        &admin,
    )
    .await;
    let collection = created["collection"]["id"].as_str().unwrap().to_owned();
    upload(
        &f.app,
        &collection,
        "notes.md",
        NOTES.as_bytes().to_vec(),
        &admin,
    )
    .await;
    let (status, _) = call(
        &f.app,
        "POST",
        "/api/saved",
        json!({"kind":"note","title":"Carnot","body":"Efficiency limit of heat engines"}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        settled(&f.app, &collection, &admin).await[0]["status"],
        "ready"
    );

    let path = "/api/admin/knowledge/embedding";
    assert_eq!(
        call(&f.app, "GET", path, Value::Null, &student).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            path,
            json!({"model":"fixture-embed-2"}),
            &student
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, settings) = call(&f.app, "GET", path, Value::Null, &admin).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["effective"]["model"], "fixture-embed");
    assert_eq!(settings["chosen"]["model"], Value::Null);
    assert_eq!(
        settings["models"],
        json!(["fixture-embed", "fixture-embed-2"])
    );
    assert_eq!(settings["index"]["documents"], 1);
    for bad in [
        json!({"model":"fixture-model"}),
        json!({"model":""}),
        json!({"color":"blue"}),
        json!({"queryPrefix":"bell\u{7}"}),
        json!({"documentPrefix":"x".repeat(501)}),
    ] {
        assert_eq!(
            call(&f.app, "PATCH", path, bad.clone(), &admin).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }

    // A new model and passage prefix re-index every document and saved item with them.
    let before = f.embedded.lock().unwrap().len();
    let (status, changed) = call(
        &f.app,
        "PATCH",
        path,
        json!({"model":"fixture-embed-2","documentPrefix":"p: "}),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["reindexed"], true);
    assert_eq!(changed["effective"]["model"], "fixture-embed-2");
    assert_eq!(
        changed["effective"]["queryPrefix"], "",
        "the default model's prefix doesn't carry over to a model Chat doesn't know"
    );
    assert_eq!(changed["prefixes"]["fixture-embed"]["queryPrefix"], "q: ");
    assert_eq!(changed["prefixes"]["fixture-embed-2"]["documentPrefix"], "");
    assert_eq!(
        settled(&f.app, &collection, &admin).await[0]["status"],
        "ready"
    );
    let mut saved_done = false;
    for _ in 0..100 {
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM saved_items WHERE embedding IS NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        if pending == 0 {
            saved_done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(saved_done, "saved items are embedded again");
    let after: Vec<Value> = f.embedded.lock().unwrap()[before..].to_vec();
    assert!(after.iter().all(|b| b["model"] == "fixture-embed-2"));
    let inputs: Vec<String> = after
        .iter()
        .flat_map(|b| b["input"].as_array().unwrap().clone())
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        inputs
            .iter()
            .any(|i| i.starts_with("p: ") && i.contains("Carnot engine")),
        "{inputs:?}"
    );
    assert!(
        inputs.iter().any(|i| i.starts_with("p: Carnot")),
        "{inputs:?}"
    );
    let (_, found) = call(
        &f.app,
        "POST",
        "/api/knowledge/search",
        json!({"query":"Carnot efficiency limit","collectionIds":[collection]}),
        &admin,
    )
    .await;
    assert_eq!(found["results"][0]["name"], "notes.md", "{found}");

    // Saving the same settings again changes nothing; it is all audited.
    let (_, same) = call(
        &f.app,
        "PATCH",
        path,
        json!({"model":"fixture-embed-2"}),
        &admin,
    )
    .await;
    assert_eq!(same["reindexed"], false);
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action='knowledge.embedding.update'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 2);

    // Back to the defaults re-indexes again.
    let (_, reset) = call(
        &f.app,
        "PATCH",
        path,
        json!({"model":null,"documentPrefix":null}),
        &admin,
    )
    .await;
    assert_eq!(reset["reindexed"], true);
    assert_eq!(reset["effective"]["model"], "fixture-embed");
    assert_eq!(reset["effective"]["documentPrefix"], "d: ");
    assert_eq!(
        settled(&f.app, &collection, &admin).await[0]["status"],
        "ready"
    );
}
