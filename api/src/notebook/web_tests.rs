use super::*;
use crate::notebook::tests::{collection, document as ready_document, group, person, project};
use crate::test_support::Database;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::{any, get},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---- Addresses ----

#[test]
fn only_public_http_addresses_are_read() {
    assert_eq!(
        address(" example.org/post ").unwrap(),
        "https://example.org/post"
    );
    assert_eq!(
        address("http://example.org/a?b=1#c").unwrap(),
        "http://example.org/a?b=1#c"
    );
    assert_eq!(
        address("https://93.184.216.34/x").unwrap(),
        "https://93.184.216.34/x"
    );
    for (wrong, said) in [
        ("", "Give a web address"),
        ("file:///etc/passwd", "Only http and https"),
        ("javascript:alert(1)", "not a web address"),
        ("javascript://alert(1)", "Only http and https"),
        ("ftp://example.org/a", "Only http and https"),
        ("https://user:secret@example.org/", "user name and password"),
        ("http://localhost:9223/json", "public web sites"),
        ("http://127.0.0.1:9223/json/version", "public web sites"),
        ("http://10.1.2.3/", "public web sites"),
        ("http://192.168.0.1/", "public web sites"),
        (
            "http://169.254.169.254/latest/meta-data",
            "public web sites",
        ),
        ("http://100.64.0.1/", "public web sites"),
        ("http://[::1]/", "public web sites"),
        ("http://[fd00::1]/", "public web sites"),
        ("http://[::ffff:10.0.0.1]/", "public web sites"),
        ("http://intranet/", "public web sites"),
        ("http://printer.local/", "public web sites"),
        ("http://svc.cluster.internal/", "public web sites"),
    ] {
        let err = address(wrong).unwrap_err();
        assert!(err.contains(said), "{wrong}: {err}");
    }
    let long = format!("https://example.org/{}", "a".repeat(2000));
    assert!(address(&long).unwrap_err().contains("2,000"));
}

#[test]
fn a_page_is_kept_as_markdown_and_a_pdf_as_itself() {
    let (name, kind, bytes) = document(Read::Page {
        url: "https://example.org/a".into(),
        title: "Warmup / schedules: a note".into(),
        text: "# Warmup\n\nLinear.".into(),
    });
    assert_eq!(
        (name.as_str(), kind, bytes.as_slice()),
        (
            "Warmup _ schedules: a note.md",
            Kind::Markdown,
            &b"# Warmup\n\nLinear."[..]
        )
    );
    // The name is what extract reads the kind from, so its sections are found.
    assert_eq!(
        crate::knowledge::extract::kind(&name, &crate::knowledge::extract::mime_for(kind)),
        Some(Kind::Markdown)
    );
    let (name, kind, _) = document(Read::Pdf {
        url: "https://example.org/p.pdf".into(),
        name: "paper.PDF".into(),
        bytes: b"%PDF-1.7".to_vec(),
    });
    assert_eq!((name.as_str(), kind), ("paper.pdf", Kind::Pdf));
    let (name, _, _) = document(Read::Page {
        url: "https://example.org/".into(),
        title: "...".into(),
        text: "x".into(),
    });
    assert_eq!(name, "Web page.md");
}

// ---- A browser stand-in ----

/// What the stand-in browser shows for an address.
#[derive(Clone)]
enum Site {
    /// A page: its status and what the page reads as.
    Page(u64, Value),
    Pdf(Vec<u8>),
    /// The address does not load at all.
    Unreachable,
}

#[derive(Clone, Default)]
struct Browser {
    sites: Arc<Mutex<HashMap<String, Site>>>,
    /// What the chat's own tab shows, and the address of a PDF it shows in the viewer.
    tab: Arc<Mutex<Option<Value>>>,
    tab_pdf: Arc<Mutex<Option<String>>>,
    /// Every DevTools method called, with the address for Page.navigate.
    calls: Arc<Mutex<Vec<String>>>,
}

impl Browser {
    fn site(&self, url: &str, site: Site) {
        self.sites.lock().unwrap().insert(url.into(), site);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn page(url: &str, title: &str, text: &str) -> Value {
    json!({"url":url,"title":title,"heading":"","text":text,"password":false,"pdf":false,"over":false})
}

fn long_text(topic: &str) -> String {
    format!(
        "# {topic}\n\n{}\n\n## Results\n\n{}",
        "The method warms up the learning rate linearly. ".repeat(10),
        "Loss reached 2.31 after warmup. ".repeat(10)
    )
}

/// A DevTools endpoint that answers like Chromium for the calls reading makes: each
/// navigation is held at its response for the reader, PDFs are streamed, and the
/// page reads as the site says. It fetches nothing.
async fn stand_in_browser() -> (String, Browser) {
    let browser = Browser::default();
    let app = Router::new()
        .route(
            "/json/version",
            get(|| async {
                Json(json!({"webSocketDebuggerUrl":"ws://ignored/devtools/browser/stand-in"}))
            }),
        )
        .route(
            "/devtools/{*path}",
            any(
                |State(b): State<Browser>, ws: WebSocketUpgrade| async move {
                    ws.on_upgrade(move |socket| devtools(socket, b))
                        .into_response()
                },
            ),
        )
        .with_state(browser.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, browser)
}

async fn devtools(mut socket: WebSocket, b: Browser) {
    // The navigation waiting for its held response, and the address loaded.
    let mut waiting: Option<u64> = None;
    let mut at = String::new();
    while let Some(Ok(Message::Text(text))) = socket.recv().await {
        let call: Value = serde_json::from_str(text.as_str()).unwrap();
        let (id, method) = (
            call["id"].as_u64().unwrap(),
            call["method"].as_str().unwrap().to_owned(),
        );
        let session = call["sessionId"].clone();
        let params = &call["params"];
        let mut events = vec![];
        let result = match method.as_str() {
            "Target.getTargets" => json!({"targetInfos":[{"targetId":"TAB","type":"page"}]}),
            "Target.createTarget" => json!({"targetId":"SCRATCH"}),
            "Target.attachToTarget" => {
                json!({"sessionId":format!("S-{}", params["targetId"].as_str().unwrap())})
            }
            "Page.navigate" => {
                at = params["url"].as_str().unwrap().to_owned();
                b.calls.lock().unwrap().push(format!("Page.navigate {at}"));
                let site = b.sites.lock().unwrap().get(&at).cloned();
                let held = |status: u64, kind: &str| {
                    json!({"method":"Fetch.requestPaused","sessionId":session,"params":{"requestId":"R1","frameId":"SCRATCH",
                        "resourceType":"Document","request":{"url":at},"responseStatusCode":status,
                        "responseHeaders":[{"name":"Content-Type","value":kind}]}})
                };
                match site {
                    Some(Site::Page(status, _)) => {
                        events.push(held(status, "text/html; charset=utf-8"))
                    }
                    Some(Site::Pdf(_)) => events.push(held(200, "application/pdf")),
                    Some(Site::Unreachable) | None => {}
                }
                if events.is_empty() {
                    json!({"frameId":"SCRATCH","errorText":"net::ERR_NAME_NOT_RESOLVED"})
                } else {
                    // Answered once the reader lets the response through or stops it.
                    waiting = Some(id);
                    for event in events.drain(..) {
                        socket
                            .send(Message::Text(event.to_string().into()))
                            .await
                            .unwrap();
                    }
                    continue;
                }
            }
            "Fetch.continueResponse" | "Fetch.failRequest" => {
                b.calls.lock().unwrap().push(method.clone());
                if let Some(navigation) = waiting.take() {
                    let answer = if method == "Fetch.failRequest" {
                        json!({"frameId":"SCRATCH","errorText":"net::ERR_ABORTED"})
                    } else {
                        json!({"frameId":"SCRATCH","loaderId":"L1"})
                    };
                    events.push(json!({"id":navigation,"result":answer}));
                }
                json!({})
            }
            "Page.getResourceTree" => match b.tab_pdf.lock().unwrap().clone() {
                Some(url) if session == "S-TAB" => {
                    json!({"frameTree":{"frame":{"id":"TAB","url":url,"mimeType":"application/pdf"}}})
                }
                _ => json!({"frameTree":{"frame":{"id":"TAB","mimeType":"text/html"}}}),
            },
            "Fetch.takeResponseBodyAsStream" => json!({"stream":"H1"}),
            "IO.read" => match b.sites.lock().unwrap().get(&at) {
                Some(Site::Pdf(bytes)) => {
                    json!({"base64Encoded":true,"data":STANDARD.encode(bytes),"eof":true})
                }
                _ => json!({"data":"","eof":true}),
            },
            "Runtime.evaluate" => {
                let expression = params["expression"].as_str().unwrap();
                let value = if expression.contains("document.readyState") {
                    json!("complete")
                } else if session == "S-TAB" {
                    b.tab.lock().unwrap().clone().unwrap_or(Value::Null)
                } else {
                    match b.sites.lock().unwrap().get(&at) {
                        Some(Site::Page(_, value)) => value.clone(),
                        _ => Value::Null,
                    }
                };
                json!({"result":{"type":"object","value":value}})
            }
            _ => json!({}),
        };
        if !method.starts_with("Fetch.continue") && method != "Fetch.failRequest" {
            b.calls.lock().unwrap().push(method.clone());
        }
        let answer = json!({"id":id,"result":result});
        if socket
            .send(Message::Text(answer.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        for event in events {
            let _ = socket.send(Message::Text(event.to_string().into())).await;
        }
    }
}

/// A web site that counts every request it gets: the pages the tests add are on it,
/// so a request here would be Chat fetching an address itself.
async fn counting_site() -> (String, Arc<Mutex<usize>>) {
    let hits: Arc<Mutex<usize>> = Arc::default();
    let counted = hits.clone();
    let app = Router::new().fallback(move || {
        let counted = counted.clone();
        async move {
            *counted.lock().unwrap() += 1;
            "<h1>Fetched</h1>"
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, hits)
}

struct Fixture {
    db: Database,
    s: AppState,
    browser: Browser,
    bucket: crate::knowledge::storage::stub::Seen,
}

async fn fixture() -> Option<Fixture> {
    let db = Database::new().await?;
    let pool = db.pool.clone();
    let (address, browser) = stand_in_browser().await;
    let (store, bucket) = crate::knowledge::storage::stub::bucket().await;
    let auth = crate::auth::Auth::new(
        pool.clone(),
        crate::auth::AuthConfig {
            origin: "https://chat.example.org".into(),
            ..crate::auth::AuthConfig::default()
        },
    )
    .await
    .unwrap();
    let mut s = AppState::new(auth, pool.clone(), None, 4);
    s.store = crate::store::Store::new(pool.clone())
        .with_files(crate::files::Files::new(pool.clone(), Some(store.clone())));
    s.knowledge = Some(crate::knowledge::Knowledge::new(
        pool.clone(),
        store,
        crate::knowledge::extract::Tools::from_env(),
        "fixture-embed".into(),
        1 << 30,
    ));
    s.browsers = Some(crate::browser::Browsers::fixed(pool, address));
    Some(Fixture {
        db,
        s,
        browser,
        bucket,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct Kept {
    id: Uuid,
    name: String,
    mime_type: String,
    status: String,
    source_url: Option<String>,
    read_at: Option<chrono::DateTime<chrono::Utc>>,
    object_key: Option<String>,
}

async fn kept(pool: &PgPool, collection: Uuid) -> Vec<Kept> {
    sqlx::query_as("SELECT id, name, mime_type, status, source_url, read_at, object_key FROM knowledge_documents WHERE collection_id = $1 ORDER BY created_at")
        .bind(collection)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn reads(pool: &PgPool, owner: Uuid) -> Vec<(String, String, Option<String>, Option<Uuid>)> {
    sqlx::query_as("SELECT url, status, error, document_id FROM notebook_page_reads WHERE owner_id = $1 ORDER BY requested_at, id")
        .bind(owner)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn ticked(pool: &PgPool, owner: Uuid, project: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT document_id FROM notebook_sources WHERE owner_id = $1 AND project_id = $2",
    )
    .bind(owner)
    .bind(project)
    .fetch_all(pool)
    .await
    .unwrap()
}

// ---- Adding pages ----

#[tokio::test]
async fn added_pages_are_read_in_the_persons_browser_and_kept_as_ticked_sources() {
    let Some(f) = fixture().await else { return };
    let pool = &f.db.pool;
    let (site, hits) = counting_site().await;
    let ana = person(pool, "ana").await;
    let p = project(pool, ana, None).await;
    let post = "https://example.org/post?id=7".to_owned();
    f.browser.site(
        &post,
        Site::Page(200, page(&post, "Warmup schedules", &long_text("Warmup"))),
    );
    let pdf = "https://example.org/files/paper.pdf".to_owned();
    f.browser
        .site(&pdf, Site::Pdf(b"%PDF-1.7\n1 0 obj".to_vec()));

    // The owner has no collection in the project yet: one is made, as Add does.
    let ids = add_pages(
        pool,
        ana,
        p,
        &[post.clone(), " ".into(), pdf.clone(), post.clone()],
    )
    .await
    .unwrap();
    assert_eq!(ids.len(), 2, "blank lines and repeats are left out");
    let listed = view(&f.s, &crate::notebook::tests::identity(ana, "ana"), p)
        .await
        .unwrap();
    let group = &listed["collections"][0];
    assert_eq!(group["reading"].as_array().unwrap().len(), 2);
    assert_eq!(group["reading"][0]["status"], "reading");
    assert_eq!(listed["webPages"], true);
    let c: Uuid = serde_json::from_value(group["id"].clone()).unwrap();

    // A read of an address on a site that counts its visitors, written as an address
    // Chat itself could reach: only the browser may load it.
    let counted = format!("{site}/report");
    f.browser.site(
        &counted,
        Site::Page(200, page(&counted, "Counted", &long_text("Counted"))),
    );
    sqlx::query("INSERT INTO notebook_page_reads (id, owner_id, project_id, collection_id, url, requested_at) VALUES ($1, $2, $3, $4, $5, NOW() + INTERVAL '1 second')")
        .bind(Uuid::new_v4())
        .bind(ana)
        .bind(p)
        .bind(c)
        .bind(&counted)
        .execute(pool)
        .await
        .unwrap();

    drain(f.s.clone(), ana).await;
    let docs = kept(pool, c).await;
    assert_eq!(docs.len(), 3, "{docs:?}");
    let (page_doc, pdf_doc) = (&docs[0], &docs[1]);
    assert_eq!(
        (
            page_doc.name.as_str(),
            page_doc.mime_type.as_str(),
            page_doc.status.as_str()
        ),
        ("Warmup schedules.md", "text/plain; charset=utf-8", "queued")
    );
    assert_eq!(page_doc.source_url.as_deref(), Some(post.as_str()));
    assert!(page_doc.read_at.is_some());
    assert_eq!(
        (pdf_doc.name.as_str(), pdf_doc.mime_type.as_str()),
        ("paper.pdf", "application/pdf")
    );
    // Stored like an upload, under the person's own prefix.
    let stored: Vec<String> = f
        .bucket
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == "PUT")
        .map(|(_, k)| k.clone())
        .collect();
    assert!(
        stored.contains(&format!("knowledge/{ana}/{}", page_doc.id)),
        "{stored:?}"
    );
    // Ticked for the person who added them, and the reads are done.
    let mut want = vec![page_doc.id, pdf_doc.id, docs[2].id];
    want.sort();
    let mut got = ticked(pool, ana, p).await;
    got.sort();
    assert_eq!(got, want);
    let done = reads(pool, ana).await;
    assert!(done.iter().all(|r| r.1 == "done"), "{done:?}");
    // Read in the browser, held at its answer, the PDF taken as a file; never by Chat.
    let calls = f.browser.calls();
    assert!(
        calls.contains(&format!("Page.navigate {post}")),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"Fetch.takeResponseBodyAsStream".to_owned()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("Page.navigate {counted}")),
        "{calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| *c == "Target.closeTarget").count(),
        3,
        "{calls:?}"
    );
    assert_eq!(*hits.lock().unwrap(), 0, "Chat fetched an address itself");
    // The notebook shows the page with its address and when it was read.
    let listed = view(&f.s, &crate::notebook::tests::identity(ana, "ana"), p)
        .await
        .unwrap();
    let group = &listed["collections"][0];
    assert_eq!(group["reading"].as_array().unwrap().len(), 0);
    let shown = group["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == json!(page_doc.id))
        .unwrap();
    assert_eq!(shown["sourceUrl"], json!(post));
    assert!(shown["readAt"].is_string());
    f.db.close().await;
}

#[tokio::test]
async fn a_page_that_does_not_read_says_why_and_can_be_tried_again_or_removed() {
    let Some(f) = fixture().await else { return };
    let pool = &f.db.pool;
    let ana = person(pool, "ana").await;
    let p = project(pool, ana, None).await;
    let c = collection(pool, ana, None, Some(p)).await;
    let gone = "https://example.org/gone".to_owned();
    let signin = "https://example.org/account".to_owned();
    let empty = "https://example.org/empty".to_owned();
    let huge = "https://example.org/huge".to_owned();
    let nowhere = "https://nowhere.example.org/".to_owned();
    f.browser.site(&gone, Site::Page(404, Value::Null));
    f.browser.site(&signin, Site::Page(200, json!({"url":signin,"title":"Sign in","text":"Sign in\n\nEmail\n\nPassword","password":true})));
    f.browser
        .site(&empty, Site::Page(200, page(&empty, "Loading", "Loading…")));
    f.browser.site(
        &huge,
        Site::Page(
            200,
            json!({"url":huge,"title":"Huge","text":"a","over":true}),
        ),
    );
    f.browser.site(&nowhere, Site::Unreachable);
    add_pages(
        pool,
        ana,
        p,
        &[
            gone.clone(),
            signin.clone(),
            empty.clone(),
            huge.clone(),
            nowhere.clone(),
        ],
    )
    .await
    .unwrap();
    drain(f.s.clone(), ana).await;
    let failed = reads(pool, ana).await;
    let why: Vec<String> = failed
        .iter()
        .map(|r| {
            assert_eq!(r.1, "failed", "{r:?}");
            r.2.clone().unwrap()
        })
        .collect();
    for (n, said) in [
        "not found (HTTP 404)",
        "asked for a sign-in",
        "no readable text",
        "more text than a notebook source can hold",
        "site was not found",
    ]
    .iter()
    .enumerate()
    {
        assert!(why[n].contains(said), "{}: {}", failed[n].0, why[n]);
    }
    assert!(kept(pool, c).await.is_empty());
    // Failed pages show with their reason; one is tried again, one removed.
    let listed = view(&f.s, &crate::notebook::tests::identity(ana, "ana"), p)
        .await
        .unwrap();
    let shown = listed["collections"][0]["reading"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(shown.len(), 5);
    let id_of = |url: &str| -> Uuid {
        serde_json::from_value(shown.iter().find(|r| r["url"] == url).unwrap()["id"].clone())
            .unwrap()
    };
    f.browser.site(
        &gone,
        Site::Page(200, page(&gone, "Back again", &long_text("Back"))),
    );
    // Someone else may neither retry nor remove it.
    let bo = person(pool, "bo").await;
    assert_eq!(
        retry(pool, bo, id_of(&gone)).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        remove(pool, bo, id_of(&nowhere)).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(retry(pool, ana, id_of(&gone)).await.unwrap(), p);
    assert_eq!(remove(pool, ana, id_of(&nowhere)).await.unwrap(), p);
    drain(f.s.clone(), ana).await;
    let docs = kept(pool, c).await;
    assert_eq!(
        docs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["Back again.md"]
    );
    let left = reads(pool, ana).await;
    assert_eq!(left.len(), 4);
    assert!(left.iter().any(|r| r.0 == gone && r.1 == "done"));
    f.db.close().await;
}

#[tokio::test]
async fn reads_are_capped_per_request_per_person_and_per_project_each_day() {
    let Some(db) = Database::new().await else {
        return;
    };
    let pool = &db.pool;
    let (ana, bo) = (person(pool, "ana").await, person(pool, "bo").await);
    let team = group(pool, &[ana, bo]).await;
    let p = project(pool, ana, Some(team)).await;
    collection(pool, ana, None, Some(p)).await;
    collection(pool, bo, None, Some(p)).await;
    let many: Vec<String> = (0..=PAGES_PER_REQUEST)
        .map(|n| format!("https://example.org/{n}"))
        .collect();
    let err = add_pages(pool, ana, p, &many).await.unwrap_err();
    assert!(err.1.contains("up to 10"), "{}", err.1);
    let err = add_pages(
        pool,
        ana,
        p,
        &["https://example.org/a".into(), "http://127.0.0.1/".into()],
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    assert!(
        err.1.contains("127.0.0.1") && err.1.contains("public"),
        "{}",
        err.1
    );
    // Waiting reads per person.
    for round in 0..2 {
        let batch: Vec<String> = (0..10)
            .map(|n| format!("https://example.org/{round}/{n}"))
            .collect();
        add_pages(pool, ana, p, &batch).await.unwrap();
    }
    let err = add_pages(pool, ana, p, &["https://example.org/more".into()])
        .await
        .unwrap_err();
    assert_eq!(err.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(err.1.contains("waiting"), "{}", err.1);
    // Reads per project in a day count everyone's, done or not.
    sqlx::query("UPDATE notebook_page_reads SET status = 'done'")
        .execute(pool)
        .await
        .unwrap();
    for round in 2..10 {
        let batch: Vec<String> = (0..10)
            .map(|n| format!("https://example.org/{round}/{n}"))
            .collect();
        add_pages(pool, if round % 2 == 0 { ana } else { bo }, p, &batch)
            .await
            .unwrap();
        sqlx::query("UPDATE notebook_page_reads SET status = 'done'")
            .execute(pool)
            .await
            .unwrap();
    }
    let err = add_pages(pool, bo, p, &["https://example.org/101".into()])
        .await
        .unwrap_err();
    assert_eq!(err.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(err.1.contains("for the day"), "{}", err.1);
    // A day later there is room again.
    sqlx::query("UPDATE notebook_page_reads SET requested_at = NOW() - INTERVAL '25 hours'")
        .execute(pool)
        .await
        .unwrap();
    add_pages(pool, bo, p, &["https://example.org/101".into()])
        .await
        .unwrap();
    db.close().await;
}

#[tokio::test]
async fn pages_go_only_into_a_project_the_person_may_use() {
    let Some(db) = Database::new().await else {
        return;
    };
    let pool = &db.pool;
    let (ana, bo, cy) = (
        person(pool, "ana").await,
        person(pool, "bo").await,
        person(pool, "cy").await,
    );
    let team = group(pool, &[ana, bo]).await;
    let p = project(pool, ana, Some(team)).await;
    let url = vec!["https://example.org/a".to_owned()];
    // Outside the project: not found. A member with no collection of their own there
    // cannot have one made; the project's owner gets one.
    assert_eq!(
        add_pages(pool, cy, p, &url).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        add_pages(pool, bo, p, &url).await.unwrap_err().0,
        StatusCode::FORBIDDEN
    );
    add_pages(pool, ana, p, &url).await.unwrap();
    let theirs: Vec<Uuid> = sqlx::query_scalar("SELECT c.owner_id FROM notebook_page_reads r JOIN knowledge_collections c ON c.id = r.collection_id")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(theirs, vec![ana]);
    db.close().await;
}

// ---- Reading again ----

#[tokio::test]
async fn reading_again_replaces_the_content_and_keeps_the_document_and_its_tick() {
    let Some(f) = fixture().await else { return };
    let pool = &f.db.pool;
    let (ana, bo) = (person(pool, "ana").await, person(pool, "bo").await);
    let team = group(pool, &[ana, bo]).await;
    let p = project(pool, ana, Some(team)).await;
    let c = collection(pool, ana, None, Some(p)).await;
    let url = "https://example.org/notes".to_owned();
    let doc = ready_document(pool, ana, c, "Notes.md", &["old words about warmup"]).await;
    sqlx::query("UPDATE knowledge_documents SET source_url = $2, read_at = NOW() - INTERVAL '3 days' WHERE id = $1")
        .bind(doc)
        .bind(&url)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO notebook_guides (document_id, owner_id, status, summary) VALUES ($1, $2, 'ready', 'Old guide')")
        .bind(doc)
        .bind(ana)
        .execute(pool)
        .await
        .unwrap();
    tick(pool, ana, p, &[doc], true).await.unwrap();
    tick(pool, bo, p, &[doc], true).await.unwrap();
    let old_key: String =
        sqlx::query_scalar("SELECT object_key FROM knowledge_documents WHERE id = $1")
            .bind(doc)
            .fetch_one(pool)
            .await
            .unwrap();

    // Only the collection's owner reads it again, and only a web page.
    assert_eq!(
        read_again(pool, bo, p, doc).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    let upload = ready_document(pool, ana, c, "upload.pdf", &["x"]).await;
    assert_eq!(
        read_again(pool, ana, p, upload).await.unwrap_err().0,
        StatusCode::BAD_REQUEST
    );

    f.browser.site(
        &url,
        Site::Page(200, page(&url, "Notes, revised", &long_text("Revised"))),
    );
    read_again(pool, ana, p, doc).await.unwrap();
    assert_eq!(
        read_again(pool, ana, p, doc).await.unwrap_err().0,
        StatusCode::CONFLICT
    );
    let listed = view(&f.s, &crate::notebook::tests::identity(ana, "ana"), p)
        .await
        .unwrap();
    let row = listed["collections"][0]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == json!(doc))
        .unwrap()
        .clone();
    assert_eq!(row["reading"]["status"], "reading");
    assert!(
        listed["collections"][0]["reading"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a read again is shown on its document"
    );
    drain(f.s.clone(), ana).await;

    let now = kept(pool, c)
        .await
        .into_iter()
        .find(|d| d.id == doc)
        .unwrap();
    assert_eq!(
        (now.name.as_str(), now.status.as_str()),
        ("Notes, revised.md", "queued")
    );
    assert!(now.read_at.unwrap() > chrono::Utc::now() - chrono::Duration::minutes(1));
    assert_ne!(now.object_key.as_deref(), Some(old_key.as_str()));
    let (passages, guides): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM knowledge_passages WHERE document_id = $1), (SELECT COUNT(*) FROM notebook_guides WHERE document_id = $1)",
    )
    .bind(doc)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        (passages, guides),
        (0, 0),
        "indexed and guided again from the new text"
    );
    let trashed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM object_trash WHERE object_key = $1 AND NOT pending)",
    )
    .bind(&old_key)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(trashed, "the old file is removed from storage");
    assert!(ticked(pool, ana, p).await.contains(&doc));
    assert!(ticked(pool, bo, p).await.contains(&doc));

    // A failed reading again leaves the document as it was and says why on it.
    f.browser.site(&url, Site::Page(503, Value::Null));
    read_again(pool, ana, p, doc).await.unwrap();
    drain(f.s.clone(), ana).await;
    let listed = view(&f.s, &crate::notebook::tests::identity(ana, "ana"), p)
        .await
        .unwrap();
    let row = listed["collections"][0]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == json!(doc))
        .unwrap()
        .clone();
    assert_eq!(row["reading"]["status"], "failed");
    assert!(row["reading"]["error"]
        .as_str()
        .unwrap()
        .contains("HTTP 503"));
    assert_eq!(
        kept(pool, c)
            .await
            .into_iter()
            .find(|d| d.id == doc)
            .unwrap()
            .name,
        "Notes, revised.md"
    );
    f.db.close().await;
}

// ---- From a chat's browser tab, and the model's tool ----

async fn chat_with_browser(pool: &PgPool, owner: Uuid, project: Option<Uuid>, on: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO conversations (id, owner_id, title, model, project_id, browser) VALUES ($1, $2, 'Reading', 'm', $3, $4)")
        .bind(id)
        .bind(owner)
        .bind(project)
        .bind(on)
        .execute(pool)
        .await
        .unwrap();
    let browser = Uuid::new_v4();
    sqlx::query("INSERT INTO browsers (id, owner_id, slug, status) VALUES ($1, $2, $3, 'running') ON CONFLICT (owner_id) DO NOTHING")
        .bind(browser)
        .bind(owner)
        .bind(owner.simple().to_string()[..16].to_owned())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO browser_tabs (conversation_id, browser_id, target_id) SELECT $1, id, 'TAB' FROM browsers WHERE owner_id = $2")
        .bind(id)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn the_page_a_chats_tab_shows_is_added_as_it_is_now() {
    let Some(f) = fixture().await else { return };
    let pool = &f.db.pool;
    let ana = person(pool, "ana").await;
    let me = crate::notebook::tests::identity(ana, "ana");
    let p = project(pool, ana, None).await;
    let c = collection(pool, ana, None, Some(p)).await;
    let chat = chat_with_browser(pool, ana, Some(p), true).await;
    let url = "https://example.org/dashboard/report?week=46";
    *f.browser.tab.lock().unwrap() = Some(page(url, "Weekly report", &long_text("Report")));
    let added = add_from_tab(&f.s, &me, chat, p).await.unwrap();
    assert_eq!(added.url, url);
    let docs = kept(pool, c).await;
    assert_eq!(docs.len(), 1);
    assert_eq!(
        (docs[0].name.as_str(), docs[0].source_url.as_deref()),
        ("Weekly report.md", Some(url))
    );
    assert_eq!(ticked(pool, ana, p).await, vec![docs[0].id]);
    // The tab is read where it stands: nothing navigates it.
    assert!(
        !f.browser
            .calls()
            .iter()
            .any(|c| c.starts_with("Page.navigate")),
        "{:?}",
        f.browser.calls()
    );
    // It counts with the project's reads for the day.
    let counted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notebook_page_reads WHERE project_id = $1 AND status = 'done'",
    )
    .bind(p)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(counted, 1);

    // Not with the browser off in the chat, nor another person's chat.
    let off = chat_with_browser(pool, ana, Some(p), false).await;
    assert_eq!(
        add_from_tab(&f.s, &me, off, p).await.unwrap_err().0,
        StatusCode::CONFLICT
    );
    let bo = person(pool, "bo").await;
    let bos = chat_with_browser(pool, bo, None, true).await;
    assert!(add_from_tab(&f.s, &me, bos, p).await.is_err());
    // A tab with no web page in it, or a sign-in page, is not kept.
    *f.browser.tab.lock().unwrap() = Some(page("about:blank", "", ""));
    assert!(add_from_tab(&f.s, &me, chat, p)
        .await
        .unwrap_err()
        .1
        .contains("no web page"));
    *f.browser.tab.lock().unwrap() = Some(
        json!({"url":"https://example.org/login","title":"Log in","text":"Log in","password":true}),
    );
    assert!(add_from_tab(&f.s, &me, chat, p)
        .await
        .unwrap_err()
        .1
        .contains("sign-in"));
    assert_eq!(kept(pool, c).await.len(), 1);
    // A PDF in the tab's viewer is taken as the file, read again from its address in
    // a tab of its own in the same browser.
    let pdf = "https://example.org/files/slides.pdf".to_owned();
    f.browser.site(&pdf, Site::Pdf(b"%PDF-1.5\n".to_vec()));
    *f.browser.tab_pdf.lock().unwrap() = Some(pdf.clone());
    let added = add_from_tab(&f.s, &me, chat, p).await.unwrap();
    assert_eq!(
        (added.name.as_str(), added.url.as_str()),
        ("slides.pdf", pdf.as_str())
    );
    assert!(f.browser.calls().contains(&format!("Page.navigate {pdf}")));
    f.db.close().await;
}

#[tokio::test]
async fn the_model_adds_only_its_own_chats_page_and_not_while_the_person_drives() {
    let Some(f) = fixture().await else { return };
    let pool = &f.db.pool;
    let ana = person(pool, "ana").await;
    let me = crate::notebook::tests::identity(ana, "ana");
    let p = project(pool, ana, None).await;
    let c = collection(pool, ana, None, Some(p)).await;
    let chat = chat_with_browser(pool, ana, Some(p), true).await;
    *f.browser.tab.lock().unwrap() = Some(page(
        "https://example.org/paper",
        "A paper",
        &long_text("Paper"),
    ));
    let (ok, summary, output) = run_tool(&f.s, &me, chat, p).await;
    assert!(ok, "{output}");
    assert_eq!(summary, "Added “A paper” to the notebook");
    assert!(
        output.contains("example.org")
            && !output.contains("warms up")
            && !output.contains("A paper"),
        "the model gets none of the page's own words: {output}"
    );
    assert_eq!(kept(pool, c).await.len(), 1);
    sqlx::query("UPDATE browser_tabs SET controller = 'user' WHERE conversation_id = $1")
        .bind(chat)
        .execute(pool)
        .await
        .unwrap();
    let (ok, _, output) = run_tool(&f.s, &me, chat, p).await;
    assert!(!ok && output.contains("has control"), "{output}");
    assert_eq!(kept(pool, c).await.len(), 1);
    let definition = definition();
    assert_eq!(definition["function"]["name"], TOOL);
    assert!(definition["function"]["description"]
        .as_str()
        .unwrap()
        .contains("only when the person asks"));
    f.db.close().await;
}

#[tokio::test]
async fn reads_left_by_a_stopped_replica_are_taken_up_then_failed() {
    let Some(f) = fixture().await else { return };
    let pool = &f.db.pool;
    let ana = person(pool, "ana").await;
    let p = project(pool, ana, None).await;
    collection(pool, ana, None, Some(p)).await;
    let url = "https://example.org/slow".to_owned();
    add_pages(pool, ana, p, std::slice::from_ref(&url))
        .await
        .unwrap();
    // Taken up by a replica that stopped, twice.
    sqlx::query("UPDATE notebook_page_reads SET claimed_at = NOW() - INTERVAL '7 minutes', attempts = 2, requested_at = NOW() - INTERVAL '8 minutes'")
        .execute(pool)
        .await
        .unwrap();
    recover(&f.s).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let left = reads(pool, ana).await;
    assert_eq!(left[0].1, "failed");
    assert!(left[0].2.as_deref().unwrap().contains("stopped"));
    // One left once is read by the next pass.
    f.browser.site(
        &url,
        Site::Page(200, page(&url, "Slow page", &long_text("Slow"))),
    );
    sqlx::query("UPDATE notebook_page_reads SET status = 'reading', error = NULL, attempts = 1")
        .execute(pool)
        .await
        .unwrap();
    recover(&f.s).await.unwrap();
    for _ in 0..50 {
        if reads(pool, ana).await[0].1 == "done" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(reads(pool, ana).await[0].1, "done");
    f.db.close().await;
}
