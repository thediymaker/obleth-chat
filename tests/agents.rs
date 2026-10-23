//! Worker agents end to end: a stand-in gateway plays the worker model, a local
//! folder stands in for the chat's workspace, and usage lands in the real ledger.
use axum::{
    response::{sse::Event, Sse},
    routing::{get, post},
    Json, Router,
};
use obleth_chat::{
    agents,
    auth::{Identity, Role},
    code::workspace::{Raw, Shell},
    gateway::Gateway,
    models,
};
use rsa::{pkcs8::EncodePrivateKey, RsaPrivateKey};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{
    convert::Infallible,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// A chat's folder on this machine, run the way a workspace pod runs scripts.
struct Local(PathBuf);

impl Shell for Local {
    async fn exec(
        &self,
        script: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
        limit: usize,
    ) -> Result<Raw, String> {
        let dir = self.0.display().to_string();
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "mkdir -p '{dir}' && cd '{dir}' || exit 1\n{script}"
            ))
            .arg("sh")
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut input = child.stdin.take().unwrap();
        if let Some(data) = stdin {
            input.write_all(data).await.map_err(|e| e.to_string())?;
        }
        drop(input);
        let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
        Ok(Raw {
            truncated: out.stdout.len() > limit,
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        })
    }
}

type Seen = Arc<Mutex<Vec<Value>>>;

/// The worker model: first it searches, then it reports what the search found. With
/// no tools it answers straight away.
async fn chat(
    axum::extract::State(seen): axum::extract::State<Seen>,
    Json(body): Json<Value>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    seen.lock().unwrap().push(body.clone());
    let messages = body["messages"].as_array().unwrap().clone();
    let task = messages[1]["content"].as_str().unwrap_or("").to_owned();
    let tools = body.get("tools").is_some();
    let found = messages
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .and_then(|m| m["content"].as_str())
        .map(str::to_owned);
    let stream = async_stream::stream! {
        match found {
            None if !tools => {
                // Thinking streams token by token: far more stream than answer.
                if task.contains("Think hard") {
                    for _ in 0..8000 {
                        yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"reasoning_content":"mull "}}]}).to_string()));
                    }
                }
                yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"content":format!("From memory: {task}")}}]}).to_string()));
                yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110}}).to_string()));
            }
            None => {
                let pattern = if task.contains("schedule") { "schedule" } else { "# Jobs" };
                let call = json!([{"index":0,"id":"call_1","type":"function","function":{"name":"search","arguments":json!({"pattern":pattern}).to_string()}}]);
                yield Ok::<_, Infallible>(Event::default().data(json!({"choices":[{"delta":{"tool_calls":call}}]}).to_string()));
                yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":200,"completion_tokens":20,"total_tokens":220}}).to_string()));
            }
            Some(found) => {
                let first = found.lines().next().unwrap_or("").to_owned();
                yield Ok(Event::default().data(json!({"choices":[{"delta":{"content":format!("Found it: {first}")}}]}).to_string()));
                yield Ok(Event::default().data(json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":300,"completion_tokens":30,"total_tokens":330}}).to_string()));
            }
        }
        yield Ok(Event::default().data("[DONE]"));
    };
    Sse::new(stream)
}

async fn gateway(seen: Seen) -> Gateway {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mocks = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"lead-model"},{"id":"mini-worker"}]})) }),
        )
        .route("/v1/chat/completions", post(chat).with_state(seen));
    tokio::spawn(async move { axum::serve(listener, mocks).await.unwrap() });
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

async fn person(pool: &PgPool) -> Identity {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, display_name) VALUES ($1,'jen','Jen')")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    Identity {
        id,
        username: "jen".into(),
        display_name: "Jen".into(),
        role: Role::User,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn workers_explore_at_once_and_report(pool: PgPool) {
    let jen = person(&pool).await;
    // Only models administrators marked as workers are offered.
    assert!(models::worker_models(&pool, &jen).await.is_empty());
    sqlx::query("INSERT INTO model_profiles (model_id, name, worker) VALUES ('mini-worker','Mini',TRUE), ('lead-model','Lead',FALSE)")
        .execute(&pool)
        .await
        .unwrap();
    let workers = models::worker_models(&pool, &jen).await;
    assert_eq!(workers, [("mini-worker".to_string(), "Mini".to_string())]);
    let definition = agents::definition(&workers, agents::Files::Workspace);
    assert_eq!(definition["function"]["name"], "delegate");
    let properties = &definition["function"]["parameters"]["properties"];
    assert_eq!(properties["model"]["enum"], json!(["mini-worker"]));
    assert!(properties["repo"].is_object() && properties["project_code"].is_null());
    // A chat without a workspace still has workers; files depend on what Chat may open.
    let plain = agents::definition(&workers, agents::Files::None);
    assert!(plain["function"]["parameters"]["properties"]["repo"].is_null());
    let project = agents::definition(&workers, agents::Files::Launch { project: true });
    assert!(project["function"]["parameters"]["properties"]["project_code"].is_object());
    assert!(project["function"]["description"]
        .as_str()
        .unwrap()
        .contains("turns on this chat's workspace"));

    let dir = tempfile::tempdir().unwrap();
    let folder = Local(dir.path().join("chat"));
    std::fs::create_dir_all(folder.0.join("app")).unwrap();
    std::fs::write(folder.0.join("app/jobs.py"), "def run():\n    schedule()\n").unwrap();
    std::fs::write(folder.0.join("README.md"), "# Jobs\n").unwrap();
    let seen: Seen = Arc::default();
    let gateway = gateway(seen.clone()).await;
    let chat = Uuid::new_v4();
    let ctx = agents::Ctx {
        pool: &pool,
        gateway: &gateway,
        user: &jen,
        conversation: chat,
        shell: Some(&folder),
        workers: &workers,
        note: String::new(),
    };
    let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let outcome = agents::run(
        &ctx,
        &json!({"tasks":[
            {"title":"Find the scheduler","task":"Find where jobs schedule work."},
            {"title":"Read the docs","task":"Summarize the README."}
        ]})
        .to_string(),
        progress,
    )
    .await;
    assert!(outcome.ok, "{}", outcome.output);
    assert_eq!(outcome.summary, "2 agents reported");
    assert!(
        outcome
            .output
            .contains("## 1. Find the scheduler\nFound it: app/jobs.py:2:    schedule()"),
        "{}",
        outcome.output
    );
    assert!(outcome
        .output
        .contains("## 2. Read the docs\nFound it: README.md:1:# Jobs"));
    let agents = outcome.extra["agents"].as_array().unwrap();
    assert!(agents
        .iter()
        .all(|a| a["status"] == "completed" && a["steps"] == 2));
    assert_eq!(agents[0]["tokens"], 550);
    // The live card saw them start, work and finish.
    let mut snapshots = vec![];
    while let Ok(s) = updates.try_recv() {
        snapshots.push(s);
    }
    assert!(snapshots
        .iter()
        .any(|s| s[0]["doing"] == "Searching for “schedule”"));
    // Workers got their task alone, read-only tools, and little thinking.
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 4);
    assert!(requests.iter().all(|r| r["model"] == "mini-worker"));
    assert_eq!(requests[0]["reasoning_effort"], "low");
    let names: Vec<&str> = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["list_files", "read_file", "search", "git_log"]);
    // Each worker is charged on its own row.
    let rows: Vec<(String, String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT purpose, status, total_tokens, model FROM generation_usage WHERE user_id=$1",
    )
    .bind(jen.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.0 == "agent"
        && r.1 == "completed"
        && r.2 == Some(550)
        && r.3.as_deref() == Some("mini-worker")));

    // A model that is not a worker is refused.
    let (progress, _updates) = tokio::sync::mpsc::unbounded_channel();
    let refused = agents::run(
        &ctx,
        &json!({"model":"lead-model","tasks":[{"title":"x","task":"x"}]}).to_string(),
        progress,
    )
    .await;
    assert!(!refused.ok && refused.output.contains("mini-worker"));

    // Without files a worker is a short chat of its own: no tools, one answer.
    seen.lock().unwrap().clear();
    let chatting = agents::Ctx {
        shell: None::<&Local>,
        note: "Context from the lead.".into(),
        ..ctx
    };
    let (progress, _updates) = tokio::sync::mpsc::unbounded_channel();
    let answered = agents::run(
        &chatting,
        &json!({"tasks":[
            {"title":"Compare","task":"Compare two queue designs."},
            {"title":"Ponder","task":"Think hard about pi."}
        ]})
        .to_string(),
        progress,
    )
    .await;
    assert!(answered.ok, "{}", answered.output);
    assert!(answered
        .output
        .contains("From memory: Context from the lead.\n\nCompare two queue designs."));
    let requests = seen.lock().unwrap().clone();
    assert!(answered
        .output
        .contains("From memory: Context from the lead.\n\nThink hard about pi."));
    assert_eq!(answered.summary, "2 agents reported");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].get("tools").is_none());
    assert!(requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("no tools or files"));
}

#[tokio::test]
async fn repositories_are_checked_out_once() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::write(origin.join("lib.rs"), "pub fn answer() -> u8 { 42 }\n").unwrap();
    let git = |args: &[&str], cwd: &std::path::Path| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"], &origin);
    git(&["add", "."], &origin);
    git(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@e",
            "commit",
            "-qm",
            "First",
        ],
        &origin,
    );
    // The chat's folder is itself a repository: the checkout stays out of it.
    let folder = Local(dir.path().join("chat"));
    std::fs::create_dir_all(&folder.0).unwrap();
    git(&["init", "-q"], &folder.0);
    let url = format!("file://{}", origin.display());
    let at = agents::checkout(&folder, &url, "origin").await.unwrap();
    assert_eq!(at, "repos/origin");
    assert!(
        agents::tool(&folder, "search", &json!({"pattern":"answer"}))
            .await
            .contains("repos/origin/lib.rs:1:")
    );
    let log = agents::tool(&folder, "git_log", &json!({"path":"repos/origin"})).await;
    assert!(log.contains("First"), "{log}");
    let exclude = std::fs::read_to_string(folder.0.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("/repos/").count(), 1);
    // A second checkout reuses the first.
    assert_eq!(
        agents::checkout(&folder, &url, "origin").await.unwrap(),
        "repos/origin"
    );
    let exclude = std::fs::read_to_string(folder.0.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("/repos/").count(), 1);
    let missing = agents::checkout(&folder, &format!("{url}-gone"), "gone")
        .await
        .unwrap_err();
    assert!(missing.starts_with("Could not check out"), "{missing}");
    assert!(!folder.0.join("repos/gone.part").exists());
}

#[tokio::test]
async fn worker_tools_only_read_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let folder = Local(dir.path().join("chat"));
    std::fs::create_dir_all(folder.0.join("src")).unwrap();
    std::fs::create_dir_all(folder.0.join("node_modules/x")).unwrap();
    std::fs::write(folder.0.join("node_modules/x/index.js"), "needle\n").unwrap();
    let lines: String = (1..=400).map(|n| format!("line {n}\n")).collect();
    std::fs::write(folder.0.join("src/long.txt"), &lines).unwrap();
    std::fs::write(folder.0.join("src/main.rs"), "fn main() { needle(); }\n").unwrap();
    let list = agents::tool(&folder, "list_files", &json!({})).await;
    assert!(
        list.contains("src/main.rs  24 bytes") && list.contains("src/"),
        "{list}"
    );
    assert!(!list.contains("node_modules"), "{list}");
    let read = agents::tool(
        &folder,
        "read_file",
        &json!({"path":"src/long.txt","start":5}),
    )
    .await;
    assert!(read.starts_with("5\tline 5\n"), "{read}");
    assert!(read.ends_with("[lines 5-304 of 400]\n"), "{read}");
    let found = agents::tool(&folder, "search", &json!({"pattern":"needle"})).await;
    assert_eq!(found.trim(), "src/main.rs:1:fn main() { needle(); }");
    let only = agents::tool(
        &folder,
        "search",
        &json!({"pattern":"line 39[0-9]","regex":true,"glob":"*.txt"}),
    )
    .await;
    assert_eq!(only.lines().count(), 10, "{only}");
    for (name, args) in [
        ("read_file", json!({"path":"../../etc/passwd"})),
        ("list_files", json!({"path":"/"})),
        ("write_file", json!({"path":"x"})),
    ] {
        let out = agents::tool(&folder, name, &args).await;
        assert!(
            out.contains("not a path inside") || out.contains("no tool named"),
            "{name}: {out}"
        );
    }
    let log = agents::tool(&folder, "git_log", &json!({})).await;
    assert_eq!(log.trim(), "This project is not a git repository.");
}
