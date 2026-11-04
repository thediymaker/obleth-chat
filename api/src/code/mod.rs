//! Code in projects: each project has one line of history, kept by Chat, that its
//! chats' workspaces sync with. Sync saves a workspace's changes, brings in what others
//! synced, and shares the result; when both sides changed the same lines it pauses so
//! the person (or the model) can combine them. A project can start from a repository,
//! and the code can leave: as a zip, or pushed to GitHub.

pub mod export;
pub mod history;
pub mod import;
pub mod repo;
pub mod workspace;

use crate::{auth::Identity, error::ApiError, sandbox::Sandbox, sandbox::Sandboxes, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use repo::Repo;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use workspace::{Author, Merge, Pod, Shell};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/conversations/{id}/code", get(status))
        .route("/api/conversations/{id}/code/sync", post(sync))
        .route("/api/conversations/{id}/code/save", post(save))
        .route("/api/conversations/{id}/code/finish", post(finish))
        .route("/api/conversations/{id}/code/stop", post(stop))
        .route("/api/conversations/{id}/code/choose", post(choose))
        .route("/api/conversations/{id}/code/undo", post(undo))
        .route("/api/conversations/{id}/code/uncommit", post(uncommit))
        .route("/api/conversations/{id}/code/start", put(start_at))
        .route("/api/conversations/{id}/code/diff", get(diff))
        .route(
            "/api/projects/{id}/code/commits/{sha}/undo",
            post(undo_commit),
        )
        .merge(history::router())
        .merge(export::router())
        .merge(import::router())
        .with_state(state)
}

fn unavailable() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Workspaces are not enabled on this server".into(),
    )
}

pub fn author(user: &Identity) -> Author {
    Author {
        name: user.display_name.clone(),
        email: format!("{}@users.chat.invalid", user.username),
    }
}

/// The chat's project, when the chat is in one its owner may still use.
async fn chat_project(
    pool: &PgPool,
    user: Uuid,
    chat: Uuid,
) -> Result<Option<(Uuid, String, bool)>, ApiError> {
    let exists: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT project_id FROM conversations WHERE id = $1 AND owner_id = $2")
            .bind(chat)
            .bind(user)
            .fetch_optional(pool)
            .await?;
    let Some(project) = exists.ok_or_else(ApiError::not_found)? else {
        return Ok(None);
    };
    let row: Option<(Uuid, String, bool)> = sqlx::query_as(&format!(
        "SELECT p.id, p.name, p.owner_id = $1 FROM projects p WHERE p.id = $2 AND {}",
        crate::projects::VISIBLE
    ))
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

struct Chat {
    sandboxes: Sandboxes,
    user: Identity,
    sandbox: Sandbox,
    project: Uuid,
}

impl Chat {
    fn pod(&self) -> Pod<'_> {
        Pod {
            sandboxes: &self.sandboxes,
            sandbox: &self.sandbox,
        }
    }
}

/// Everything a write needs: the chat's running workspace and its project.
async fn chat(s: &AppState, headers: &HeaderMap, id: Uuid) -> Result<Chat, ApiError> {
    let user = s.auth.require(headers, true).await?;
    let sandboxes = s.sandboxes.clone().ok_or_else(unavailable)?;
    if !sandboxes.allowed(&user) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Workspaces are available to administrators".into(),
        ));
    }
    let (project, ..) = chat_project(&s.store.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::bad("Put this chat in a project to share its code"))?;
    let sandbox = sandboxes
        .get(user.id, id)
        .await?
        .ok_or_else(|| ApiError::bad("This chat has no workspace yet"))?;
    let sandbox = if sandbox.status == "running" {
        sandbox
    } else {
        sandboxes
            .ensure_running(&sandbox)
            .await
            .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?
    };
    Ok(Chat {
        sandboxes,
        user,
        sandbox,
        project,
    })
}

/// A commit message's links back to the chat and its latest reply.
async fn trailers(pool: &PgPool, chat: Uuid) -> String {
    let turn: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM messages WHERE conversation_id = $1 AND role = 'assistant' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(chat)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match turn {
        Some(turn) => format!("\n\nObleth-Chat: {chat}\nObleth-Turn: {turn}"),
        None => format!("\n\nObleth-Chat: {chat}"),
    }
}

/// A message for these changes when the person gives none.
fn draft(changes: &[workspace::Change]) -> String {
    let verb = |c: &workspace::Change| match c.status.as_str() {
        "added" => "Add",
        "deleted" => "Remove",
        _ => "Update",
    };
    match changes {
        [] => "Save work".into(),
        [one] => format!("{} {}", verb(one), one.path),
        [first, rest @ ..] => {
            let same = rest.iter().all(|c| verb(c) == verb(first));
            format!(
                "{} {} and {} more file{}",
                if same { verb(first) } else { "Update" },
                first.path,
                rest.len(),
                if rest.len() == 1 { "" } else { "s" }
            )
        }
    }
}

fn clean_message(message: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(m) = message.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    if m.chars().count() > 2000 || m.contains('\0') {
        return Err(ApiError::bad("Keep the message under 2,000 characters"));
    }
    Ok(Some(m.to_owned()))
}

/// How many of main's commits the workspace does not have.
async fn behind(
    pool: &PgPool,
    project: Uuid,
    base: Option<&str>,
) -> Result<(Option<(String, i64)>, i64), ApiError> {
    let mut db = pool.acquire().await?;
    let main = repo::head(&mut db, project).await?;
    let Some((sha, commits)) = &main else {
        return Ok((None, 0));
    };
    let at = match base {
        Some(base) if base == sha => return Ok((main.clone(), 0)),
        Some(base) => repo::commits_at(&mut db, project, base).await?.unwrap_or(0),
        None => 0,
    };
    Ok((main.clone(), (commits - at).max(0)))
}

/// The Git tab's view of a chat: its project, its workspace's changes, and how far
/// it is from the project's latest code.
pub async fn report(
    pool: &PgPool,
    sh: &impl Shell,
    who: &Author,
    project: Uuid,
) -> Result<Value, ApiError> {
    let ws = workspace::status(sh, who).await?;
    let (main, behind_by) = behind(pool, project, ws.base.as_deref()).await?;
    let conflicts: Vec<&str> = ws
        .changes
        .iter()
        .filter(|c| c.status == "conflict")
        .map(|c| c.path.as_str())
        .collect();
    Ok(json!({
        "running":true,
        "main":main.map(|(sha,commits)| json!({"sha":sha,"commits":commits})),
        "repo":ws.repo,"head":ws.head,"base":ws.base,"merging":ws.merging,"ahead":ws.ahead,"behind":behind_by,
        "changes":ws.changes,"conflicts":conflicts,"log":ws.log,"draft":draft(&ws.changes),
    }))
}

async fn status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    let project = chat_project(pool, user.id, id).await?;
    let Some((project_id, name, mine)) = project else {
        return Ok(Json(json!({"project":null})));
    };
    let project_json = json!({"id":project_id,"name":name,"mine":mine});
    let sandbox = match &s.sandboxes {
        Some(sb) if sb.allowed(&user) => sb.get(user.id, id).await?,
        _ => None,
    };
    let running = sandbox.as_ref().filter(|w| w.status == "running");
    let (Some(sb), Some(sandbox)) = (s.sandboxes.as_ref(), running) else {
        let (main, _) = behind(pool, project_id, None).await?;
        return Ok(Json(json!({
            "project":project_json,"workspace":sandbox.is_some(),"running":false,
            "main":main.map(|(sha,commits)| json!({"sha":sha,"commits":commits})),
        })));
    };
    let pod = Pod {
        sandboxes: sb,
        sandbox,
    };
    let mut out = report(pool, &pod, &author(&user), project_id).await?;
    out["project"] = project_json;
    out["workspace"] = json!(true);
    Ok(Json(out))
}

#[derive(Deserialize, Default)]
struct SyncInput {
    message: Option<String>,
}

/// What a sync ended with, for the Git tab.
fn outcome(state: &str, extra: Value) -> Value {
    let mut out = json!({"state":state});
    if let (Some(o), Some(e)) = (out.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    out
}

async fn record(pool: &PgPool, chat: Uuid, project: Uuid, synced: &str) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO conversation_code (conversation_id, project_id, synced, synced_at, updated_at) VALUES ($1,$2,$3,NOW(),NOW())
         ON CONFLICT (conversation_id) DO UPDATE SET project_id = $2, synced = $3, synced_at = NOW(), updated_at = NOW()",
    )
    .bind(chat)
    .bind(project)
    .bind(synced)
    .execute(pool)
    .await?;
    Ok(())
}

/// Brings main in and shares the workspace's commits, retrying when someone else
/// synced in between. Stops at a conflict.
async fn exchange(
    pool: &PgPool,
    sh: &impl Shell,
    user: &Identity,
    project: Uuid,
    chat: Uuid,
) -> Result<Value, ApiError> {
    let who = author(user);
    let combine = format!(
        "Bring in the project's latest changes{}",
        trailers(pool, chat).await
    );
    for _ in 0..3 {
        let ws = workspace::status(sh, &who).await?;
        if ws.merging {
            return Ok(outcome("conflict", json!({})));
        }
        let mut db = pool.acquire().await?;
        let main = repo::head(&mut db, project).await?;
        // A workspace that last synced with another project (or none) starts over.
        let known = match ws.base.as_deref() {
            Some(base) => repo::commits_at(&mut db, project, base).await?.is_some(),
            None => true,
        };
        let base = if known { ws.base.clone() } else { None };
        if let Some((sha, _)) = &main {
            // Download only what the workspace lacks; combine whenever its work does
            // not yet include main (also after a stopped sync, which kept main).
            if base.as_deref() != Some(sha.as_str()) {
                let server = repo::Repo::load(&mut db, project).await?;
                let exclude = base.as_ref().map(|b| format!("^{b}"));
                let mut revs = vec!["main"];
                if let Some(exclude) = &exclude {
                    revs.push(exclude);
                }
                let bundle = server.bundle(&revs).await?;
                drop(db);
                if !known {
                    workspace::forget(sh, &who).await?;
                }
                workspace::receive(sh, &who, &bundle).await?;
            } else {
                drop(db);
            }
            if let Merge::Conflicts(files) = workspace::merge(sh, &who, &combine).await? {
                return Ok(outcome("conflict", json!({"conflicts":files})));
            }
        } else if ws.base.is_some() {
            drop(db);
            workspace::forget(sh, &who).await?;
        }
        let Some(bundle) = workspace::send(sh).await? else {
            if let Some((sha, _)) = &main {
                record(pool, chat, project, sha).await?;
            }
            return Ok(outcome(
                "synced",
                json!({"main":main.map(|(sha,_)| sha),"shared":false}),
            ));
        };
        let expected = main.as_ref().map(|(sha, _)| sha.as_str());
        match repo::advance(pool, project, user.id, expected, &bundle).await? {
            repo::Advance::Moved { main, commits } => {
                workspace::mark(sh, &who, &main).await?;
                record(pool, chat, project, &main).await?;
                return Ok(outcome(
                    "synced",
                    json!({"main":main,"commits":commits,"shared":true}),
                ));
            }
            repo::Advance::Stale { .. } => continue,
        }
    }
    Err(repo::conflict(
        "Others kept syncing at the same moment. Try again",
    ))
}

/// Sync: saves the workspace's changes, brings in the project's latest code, and
/// shares the result. Ends "synced", or "conflict" with the files to settle.
pub async fn sync_chat(
    pool: &PgPool,
    sh: &impl Shell,
    user: &Identity,
    project: Uuid,
    chat: Uuid,
    message: Option<&str>,
) -> Result<Value, ApiError> {
    let who = author(user);
    let ws = workspace::status(sh, &who).await?;
    if ws.merging {
        return Err(repo::conflict(
            "A sync is paused on a conflict. Finish it or stop it first",
        ));
    }
    let message = clean_message(message)?.unwrap_or_else(|| draft(&ws.changes));
    let message = format!("{message}{}", trailers(pool, chat).await);
    workspace::commit(sh, &who, &message).await?;
    exchange(pool, sh, user, project, chat).await
}

/// Finishes a paused sync: the combined files are committed, then shared.
pub async fn finish_chat(
    pool: &PgPool,
    sh: &impl Shell,
    user: &Identity,
    project: Uuid,
    chat: Uuid,
) -> Result<Value, ApiError> {
    workspace::conclude(sh, &author(user)).await?;
    exchange(pool, sh, user, project, chat).await
}

async fn sync(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<SyncInput>>,
) -> Result<Json<Value>, ApiError> {
    let c = chat(&s, &headers, id).await?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let pod = c.pod();
    let out = sync_chat(
        &s.store.pool,
        &pod,
        &c.user,
        c.project,
        id,
        body.message.as_deref(),
    )
    .await?;
    Ok(Json(out))
}

/// Save without syncing: the changes become a commit in this chat's workspace only.
async fn save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<SyncInput>>,
) -> Result<Json<Value>, ApiError> {
    let c = chat(&s, &headers, id).await?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let pod = c.pod();
    let who = author(&c.user);
    let ws = workspace::status(&pod, &who).await?;
    if ws.merging {
        return Err(repo::conflict(
            "A sync is paused on a conflict. Finish it or stop it first",
        ));
    }
    let message = clean_message(body.message.as_deref())?.unwrap_or_else(|| draft(&ws.changes));
    let message = format!("{message}{}", trailers(&s.store.pool, id).await);
    let saved = workspace::commit(&pod, &who, &message).await?;
    Ok(Json(json!({"saved":saved})))
}

async fn finish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let c = chat(&s, &headers, id).await?;
    let pod = c.pod();
    Ok(Json(
        finish_chat(&s.store.pool, &pod, &c.user, c.project, id).await?,
    ))
}

async fn stop(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let c = chat(&s, &headers, id).await?;
    workspace::abort(&c.pod(), &author(&c.user)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Choice {
    path: String,
    side: String,
}

pub(crate) fn check_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.len() > 1000
        || path.starts_with('/')
        || path.starts_with('-')
        || path.split('/').any(|part| part == "..")
        || path.contains('\0')
    {
        return Err(ApiError::bad("Choose a file in the project"));
    }
    Ok(())
}

async fn choose(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Choice>,
) -> Result<StatusCode, ApiError> {
    check_path(&body.path)?;
    let c = chat(&s, &headers, id).await?;
    workspace::choose(&c.pod(), &author(&c.user), &body.path, &body.side).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UndoInput {
    path: Option<String>,
}

async fn undo(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UndoInput>,
) -> Result<StatusCode, ApiError> {
    if let Some(path) = &body.path {
        check_path(path)?;
    }
    let c = chat(&s, &headers, id).await?;
    workspace::undo(&c.pod(), &author(&c.user), body.path.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes back the last save, when it has not been synced.
async fn uncommit(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let c = chat(&s, &headers, id).await?;
    workspace::uncommit(&c.pod(), &author(&c.user)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct StartInput {
    sha: String,
}

/// Where a new chat's workspace begins: an earlier commit of the project's code.
async fn start_at(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<StartInput>,
) -> Result<Json<Value>, ApiError> {
    history::check_sha(&body.sha)?;
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    let (project, ..) = chat_project(pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::bad("Put this chat in a project first"))?;
    let mut db = pool.acquire().await?;
    let server = Repo::load(&mut db, project).await?;
    drop(db);
    let full = server
        .resolve(&body.sha)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !server.on_main(&full).await? {
        return Err(ApiError::not_found());
    }
    sqlx::query(
        "INSERT INTO conversation_code (conversation_id, project_id, start) VALUES ($1,$2,$3)
         ON CONFLICT (conversation_id) DO UPDATE SET project_id = $2, start = $3, updated_at = NOW()",
    )
    .bind(id)
    .bind(project)
    .bind(&full)
    .execute(pool)
    .await?;
    Ok(Json(json!({"start":full})))
}

/// Puts the project's code in a project chat's new workspace: at the commit the chat
/// was started from, otherwise at main. Returns the running workspace when it did.
pub async fn seed(
    pool: &PgPool,
    sandboxes: &Sandboxes,
    user: &Identity,
    chat: Uuid,
    sandbox: &Sandbox,
) -> Result<Option<Sandbox>, ApiError> {
    let Some((project, ..)) = chat_project(pool, user.id, chat).await? else {
        return Ok(None);
    };
    if repo::head(&mut *pool.acquire().await?, project)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let running = sandboxes
        .ensure_running(sandbox)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    let pod = Pod {
        sandboxes,
        sandbox: &running,
    };
    Ok(begin(pool, &pod, user, project, chat)
        .await?
        .then_some(running))
}

/// Fills an empty chat folder with the project's code, as `seed` does once the
/// workspace is running. Returns whether it did.
pub async fn begin(
    pool: &PgPool,
    sh: &impl Shell,
    user: &Identity,
    project: Uuid,
    chat: Uuid,
) -> Result<bool, ApiError> {
    let mut db = pool.acquire().await?;
    let start: Option<String> = sqlx::query_scalar(
        "SELECT start FROM conversation_code WHERE conversation_id = $1 AND project_id = $2",
    )
    .bind(chat)
    .bind(project)
    .fetch_optional(&mut *db)
    .await?
    .flatten();
    let server = Repo::load(&mut db, project).await?;
    drop(db);
    let Some(main) = server.main.clone() else {
        return Ok(false);
    };
    let rev = match start {
        Some(start) if server.on_main(&start).await? => start,
        _ => main.clone(),
    };
    let bundle = server.bundle(&["main"]).await?;
    if !workspace::start(sh, &author(user), &bundle, &rev).await? {
        return Ok(false);
    }
    // How far behind it is, for a commit that was never main itself.
    if rev != main {
        let commits = server.count(&rev).await?;
        sqlx::query("INSERT INTO project_code_tips (project_id, tip, commits) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(project)
            .bind(&rev)
            .bind(commits as i32)
            .execute(pool)
            .await?;
    }
    record(pool, chat, project, &rev).await?;
    sqlx::query("UPDATE conversation_code SET start = NULL WHERE conversation_id = $1")
        .bind(chat)
        .execute(pool)
        .await?;
    Ok(true)
}

/// Undo a commit in the project's history: a new commit on main takes back its
/// changes, and each chat brings that in with its next Sync.
async fn undo_commit(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, sha)): Path<(Uuid, String)>,
) -> Result<Json<Value>, ApiError> {
    history::check_sha(&sha)?;
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    history::visible(pool, user.id, id).await?;
    for _ in 0..3 {
        let mut db = pool.acquire().await?;
        let mut server = Repo::load(&mut db, id).await?;
        drop(db);
        let full = server
            .resolve(&sha)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if !server.on_main(&full).await? {
            return Err(ApiError::not_found());
        }
        let expected = server.main.clone();
        let bundle = server.revert(&full, &author(&user)).await?;
        match repo::advance(pool, id, user.id, expected.as_deref(), &bundle).await? {
            repo::Advance::Moved { main, commits } => {
                return Ok(Json(json!({"sha":main,"commits":commits})))
            }
            repo::Advance::Stale { .. } => continue,
        }
    }
    Err(repo::conflict(
        "Others kept syncing at the same moment. Try again",
    ))
}

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

async fn diff(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<Value>, ApiError> {
    check_path(&q.path)?;
    let user = s.auth.require(&headers, false).await?;
    let sb = s.sandboxes.as_ref().ok_or_else(unavailable)?;
    let sandbox = sb
        .get(user.id, id)
        .await?
        .filter(|w| w.status == "running")
        .ok_or_else(|| repo::conflict("The workspace is not running"))?;
    let pod = Pod {
        sandboxes: sb,
        sandbox: &sandbox,
    };
    let mut text = workspace::diff(&pod, &author(&user), &q.path).await?;
    let truncated = text.len() > 200_000;
    if truncated {
        let mut end = 200_000;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Ok(Json(json!({"diff":text,"truncated":truncated})))
}

/// The model's tool for the Git tab's Sync, offered in project chats with a workspace.
pub fn tool_definition() -> Value {
    json!({"type":"function","function":{"name":"sync_code","description":"Sync this chat's workspace with the project's shared code, as the Git tab's Sync button does: save the changes as a commit, bring in what others synced, and share the result. Use it only when the person asks to sync, share, save or commit their work. When files changed in both places it pauses and lists them: combine the two versions in each file (keep both changes working and remove the <<<<<<<, ======= and >>>>>>> lines), then call it again with finish set. Set stop to back out of a paused sync, which puts every file back.","parameters":{"type":"object","properties":{"message":{"type":"string","description":"One line saying what changed, in the person's terms. Defaults to a summary of the changed files."},"finish":{"type":"boolean","description":"Finish a paused sync once its files are combined."},"stop":{"type":"boolean","description":"Back out of a paused sync."}},"required":[],"additionalProperties":false}}})
}

pub fn tool_summary(args: &Value) -> String {
    if args["stop"].as_bool() == Some(true) {
        "Stopped the sync".into()
    } else if args["finish"].as_bool() == Some(true) {
        "Finished the sync".into()
    } else {
        "Synced with the project".into()
    }
}

/// Runs sync_code; the result reads like the Git tab.
pub async fn run_tool(
    pool: &PgPool,
    sh: &impl Shell,
    user: &Identity,
    project: Uuid,
    chat: Uuid,
    raw: &str,
) -> crate::sandbox::tools::Outcome {
    let args: Value = serde_json::from_str(raw.trim()).unwrap_or(json!({}));
    let summary = tool_summary(&args);
    let who = author(user);
    let result: Result<String, ApiError> = async {
        if args["stop"].as_bool() == Some(true) {
            workspace::abort(sh, &who).await?;
            return Ok("Stopped. Every file is back as it was before the sync, with this chat's changes saved; nothing was shared.".into());
        }
        let paused = workspace::status(sh, &who).await?.merging;
        let out = if paused {
            if args["finish"].as_bool() != Some(true) {
                let ws = workspace::status(sh, &who).await?;
                let files: Vec<&str> = ws.changes.iter().filter(|c| c.status == "conflict").map(|c| c.path.as_str()).collect();
                return Ok(format!("A sync is paused. Files still to combine: {}. Combine them, then call sync_code with finish.", files.join(", ")));
            }
            finish_chat(pool, sh, user, project, chat).await?
        } else {
            sync_chat(pool, sh, user, project, chat, args["message"].as_str()).await?
        };
        Ok(match out["state"].as_str() {
            Some("conflict") => {
                let files: Vec<&str> = out["conflicts"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                format!("Paused: {} changed both here and in the project. Read each, combine the two versions between the conflict markers so both changes still work, remove the markers, then call sync_code with finish. Tell the person what you kept from each side.", if files.is_empty() { "some files".to_string() } else { files.join(", ") })
            }
            _ if out["shared"].as_bool() == Some(true) => format!("Synced. Everyone in the project now has these changes ({} commits on main).", out["commits"]),
            _ => "Already up to date with the project; there was nothing new to share.".into(),
        })
    }
    .await;
    let (ok, output) = match result {
        Ok(text) => (true, text),
        Err(e) => (false, format!("Error: {}", e.1)),
    };
    crate::sandbox::tools::Outcome {
        ok,
        detail: output.clone(),
        summary,
        output,
        preview: None,
        extra: json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, status: &str) -> workspace::Change {
        workspace::Change {
            path: path.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    #[test]
    fn drafts_a_message_from_the_changes() {
        assert_eq!(draft(&[]), "Save work");
        assert_eq!(draft(&[change("a.py", "modified")]), "Update a.py");
        assert_eq!(
            draft(&[change("a.py", "added"), change("b.py", "added")]),
            "Add a.py and 1 more file"
        );
        assert_eq!(
            draft(&[
                change("a.py", "added"),
                change("b.py", "deleted"),
                change("c", "modified")
            ]),
            "Update a.py and 2 more files"
        );
    }

    #[test]
    fn paths_stay_in_the_project() {
        for bad in ["", "/etc/passwd", "../x", "a/../../b", "-rf"] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
        assert!(check_path("src/main.rs").is_ok());
    }
}
