//! Project code end to end, against real PostgreSQL and git: two people's workspaces
//! (local folders here, standing in for workspace pods) sync with one project.
use obleth_chat::{
    auth::{Identity, Role},
    code::{
        self,
        repo::Repo,
        workspace::{self, Raw, Shell},
    },
};
use serde_json::Value;
use sqlx::PgPool;
use std::path::{Path, PathBuf};
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
            .env("HOME", &self.0)
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

impl Local {
    fn write(&self, path: &str, text: &str) {
        let file = self.0.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(path)).ok()
    }
}

async fn person(pool: &PgPool, name: &str) -> Identity {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, display_name) VALUES ($1,$2,$3)")
        .bind(id)
        .bind(name.to_lowercase())
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    Identity {
        id,
        username: name.to_lowercase(),
        display_name: name.into(),
        role: Role::User,
    }
}

async fn chat(pool: &PgPool, owner: &Identity, project: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO conversations (id, owner_id, title, model, project_id) VALUES ($1,$2,'Work','m',$3)")
        .bind(id)
        .bind(owner.id)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn project(pool: &PgPool, owner: &Identity, group: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, owner_id, name, scope, group_id) VALUES ($1,$2,'Benchmarks',$3,$4)")
        .bind(id)
        .bind(owner.id)
        .bind(if group.is_some() { "group" } else { "personal" })
        .bind(group)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn server_git(pool: &PgPool, project: Uuid, args: &[&str]) -> String {
    let mut db = pool.acquire().await.unwrap();
    let repo = Repo::load(&mut db, project).await.unwrap();
    repo.git(args).await.unwrap().text()
}

fn state(v: &Value) -> &str {
    v["state"].as_str().unwrap()
}

fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn folder(root: &Path, name: &str) -> Local {
    Local(root.join(name))
}

#[sqlx::test(migrations = "./migrations")]
async fn two_people_sync_one_project(pool: PgPool) {
    let jen = person(&pool, "Jen").await;
    let kim = person(&pool, "Kim").await;
    let group = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups (id, name) VALUES ($1,'Lab')")
        .bind(group)
        .execute(&pool)
        .await
        .unwrap();
    for user in [&jen, &kim] {
        sqlx::query("INSERT INTO user_group_members (group_id, user_id) VALUES ($1,$2)")
            .bind(group)
            .bind(user.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let p = project(&pool, &jen, Some(group)).await;
    let jen_chat = chat(&pool, &jen, p).await;
    let kim_chat = chat(&pool, &kim, p).await;
    let dirs = root();
    let a = folder(dirs.path(), "jen");
    let b = folder(dirs.path(), "kim");

    // Jen starts the project's code from her workspace; dependencies stay out.
    a.write("plot.py", "colors = 'blue'\n");
    a.write("node_modules/dep/index.js", "x");
    let out = code::sync_chat(&pool, &a, &jen, p, jen_chat, Some("Start the plots"))
        .await
        .unwrap();
    assert_eq!(
        (state(&out), out["shared"].as_bool()),
        ("synced", Some(true))
    );
    let files = server_git(&pool, p, &["ls-tree", "-r", "--name-only", "main"]).await;
    assert_eq!(files.trim(), "plot.py");
    let trailer = server_git(
        &pool,
        p,
        &[
            "log",
            "-1",
            "--format=%s|%an|%(trailers:key=Obleth-Chat,valueonly)",
            "main",
        ],
    )
    .await;
    assert_eq!(trailer.trim(), format!("Start the plots|Jen|{jen_chat}"));

    // Kim's empty workspace gets it on her first sync.
    let out = code::sync_chat(&pool, &b, &kim, p, kim_chat, None)
        .await
        .unwrap();
    assert_eq!(
        (state(&out), out["shared"].as_bool()),
        ("synced", Some(false))
    );
    assert_eq!(b.read("plot.py").as_deref(), Some("colors = 'blue'\n"));

    // Kim changes it and adds a file; the message is drafted for her.
    b.write("plot.py", "colors = 'teal'\n");
    b.write("notes.md", "# Notes\n");
    let report = code::report(&pool, &b, &code::author(&kim), p)
        .await
        .unwrap();
    assert_eq!(report["changes"].as_array().unwrap().len(), 2);
    assert_eq!(report["draft"], "Update notes.md and 1 more file");
    let out = code::sync_chat(&pool, &b, &kim, p, kim_chat, None)
        .await
        .unwrap();
    assert_eq!(out["shared"], true);
    assert_eq!(out["commits"], 2);

    // Jen is now one behind; she changed the same line, so Sync pauses.
    let report = code::report(&pool, &a, &code::author(&jen), p)
        .await
        .unwrap();
    assert_eq!(report["behind"], 1);
    a.write("plot.py", "colors = 'red'\n");
    let out = code::sync_chat(&pool, &a, &jen, p, jen_chat, None)
        .await
        .unwrap();
    assert_eq!(state(&out), "conflict");
    assert_eq!(out["conflicts"], serde_json::json!(["plot.py"]));
    let report = code::report(&pool, &a, &code::author(&jen), p)
        .await
        .unwrap();
    assert_eq!(report["merging"], true);
    assert_eq!(report["conflicts"], serde_json::json!(["plot.py"]));
    // A second Sync while paused is refused; finishing with both versions in the file too.
    assert!(code::sync_chat(&pool, &a, &jen, p, jen_chat, None)
        .await
        .is_err());
    let err = code::finish_chat(&pool, &a, &jen, p, jen_chat)
        .await
        .unwrap_err();
    assert!(err.1.contains("plot.py"), "{}", err.1);
    // Stop puts her version back.
    workspace::abort(&a, &code::author(&jen)).await.unwrap();
    assert_eq!(a.read("plot.py").as_deref(), Some("colors = 'red'\n"));
    assert_eq!(a.read("notes.md"), None);

    // Again, keeping Kim's version this time; her new file comes along.
    let out = code::sync_chat(&pool, &a, &jen, p, jen_chat, None)
        .await
        .unwrap();
    assert_eq!(state(&out), "conflict");
    workspace::choose(&a, &code::author(&jen), "plot.py", "theirs")
        .await
        .unwrap();
    let out = code::finish_chat(&pool, &a, &jen, p, jen_chat)
        .await
        .unwrap();
    assert_eq!(
        (state(&out), out["shared"].as_bool()),
        ("synced", Some(true))
    );
    assert_eq!(a.read("plot.py").as_deref(), Some("colors = 'teal'\n"));
    assert_eq!(a.read("notes.md").as_deref(), Some("# Notes\n"));

    // Combining by hand (as the model would): write both, then finish.
    a.write("plot.py", "colors = 'navy'\n");
    code::sync_chat(&pool, &a, &jen, p, jen_chat, None)
        .await
        .unwrap();
    b.write("plot.py", "colors = 'gold'\n");
    let out = code::sync_chat(&pool, &b, &kim, p, kim_chat, None)
        .await
        .unwrap();
    assert_eq!(state(&out), "conflict");
    b.write("plot.py", "colors = ['navy', 'gold']\n");
    let out = code::finish_chat(&pool, &b, &kim, p, kim_chat)
        .await
        .unwrap();
    assert_eq!(state(&out), "synced");
    let main = server_git(&pool, p, &["show", "main:plot.py"]).await;
    assert_eq!(main, "colors = ['navy', 'gold']\n");
    // The combining commit keeps its links, without git's notes about the conflict.
    let message = server_git(&pool, p, &["log", "-1", "--format=%B", "main"]).await;
    assert!(
        message.starts_with("Bring in the project's latest changes"),
        "{message}"
    );
    assert!(
        message.contains("Obleth-Chat:") && !message.contains("# Conflicts"),
        "{message}"
    );
    // Jen's next Sync brings it in without a conflict.
    let out = code::sync_chat(&pool, &a, &jen, p, jen_chat, None)
        .await
        .unwrap();
    assert_eq!(state(&out), "synced");
    assert_eq!(
        a.read("plot.py").as_deref(),
        Some("colors = ['navy', 'gold']\n")
    );
    let report = code::report(&pool, &a, &code::author(&jen), p)
        .await
        .unwrap();
    assert_eq!(
        (report["behind"].as_i64(), report["ahead"].as_i64()),
        (Some(0), Some(0))
    );

    // Diff and undo, one file or all.
    a.write("plot.py", "colors = []\n");
    a.write("scratch.txt", "tmp\n");
    let diff = workspace::diff(&a, &code::author(&jen), "plot.py")
        .await
        .unwrap();
    assert!(diff.contains("+colors = []"), "{diff}");
    workspace::undo(&a, &code::author(&jen), Some("plot.py"))
        .await
        .unwrap();
    assert_eq!(
        a.read("plot.py").as_deref(),
        Some("colors = ['navy', 'gold']\n")
    );
    assert!(a.read("scratch.txt").is_some());
    workspace::undo(&a, &code::author(&jen), None)
        .await
        .unwrap();
    assert_eq!(a.read("scratch.txt"), None);
    assert!(
        a.0.join("node_modules/dep/index.js").exists(),
        "ignored files stay"
    );

    // Who is working on it, and how far behind.
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT conversation_id, synced FROM conversation_code WHERE project_id = $1",
    )
    .bind(p)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);

    // A chat moved into a new project brings its history with it.
    let p2 = project(&pool, &kim, None).await;
    sqlx::query("UPDATE conversations SET project_id = $2 WHERE id = $1")
        .bind(kim_chat)
        .bind(p2)
        .execute(&pool)
        .await
        .unwrap();
    let out = code::sync_chat(&pool, &b, &kim, p2, kim_chat, None)
        .await
        .unwrap();
    assert_eq!(out["shared"], true);
    let count = server_git(&pool, p2, &["rev-list", "--count", "main"]).await;
    assert!(count.trim().parse::<i32>().unwrap() >= 5, "{count}");
}

#[sqlx::test(migrations = "./migrations")]
async fn many_syncs_fold_into_one_bundle(pool: PgPool) {
    let jen = person(&pool, "Jen").await;
    let p = project(&pool, &jen, None).await;
    let c = chat(&pool, &jen, p).await;
    let dirs = root();
    let a = folder(dirs.path(), "jen");
    for n in 0..32 {
        a.write("count.txt", &format!("{n}\n"));
        let out = code::sync_chat(&pool, &a, &jen, p, c, None).await.unwrap();
        assert_eq!(out["shared"], true);
    }
    let bundles: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM project_code_bundles WHERE project_id = $1")
            .bind(p)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(bundles < 30, "{bundles}");
    assert_eq!(
        server_git(&pool, p, &["rev-list", "--count", "main"])
            .await
            .trim(),
        "32"
    );
    assert_eq!(
        server_git(&pool, p, &["show", "main:count.txt"]).await,
        "31\n"
    );
    // A fresh workspace still gets everything.
    let b = folder(dirs.path(), "fresh");
    let c2 = chat(&pool, &jen, p).await;
    code::sync_chat(&pool, &b, &jen, p, c2, None).await.unwrap();
    assert_eq!(b.read("count.txt").as_deref(), Some("31\n"));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_model_syncs_and_settles_a_conflict(pool: PgPool) {
    let jen = person(&pool, "Jen").await;
    let kim = person(&pool, "Kim").await;
    let p = project(&pool, &jen, None).await;
    let jen_chat = chat(&pool, &jen, p).await;
    let kim_chat = chat(&pool, &kim, p).await;
    let dirs = root();
    let a = folder(dirs.path(), "jen");
    let b = folder(dirs.path(), "kim");
    a.write("app.py", "x = 1\n");
    let out = code::run_tool(
        &pool,
        &a,
        &jen,
        p,
        jen_chat,
        r#"{"message":"Start the app"}"#,
    )
    .await;
    assert!(out.ok && out.output.starts_with("Synced"), "{}", out.output);
    code::sync_chat(&pool, &b, &kim, p, kim_chat, None)
        .await
        .unwrap();
    b.write("app.py", "x = 2\n");
    code::sync_chat(&pool, &b, &kim, p, kim_chat, None)
        .await
        .unwrap();
    a.write("app.py", "x = 3\n");
    let out = code::run_tool(&pool, &a, &jen, p, jen_chat, "{}").await;
    assert!(out.output.starts_with("Paused: app.py"), "{}", out.output);
    // Asked again without combining, it says what is left.
    let out = code::run_tool(&pool, &a, &jen, p, jen_chat, "{}").await;
    assert!(
        out.output.contains("still to combine: app.py"),
        "{}",
        out.output
    );
    // Markers left in: finishing is refused.
    let out = code::run_tool(&pool, &a, &jen, p, jen_chat, r#"{"finish":true}"#).await;
    assert!(!out.ok, "{}", out.output);
    a.write("app.py", "x = 2 + 3\n");
    let out = code::run_tool(&pool, &a, &jen, p, jen_chat, r#"{"finish":true}"#).await;
    assert!(out.ok && out.output.starts_with("Synced"), "{}", out.output);
    assert_eq!(
        server_git(&pool, p, &["show", "main:app.py"]).await,
        "x = 2 + 3\n"
    );
    assert_eq!(
        code::tool_summary(&serde_json::json!({"finish":true})),
        "Finished the sync"
    );
}

mod http {
    use super::*;
    use axum::{
        body::Body,
        extract::{Path as UrlPath, State},
        http::{Request, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    use http_body_util::BodyExt;
    use obleth_chat::{
        auth::{Auth, AuthConfig},
        router, AppState,
    };
    use serde_json::json;
    use tower::ServiceExt;

    const ORIGIN: &str = "http://localhost";
    const PASSWORD: &str = "fixture-only-password-42";

    struct Session {
        cookie: String,
        csrf: String,
        user: Identity,
    }

    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        body: Value,
        s: Option<&Session>,
    ) -> (StatusCode, Vec<u8>) {
        let mut r = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("content-type", "application/json");
        if let Some(s) = s {
            r = r
                .header("cookie", &s.cookie)
                .header("x-csrf-token", &s.csrf);
        }
        let response = app
            .clone()
            .oneshot(
                r.body(Body::from(if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                }))
                .unwrap(),
            )
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
        s: &Session,
    ) -> (StatusCode, Value) {
        let (status, bytes) = call(app, method, path, body, Some(s)).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn login(app: &Router, name: &str) -> Session {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"username":name,"password":PASSWORD}).to_string(),
                    ))
                    .unwrap(),
            )
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
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        Session {
            cookie,
            csrf: body["csrfToken"].as_str().unwrap().to_owned(),
            user: serde_json::from_value(body["user"].clone()).unwrap(),
        }
    }

    #[derive(Clone)]
    struct Hub(PathBuf);
    async fn hub_user() -> Json<Value> {
        Json(json!({"login":"jen"}))
    }
    async fn hub_repo(
        UrlPath((_, name)): UrlPath<(String, String)>,
        State(hub): State<Hub>,
    ) -> (StatusCode, Json<Value>) {
        if hub.0.join("jen").join(format!("{name}.git")).exists() {
            (
                StatusCode::OK,
                Json(json!({"html_url":format!("https://github.example/jen/{name}")})),
            )
        } else {
            (StatusCode::NOT_FOUND, Json(json!({})))
        }
    }
    async fn hub_create(
        State(hub): State<Hub>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let name = body["name"].as_str().unwrap();
        let dir = hub.0.join("jen").join(format!("{name}.git"));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .current_dir(&dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        (
            StatusCode::CREATED,
            Json(
                json!({"html_url":format!("https://github.example/jen/{name}"),"private":body["private"]}),
            ),
        )
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn members_read_history_and_take_the_code_out(pool: PgPool) {
        let auth = Auth::new(
            pool.clone(),
            AuthConfig {
                origin: ORIGIN.into(),
                session_hours: 12,
                bootstrap_username: Some("test-admin".into()),
                bootstrap_password: Some(PASSWORD.into()),
                oidc: None,
                login_rate_per_minute: 1000,
                ..AuthConfig::default()
            },
        )
        .await
        .unwrap();
        let state = AppState::new(auth, pool.clone(), None, 4);
        let app = router(state, "web".into());
        let admin = login(&app, "test-admin").await;
        for name in ["jen", "kim", "out"] {
            let (status, _) = json_call(
                &app,
                "POST",
                "/api/admin/users",
                json!({"username":name,"password":PASSWORD,"displayName":name,"role":"user"}),
                &admin,
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
        }
        let jen = login(&app, "jen").await;
        let kim = login(&app, "kim").await;
        let out = login(&app, "out").await;
        let (_, g) = json_call(
            &app,
            "POST",
            "/api/admin/groups",
            json!({"name":"Lab"}),
            &admin,
        )
        .await;
        let group = g["group"]["id"].as_str().unwrap().to_owned();
        for s in [&jen, &kim] {
            json_call(
                &app,
                "PUT",
                &format!("/api/admin/groups/{group}/members/{}", s.user.id),
                Value::Null,
                &admin,
            )
            .await;
        }
        let (_, made) = json_call(
            &app,
            "POST",
            "/api/projects",
            json!({"name":"Gaudi benchmarks","scope":"group","groupId":group}),
            &jen,
        )
        .await;
        let p: Uuid = made["project"]["id"].as_str().unwrap().parse().unwrap();
        let jen_chat = chat(&pool, &jen.user, p).await;
        let kim_chat = chat(&pool, &kim.user, p).await;
        let dirs = root();
        let a = folder(dirs.path(), "jen");
        let b = folder(dirs.path(), "kim");

        // Before any code, and without workspaces on this server.
        let (status, empty) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code"),
            Value::Null,
            &kim,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(empty["main"].is_null());
        let (_, chat_view) = json_call(
            &app,
            "GET",
            &format!("/api/conversations/{jen_chat}/code"),
            Value::Null,
            &jen,
        )
        .await;
        assert_eq!(
            (
                chat_view["project"]["name"].as_str(),
                chat_view["workspace"].as_bool()
            ),
            (Some("Gaudi benchmarks"), Some(false))
        );
        let (status, _) = call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/export.zip"),
            Value::Null,
            Some(&jen),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        a.write("plot.py", "a = 1\nb = 2\n");
        code::sync_chat(&pool, &a, &jen.user, p, jen_chat, Some("Start"))
            .await
            .unwrap();
        code::sync_chat(&pool, &b, &kim.user, p, kim_chat, None)
            .await
            .unwrap();
        b.write("plot.py", "a = 1\nb = 3\n");
        code::sync_chat(&pool, &b, &kim.user, p, kim_chat, Some("Change b"))
            .await
            .unwrap();

        // Someone outside the group sees nothing.
        for path in ["", "/log", "/files", "/blame?path=plot.py", "/export.zip"] {
            let (status, _) = call(
                &app,
                "GET",
                &format!("/api/projects/{p}/code{path}"),
                Value::Null,
                Some(&out),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }

        // Jen sees both chats working on it, but only her own chat's title.
        let (_, view) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code"),
            Value::Null,
            &jen,
        )
        .await;
        assert_eq!(view["main"]["commits"], 2);
        assert_eq!(view["main"]["updatedBy"], "kim");
        let chats = view["chats"].as_array().unwrap();
        assert_eq!(chats.len(), 2);
        let mine = chats.iter().find(|c| c["chat"]["mine"] == true).unwrap();
        assert_eq!(
            (mine["chat"]["title"].as_str(), mine["behind"].as_i64()),
            (Some("Work"), Some(1))
        );
        let theirs = chats.iter().find(|c| c["chat"]["mine"] == false).unwrap();
        assert!(theirs["chat"]["title"].is_null());
        assert_eq!(theirs["chat"]["ownerName"], "kim");

        // History, newest first, each commit leading back to its chat.
        let (_, log) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/log"),
            Value::Null,
            &jen,
        )
        .await;
        let commits = log["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0]["subject"], "Change b");
        assert_eq!(commits[0]["chat"]["ownerName"], "kim");
        assert_eq!(commits[1]["chat"]["title"], "Work");
        assert_eq!(commits[0]["parents"][0], commits[1]["sha"]);
        let sha = commits[0]["sha"].as_str().unwrap();
        let (_, detail) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/commits/{sha}?path=plot.py"),
            Value::Null,
            &jen,
        )
        .await;
        assert_eq!(detail["files"][0]["path"], "plot.py");
        assert_eq!(
            (
                detail["files"][0]["added"].as_i64(),
                detail["files"][0]["removed"].as_i64()
            ),
            (Some(1), Some(1))
        );
        assert!(detail["patch"].as_str().unwrap().contains("+b = 3"));
        let (status, _) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/commits/{}", "0".repeat(40)),
            Value::Null,
            &jen,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Files, a file, and who changed each line.
        let (_, files) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/files"),
            Value::Null,
            &kim,
        )
        .await;
        assert_eq!(files["files"][0]["path"], "plot.py");
        let (_, file) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/file?path=plot.py"),
            Value::Null,
            &kim,
        )
        .await;
        assert_eq!(file["content"], "a = 1\nb = 3\n");
        let (_, blame) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/blame?path=plot.py"),
            Value::Null,
            &kim,
        )
        .await;
        let groups = blame["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(
            (groups[0]["author"].as_str(), groups[0]["lines"].clone()),
            (Some("jen"), json!(["a = 1"]))
        );
        assert_eq!(
            (groups[1]["author"].as_str(), groups[1]["start"].as_i64()),
            (Some("kim"), Some(2))
        );
        assert_eq!(groups[1]["chat"]["title"], "Work");
        let (_, lines) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/lines?path=plot.py&start=2&end=2"),
            Value::Null,
            &kim,
        )
        .await;
        let versions = lines["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions[0]["patch"].as_str().unwrap().contains("+b = 3"));
        let (status, _) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/file?path=../etc/passwd"),
            Value::Null,
            &kim,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // A zip with the code, its history and Jen's chats.
        let (status, zip) = call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/export.zip?history=true&chats=true"),
            Value::Null,
            Some(&jen),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&zip[..2], b"PK");
        let names = String::from_utf8_lossy(&zip).into_owned();
        for name in [
            "gaudi-benchmarks/plot.py",
            "gaudi-benchmarks/gaudi-benchmarks.bundle",
            "gaudi-benchmarks/HISTORY.txt",
            "gaudi-benchmarks/chats/01-work.md",
        ] {
            assert!(names.contains(name), "{name}");
        }

        // Pushed to GitHub (a stand-in here), created on the first push.
        let hub_dir = dirs.path().join("hub");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let hub = Router::new()
            .route("/user", get(hub_user))
            .route("/repos/{owner}/{name}", get(hub_repo))
            .route("/user/repos", post(hub_create))
            .with_state(Hub(hub_dir.clone()));
        tokio::spawn(async move { axum::serve(listener, hub).await.unwrap() });
        std::env::set_var("OBLETH_CHAT_GITHUB_API", &api);
        std::env::set_var(
            "OBLETH_CHAT_GITHUB_GIT",
            format!("file://{}", hub_dir.display()),
        );
        let push = json!({"name":"gaudi","private":true,"token":"ghp_testtoken1234567890"});
        let (status, sent) = json_call(
            &app,
            "POST",
            &format!("/api/projects/{p}/code/github"),
            push.clone(),
            &jen,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{sent}");
        assert_eq!(sent["target"], "jen/gaudi");
        let pushed = std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(hub_dir.join("jen/gaudi.git"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&pushed.stdout).trim(), sha);
        // Pushing again updates it; the overview remembers where it went.
        let (status, _) = json_call(
            &app,
            "POST",
            &format!("/api/projects/{p}/code/github"),
            push.clone(),
            &jen,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, view) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code"),
            Value::Null,
            &kim,
        )
        .await;
        assert_eq!(view["exports"][0]["target"], "jen/gaudi");
        assert_eq!(view["exports"][0]["behind"], 0);
        // Bad input never reaches GitHub.
        for bad in [
            json!({"name":"../x","token":"ghp_testtoken1234567890"}),
            json!({"name":"ok","token":"short"}),
        ] {
            let (status, _) = json_call(
                &app,
                "POST",
                &format!("/api/projects/{p}/code/github"),
                bad,
                &jen,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (status, _) = json_call(
            &app,
            "POST",
            &format!("/api/projects/{p}/code/github"),
            push,
            &out,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The commits on main, newest first.
    async fn shas(pool: &PgPool, project: Uuid) -> Vec<String> {
        server_git(pool, project, &["rev-list", "main"])
            .await
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn commits_are_undone_compared_and_started_from(pool: PgPool) {
        let auth = Auth::new(
            pool.clone(),
            AuthConfig {
                origin: ORIGIN.into(),
                session_hours: 12,
                bootstrap_username: Some("test-admin".into()),
                bootstrap_password: Some(PASSWORD.into()),
                oidc: None,
                login_rate_per_minute: 1000,
                ..AuthConfig::default()
            },
        )
        .await
        .unwrap();
        let state = AppState::new(auth, pool.clone(), None, 4);
        let app = router(state, "web".into());
        let admin = login(&app, "test-admin").await;
        for name in ["jen", "out"] {
            json_call(
                &app,
                "POST",
                "/api/admin/users",
                json!({"username":name,"password":PASSWORD,"displayName":name,"role":"user"}),
                &admin,
            )
            .await;
        }
        let jen = login(&app, "jen").await;
        let out = login(&app, "out").await;
        let (_, made) =
            json_call(&app, "POST", "/api/projects", json!({"name":"Site"}), &jen).await;
        let p: Uuid = made["project"]["id"].as_str().unwrap().parse().unwrap();
        let c = chat(&pool, &jen.user, p).await;
        let dirs = root();
        let a = folder(dirs.path(), "jen");
        a.write("a.txt", "1\n");
        a.write("b.txt", "x\n");
        code::sync_chat(&pool, &a, &jen.user, p, c, Some("Start"))
            .await
            .unwrap();
        a.write("b.txt", "y\n");
        code::sync_chat(&pool, &a, &jen.user, p, c, Some("Change b"))
            .await
            .unwrap();
        a.write("c.txt", "c\n");
        code::sync_chat(&pool, &a, &jen.user, p, c, Some("Add c"))
            .await
            .unwrap();
        let log = shas(&pool, p).await;
        let (first, change_b) = (log[2].clone(), log[1].clone());

        // Undo: a new commit takes back one commit's changes and keeps the rest.
        let undo = |sha: &str| format!("/api/projects/{p}/code/commits/{sha}/undo");
        let (status, body) = json_call(&app, "POST", &undo(&change_b), Value::Null, &jen).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["commits"], 4);
        assert_eq!(server_git(&pool, p, &["show", "main:b.txt"]).await, "x\n");
        assert_eq!(server_git(&pool, p, &["show", "main:c.txt"]).await, "c\n");
        assert_eq!(
            server_git(&pool, p, &["log", "-1", "--format=%s%n%an", "main"]).await,
            "Undo “Change b”\njen\n"
        );
        // Again: nothing left to undo. The first commit and outsiders are refused.
        let (status, _) = json_call(&app, "POST", &undo(&change_b), Value::Null, &jen).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = json_call(&app, "POST", &undo(&first), Value::Null, &jen).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = json_call(&app, "POST", &undo(&change_b), Value::Null, &out).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // Lines changed again later: undoing the earlier change alone would clash.
        code::sync_chat(&pool, &a, &jen.user, p, c, None)
            .await
            .unwrap();
        a.write("a.txt", "2\n");
        code::sync_chat(&pool, &a, &jen.user, p, c, Some("Two"))
            .await
            .unwrap();
        a.write("a.txt", "3\n");
        code::sync_chat(&pool, &a, &jen.user, p, c, Some("Three"))
            .await
            .unwrap();
        let two = shas(&pool, p).await[1].clone();
        let (status, _) = json_call(&app, "POST", &undo(&two), Value::Null, &jen).await;
        assert_eq!(status, StatusCode::CONFLICT);

        // Compare the first commit with now.
        let (status, cmp) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/compare?from={}", &first[..10]),
            Value::Null,
            &jen,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{cmp}");
        assert_eq!(cmp["from"]["subject"], "Start");
        assert_eq!(cmp["to"]["subject"], "Three");
        assert_eq!(cmp["total"], 5);
        let paths: Vec<&str> = cmp["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["a.txt", "c.txt"]);
        let (_, one) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/compare?from={first}&to={change_b}&path=b.txt"),
            Value::Null,
            &jen,
        )
        .await;
        assert_eq!(one["total"], 1);
        assert!(one["patch"].as_str().unwrap().contains("+y"), "{one}");
        let (status, _) = json_call(
            &app,
            "GET",
            &format!("/api/projects/{p}/code/compare?from={first}"),
            Value::Null,
            &out,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // A new chat from the first commit: its workspace starts there and its next
        // Sync brings in everything since.
        let fresh = chat(&pool, &jen.user, p).await;
        let (status, started) = json_call(
            &app,
            "PUT",
            &format!("/api/conversations/{fresh}/code/start"),
            json!({"sha":&first[..8]}),
            &jen,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{started}");
        assert_eq!(started["start"], first.as_str());
        let f = folder(dirs.path(), "fresh");
        assert!(code::begin(&pool, &f, &jen.user, p, fresh).await.unwrap());
        assert_eq!(f.read("b.txt").as_deref(), Some("x\n"));
        assert!(f.read("c.txt").is_none());
        let who = code::author(&jen.user);
        let view = code::report(&pool, &f, &who, p).await.unwrap();
        assert_eq!(
            (view["behind"].as_i64(), view["ahead"].as_i64()),
            (Some(5), Some(0))
        );
        // Only an empty folder is filled.
        assert!(!code::begin(&pool, &f, &jen.user, p, fresh).await.unwrap());
        f.write("d.txt", "d\n");
        code::sync_chat(&pool, &f, &jen.user, p, fresh, Some("Add d"))
            .await
            .unwrap();
        assert_eq!(f.read("c.txt").as_deref(), Some("c\n"));
        assert_eq!(f.read("a.txt").as_deref(), Some("3\n"));
        assert_eq!(server_git(&pool, p, &["show", "main:d.txt"]).await, "d\n");

        // Undo a save that was not synced: its changes are unsaved again.
        f.write("e.txt", "e\n");
        assert!(workspace::commit(&f, &who, "Add e").await.unwrap());
        workspace::uncommit(&f, &who).await.unwrap();
        // Repositories checked out to read are not the project's changes.
        f.write("repos/lib/README.md", "lib\n");
        let status = workspace::status(&f, &who).await.unwrap();
        assert_eq!(status.ahead, 0);
        assert_eq!(status.changes.len(), 1);
        assert_eq!(
            (
                status.changes[0].path.as_str(),
                status.changes[0].status.as_str()
            ),
            ("e.txt", "added")
        );
        // A synced one stays: it is in the project's history.
        f.write("e.txt", "");
        std::fs::remove_file(dirs.path().join("fresh/e.txt")).unwrap();
        assert!(workspace::uncommit(&f, &who).await.is_err());
    }
}

fn git_in(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_project_starts_from_a_repository(pool: PgPool) {
    let jen = person(&pool, "Jen").await;
    let p = project(&pool, &jen, None).await;
    let dirs = root();
    // Someone else's repository, with three commits on its default branch.
    let origin = dirs.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    git_in(&origin, &["init", "-q", "-b", "trunk"]);
    for (n, text) in ["one", "two", "three"].iter().enumerate() {
        std::fs::write(origin.join("notes.txt"), format!("{text}\n")).unwrap();
        git_in(&origin, &["add", "."]);
        git_in(
            &origin,
            &[
                "-c",
                "user.name=Ada",
                "-c",
                "user.email=ada@example.org",
                "commit",
                "-qm",
                &format!("Step {}", n + 1),
            ],
        );
    }
    let source = |fetch: String| code::import::Source {
        url: "https://example.org/ada/notes".into(),
        fetch,
        config: vec![],
        secret: None,
    };
    let missing = code::import::start(
        &pool,
        p,
        jen.id,
        &code::author(&jen),
        &source(format!("file://{}/gone", dirs.path().display())),
    )
    .await
    .unwrap_err();
    assert!(
        missing.1.contains("could not read that repository"),
        "{}",
        missing.1
    );

    let done = code::import::start(
        &pool,
        p,
        jen.id,
        &code::author(&jen),
        &source(format!("file://{}", origin.display())),
    )
    .await
    .unwrap();
    assert!(done.history);
    assert_eq!(done.commits, 3);
    // Its history is the project's, authors and all.
    assert_eq!(
        server_git(&pool, p, &["log", "--format=%an %s", "main"]).await,
        "Ada Step 3\nAda Step 2\nAda Step 1\n"
    );
    let origin_row: (Option<String>, Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT origin_url, origin_sha, origin_history FROM project_code WHERE project_id = $1",
    )
    .bind(p)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        origin_row.0.as_deref(),
        Some("https://example.org/ada/notes")
    );
    assert_eq!(origin_row.1.as_deref(), Some(done.main.as_str()));
    assert_eq!(origin_row.2, Some(true));
    // Only once: a project with code keeps it.
    let again = code::import::start(
        &pool,
        p,
        jen.id,
        &code::author(&jen),
        &source(format!("file://{}", origin.display())),
    )
    .await
    .unwrap_err();
    assert_eq!(again.0, axum::http::StatusCode::CONFLICT);

    // A chat in the project starts with that code and syncs on top of it.
    let c = chat(&pool, &jen, p).await;
    let f = folder(dirs.path(), "jen");
    assert!(code::begin(&pool, &f, &jen, p, c).await.unwrap());
    assert_eq!(f.read("notes.txt").as_deref(), Some("three\n"));
    f.write("notes.txt", "four\n");
    code::sync_chat(&pool, &f, &jen, p, c, Some("Step 4"))
        .await
        .unwrap();
    assert_eq!(
        server_git(&pool, p, &["log", "-1", "--format=%an %s", "main"]).await,
        "Jen Step 4\n"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_repository_too_big_to_keep_comes_in_as_its_latest_version(pool: PgPool) {
    let jen = person(&pool, "Jen").await;
    let p = project(&pool, &jen, None).await;
    let dirs = root();
    let origin = dirs.path().join("origin");
    std::fs::create_dir_all(origin.join("src")).unwrap();
    git_in(&origin, &["init", "-q"]);
    for n in 1..=3 {
        std::fs::write(origin.join("src/lib.rs"), format!("// v{n}\n")).unwrap();
        git_in(&origin, &["add", "."]);
        git_in(
            &origin,
            &[
                "-c",
                "user.name=Ada",
                "-c",
                "user.email=a@e",
                "commit",
                "-qm",
                &format!("v{n}"),
            ],
        );
    }
    let source = code::import::Source {
        url: "https://example.org/ada/big".into(),
        fetch: format!("file://{}", origin.display()),
        config: vec![],
        secret: None,
    };
    let done = code::import::start_within(&pool, p, jen.id, &code::author(&jen), &source, 1)
        .await
        .unwrap();
    assert!(!done.history);
    assert_eq!(done.commits, 1);
    assert_eq!(
        server_git(&pool, p, &["show", "main:src/lib.rs"]).await,
        "// v3\n"
    );
    let log = server_git(&pool, p, &["log", "--format=%an|%s", "main"]).await;
    assert_eq!(log, "Jen|Start from https://example.org/ada/big\n");
}
