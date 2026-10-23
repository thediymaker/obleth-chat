//! A project's code as its members read it: who is working on it, the history, each
//! commit's files, and every line's last change. Commits made in a chat carry trailers
//! naming the chat and turn, so history leads back to the conversation that wrote it.

use super::repo::{self, Repo};
use crate::{error::ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{id}/code", get(overview))
        .route("/api/projects/{id}/code/log", get(log))
        .route("/api/projects/{id}/code/commits/{sha}", get(commit))
        .route("/api/projects/{id}/code/files", get(files))
        .route("/api/projects/{id}/code/file", get(file))
        .route("/api/projects/{id}/code/blame", get(blame))
        .route("/api/projects/{id}/code/lines", get(lines))
}

const SEP: char = '\x1f';
const END: char = '\x1e';
const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%at%x1f%s%x1f%(trailers:key=Obleth-Chat,valueonly,separator=)%x1f%(trailers:key=Obleth-Turn,valueonly,separator=)%x1e";

/// The project's name, when the person may see it.
pub(crate) async fn visible(pool: &PgPool, user: Uuid, project: Uuid) -> Result<String, ApiError> {
    sqlx::query_scalar(&format!(
        "SELECT p.name FROM projects p WHERE p.id = $2 AND {}",
        crate::projects::VISIBLE
    ))
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)
}

async fn open(s: &AppState, headers: &HeaderMap, project: Uuid) -> Result<(Uuid, Repo), ApiError> {
    let user = s.auth.require(headers, false).await?;
    visible(&s.store.pool, user.id, project).await?;
    let mut db = s.store.pool.acquire().await?;
    let repo = Repo::load(&mut db, project).await?;
    Ok((user.id, repo))
}

#[derive(Debug, Clone)]
struct Entry {
    sha: String,
    parents: Vec<String>,
    author: String,
    time: i64,
    subject: String,
    chat: Option<Uuid>,
    turn: Option<Uuid>,
}

fn parse_log(out: &str) -> Vec<Entry> {
    out.split(END)
        .filter_map(|record| {
            let f: Vec<&str> = record.trim_start_matches('\n').split(SEP).collect();
            if f.len() < 5 || f[0].len() != 40 {
                return None;
            }
            Some(Entry {
                sha: f[0].to_owned(),
                parents: f[1].split_whitespace().map(str::to_owned).collect(),
                author: f[2].to_owned(),
                time: f[3].parse().unwrap_or(0),
                subject: f[4].to_owned(),
                chat: f.get(5).and_then(|c| c.trim().parse().ok()),
                turn: f.get(6).and_then(|c| c.trim().parse().ok()),
            })
        })
        .collect()
}

/// The chats commits came from, as this person may see them: a title for their own
/// and for ones shared with them, only the owner's name for the rest.
async fn chats(pool: &PgPool, user: Uuid, ids: &[Uuid]) -> Result<HashMap<Uuid, Value>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(&format!(
        "SELECT c.id, c.title, c.owner_id = $1 AS mine, u.display_name, (c.owner_id = $1 OR ({})) AS readable
         FROM conversations c JOIN users u ON u.id = c.owner_id WHERE c.id = ANY($2)",
        crate::shares::SEES
    ))
    .bind(user)
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let readable: bool = r.get("readable");
            (
                r.get::<Uuid, _>("id"),
                json!({"id":r.get::<Uuid,_>("id"),"title":readable.then(|| r.get::<String,_>("title")),
                    "ownerName":r.get::<String,_>("display_name"),"mine":r.get::<bool,_>("mine"),"readable":readable}),
            )
        })
        .collect())
}

fn entry_json(e: &Entry, chats: &HashMap<Uuid, Value>) -> Value {
    json!({"sha":e.sha,"parents":e.parents,"author":e.author,"time":e.time,"subject":e.subject,
        "chat":e.chat.and_then(|c| chats.get(&c).cloned()),"turn":e.turn})
}

async fn overview(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    visible(pool, user.id, id).await?;
    let head = sqlx::query(
        "SELECT pc.main, pc.commits, pc.bytes, pc.updated_at, u.display_name FROM project_code pc LEFT JOIN users u ON u.id = pc.updated_by WHERE pc.project_id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let main = head.as_ref().and_then(|r| {
        r.get::<Option<String>, _>("main").map(|sha| {
            json!({"sha":sha,"commits":r.get::<i32,_>("commits"),"bytes":r.get::<i64,_>("bytes"),
                "updatedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at"),"updatedBy":r.get::<Option<String>,_>("display_name")})
        })
    });
    let commits = head
        .as_ref()
        .map(|r| r.get::<i32, _>("commits"))
        .unwrap_or(0);
    let rows = sqlx::query(
        "SELECT cc.conversation_id, cc.synced_at, COALESCE(t.commits, 0) AS at
         FROM conversation_code cc JOIN conversations c ON c.id = cc.conversation_id AND c.project_id = cc.project_id
         LEFT JOIN project_code_tips t ON t.project_id = cc.project_id AND t.tip = cc.synced
         WHERE cc.project_id = $1 ORDER BY cc.synced_at DESC NULLS LAST LIMIT 50",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.get("conversation_id")).collect();
    let known = chats(pool, user.id, &ids).await?;
    let working: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            let chat = known.get(&r.get::<Uuid, _>("conversation_id"))?.clone();
            Some(json!({"chat":chat,"syncedAt":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("synced_at"),
                "behind":(commits - r.get::<i32,_>("at")).max(0)}))
        })
        .collect();
    let exports: Vec<Value> = sqlx::query(
        "SELECT DISTINCT ON (e.target) e.kind, e.target, e.url, e.tip, e.created_at, COALESCE(t.commits, 0) AS at
         FROM project_code_exports e LEFT JOIN project_code_tips t ON t.project_id = e.project_id AND t.tip = e.tip
         WHERE e.project_id = $1 ORDER BY e.target, e.created_at DESC",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| json!({"kind":r.get::<String,_>("kind"),"target":r.get::<String,_>("target"),"url":r.get::<String,_>("url"),
        "at":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),"behind":(commits - r.get::<i32,_>("at")).max(0)}))
    .collect();
    Ok(Json(
        json!({"main":main,"chats":working,"exports":exports,"limits":{"bytes":repo::MAX_PROJECT_BYTES}}),
    ))
}

#[derive(Deserialize)]
struct LogQuery {
    limit: Option<usize>,
    path: Option<String>,
}

async fn log(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Value>, ApiError> {
    let (user, repo) = open(&s, &headers, id).await?;
    if repo.main.is_none() {
        return Ok(Json(json!({"commits":[]})));
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500).to_string();
    let mut args = vec!["log", "--topo-order", LOG_FORMAT, "-n", &limit, "main"];
    if let Some(path) = &q.path {
        super::check_path(path)?;
        args.extend(["--", path.as_str()]);
    }
    let out = repo.git(&args).await?;
    let entries = parse_log(&out.text());
    let ids: Vec<Uuid> = entries
        .iter()
        .filter_map(|e| e.chat)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let known = chats(&s.store.pool, user, &ids).await?;
    Ok(Json(
        json!({"commits":entries.iter().map(|e| entry_json(e, &known)).collect::<Vec<_>>()}),
    ))
}

fn check_sha(sha: &str) -> Result<(), ApiError> {
    if sha.len() < 4 || sha.len() > 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad("That is not a commit"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct CommitQuery {
    path: Option<String>,
}

fn numstat(out: &str) -> Vec<Value> {
    out.lines()
        .filter_map(|line| {
            let mut p = line.splitn(3, '\t');
            let (a, r, path) = (p.next()?, p.next()?, p.next()?);
            Some(json!({"path":path,"added":a.parse::<i64>().ok(),"removed":r.parse::<i64>().ok()}))
        })
        .collect()
}

async fn commit(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, sha)): Path<(Uuid, String)>,
    Query(q): Query<CommitQuery>,
) -> Result<Json<Value>, ApiError> {
    check_sha(&sha)?;
    let (user, repo) = open(&s, &headers, id).await?;
    let full = repo.resolve(&sha).await?.ok_or_else(ApiError::not_found)?;
    // Only commits in the project's history.
    if !repo
        .git(&["merge-base", "--is-ancestor", &full, "main"])
        .await?
        .ok
    {
        return Err(ApiError::not_found());
    }
    let out = repo.git(&["show", "-s", LOG_FORMAT, &full]).await?;
    let entry = parse_log(&out.text())
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    let body = repo
        .git(&["show", "-s", "--format=%b", &full])
        .await?
        .text();
    let body: String = body
        .lines()
        .filter(|l| !l.starts_with("Obleth-Chat:") && !l.starts_with("Obleth-Turn:"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    let parent = entry.parents.first().cloned();
    let stat = match &parent {
        Some(p) => {
            repo.git(&["diff", "--numstat", "--no-renames", p, &full])
                .await?
        }
        None => {
            repo.git(&[
                "show",
                "--numstat",
                "--format=",
                "--no-renames",
                "--root",
                &full,
            ])
            .await?
        }
    };
    let patch = match &q.path {
        Some(path) => {
            super::check_path(path)?;
            let out = match &parent {
                Some(p) => {
                    repo.git(&["diff", "--no-color", "--no-renames", p, &full, "--", path])
                        .await?
                }
                None => {
                    repo.git(&[
                        "show",
                        "--no-color",
                        "--format=",
                        "--root",
                        &full,
                        "--",
                        path,
                    ])
                    .await?
                }
            };
            let mut text = out.text();
            if text.len() > 200_000 {
                let mut end = 200_000;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            Some(text)
        }
        None => None,
    };
    let known = chats(
        &s.store.pool,
        user,
        &entry.chat.into_iter().collect::<Vec<_>>(),
    )
    .await?;
    Ok(Json(
        json!({"commit":entry_json(&entry, &known),"body":body,"files":numstat(&stat.text()),"patch":patch}),
    ))
}

async fn files(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (_, repo) = open(&s, &headers, id).await?;
    if repo.main.is_none() {
        return Ok(Json(json!({"files":[]})));
    }
    let out = repo.git(&["ls-tree", "-r", "-l", "-z", "main"]).await?;
    let files: Vec<Value> = out
        .text()
        .split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            let mut m = meta.split_whitespace();
            let (_mode, kind, _sha, size) = (m.next()?, m.next()?, m.next()?, m.next()?);
            (kind == "blob").then(|| json!({"path":path,"size":size.parse::<i64>().ok()}))
        })
        .take(5000)
        .collect();
    Ok(Json(json!({"files":files})))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
    sha: Option<String>,
}

async fn file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> Result<Json<Value>, ApiError> {
    super::check_path(&q.path)?;
    let rev = q.sha.clone().unwrap_or_else(|| "main".into());
    if rev != "main" {
        check_sha(&rev)?;
    }
    let (_, repo) = open(&s, &headers, id).await?;
    let spec = format!("{rev}:{}", q.path);
    let size = repo.git(&["cat-file", "-s", &spec]).await?;
    if !size.ok {
        return Err(ApiError::not_found());
    }
    let size: i64 = size.text().trim().parse().unwrap_or(0);
    if size > 500_000 {
        return Ok(Json(json!({"path":q.path,"size":size,"tooLarge":true})));
    }
    let out = repo.git(&["cat-file", "-p", &spec]).await?;
    let binary = out.stdout.contains(&0);
    Ok(Json(
        json!({"path":q.path,"size":size,"binary":binary,"content":(!binary).then(|| out.text())}),
    ))
}

/// Groups of consecutive lines last changed by the same commit.
fn parse_blame(out: &str) -> (Vec<Value>, Vec<String>) {
    let mut groups: Vec<(String, usize, Vec<String>)> = Vec::new();
    let mut info: HashMap<String, (String, i64, String)> = HashMap::new();
    let mut current = String::new();
    let mut line_no = 0usize;
    for line in out.split('\n') {
        if let Some(content) = line.strip_prefix('\t') {
            match groups.last_mut() {
                Some((sha, _, lines)) if *sha == current => lines.push(content.to_owned()),
                _ => groups.push((current.clone(), line_no, vec![content.to_owned()])),
            }
            continue;
        }
        let mut parts = line.split(' ');
        let first = parts.next().unwrap_or("");
        if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            current = first.to_owned();
            line_no = parts.nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            info.entry(current.clone()).or_default();
        } else if let Some(v) = line.strip_prefix("author ") {
            info.entry(current.clone()).or_default().0 = v.to_owned();
        } else if let Some(v) = line.strip_prefix("author-time ") {
            info.entry(current.clone()).or_default().1 = v.parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("summary ") {
            info.entry(current.clone()).or_default().2 = v.to_owned();
        }
    }
    let shas: Vec<String> = info.keys().cloned().collect();
    let groups = groups
        .into_iter()
        .map(|(sha, start, lines)| {
            let (author, time, summary) = info.get(&sha).cloned().unwrap_or_default();
            json!({"sha":sha,"start":start,"lines":lines,"author":author,"time":time,"subject":summary})
        })
        .collect();
    (groups, shas)
}

#[derive(Deserialize)]
struct BlameQuery {
    path: String,
}

async fn blame(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<BlameQuery>,
) -> Result<Json<Value>, ApiError> {
    super::check_path(&q.path)?;
    let (user, repo) = open(&s, &headers, id).await?;
    if repo.main.is_none() {
        return Err(ApiError::not_found());
    }
    let size: i64 = repo
        .git(&["cat-file", "-s", &format!("main:{}", q.path)])
        .await?
        .text()
        .trim()
        .parse()
        .map_err(|_| ApiError::not_found())?;
    if size > 500_000 {
        return Err(ApiError::bad("This file is too large to show line by line"));
    }
    let out = repo
        .git(&["blame", "--porcelain", "main", "--", &q.path])
        .await?;
    if !out.ok {
        return Err(ApiError::not_found());
    }
    let (mut groups, shas) = parse_blame(&out.text());
    // Which chat each commit came from.
    let mut args = vec!["log", "--no-walk=unsorted", LOG_FORMAT];
    args.extend(shas.iter().map(String::as_str));
    let entries = if shas.is_empty() {
        Vec::new()
    } else {
        parse_log(&repo.git(&args).await?.text())
    };
    let ids: Vec<Uuid> = entries
        .iter()
        .filter_map(|e| e.chat)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let known = chats(&s.store.pool, user, &ids).await?;
    let by_sha: HashMap<&str, &Entry> = entries.iter().map(|e| (e.sha.as_str(), e)).collect();
    for group in &mut groups {
        if let Some(e) = group["sha"].as_str().and_then(|s| by_sha.get(s)) {
            group["chat"] = e
                .chat
                .and_then(|c| known.get(&c).cloned())
                .unwrap_or(Value::Null);
            group["turn"] = json!(e.turn);
        }
    }
    Ok(Json(json!({"path":q.path,"groups":groups})))
}

#[derive(Deserialize)]
struct LinesQuery {
    path: String,
    start: u32,
    end: u32,
}

/// Every version of a range of lines, newest first, with what each change did.
async fn lines(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<LinesQuery>,
) -> Result<Json<Value>, ApiError> {
    super::check_path(&q.path)?;
    if q.start == 0 || q.end < q.start || q.end - q.start > 400 {
        return Err(ApiError::bad("Choose up to 400 lines"));
    }
    let (user, repo) = open(&s, &headers, id).await?;
    if repo.main.is_none() {
        return Err(ApiError::not_found());
    }
    let range = format!("-L{},{}:{}", q.start, q.end, q.path);
    let format = format!(
        "{}{}",
        "--format=@@@commit",
        LOG_FORMAT.trim_start_matches("--format=")
    );
    let out = repo
        .git(&["log", "-n", "30", "--no-color", &format, &range, "main"])
        .await?;
    if !out.ok {
        return Err(ApiError::bad("Those lines are not in the file"));
    }
    let text = out.text();
    let mut versions = Vec::new();
    for chunk in text.split("@@@commit").filter(|c| !c.trim().is_empty()) {
        let (head, patch) = chunk.split_once(END).unwrap_or((chunk, ""));
        let Some(entry) = parse_log(&format!("{head}{END}")).into_iter().next() else {
            continue;
        };
        let patch: String = patch.trim_start_matches('\n').chars().take(8000).collect();
        versions.push((entry, patch));
    }
    let ids: Vec<Uuid> = versions
        .iter()
        .filter_map(|(e, _)| e.chat)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let known = chats(&s.store.pool, user, &ids).await?;
    Ok(Json(json!({"versions":versions.iter().map(|(e, p)| {
        let mut v = entry_json(e, &known);
        v["patch"] = json!(p);
        v
    }).collect::<Vec<_>>()})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_log_records() {
        let out = format!(
            "{a}{SEP}{b} {c}{SEP}Jen{SEP}1700000000{SEP}Combine{SEP}6f7d0a7e-3f3c-4c55-9a43-0d9f1c8f2b11{SEP}{END}\n{b}{SEP}{SEP}Kim{SEP}1690000000{SEP}Start{SEP}{SEP}{END}\n",
            a = "a".repeat(40),
            b = "b".repeat(40),
            c = "c".repeat(40)
        );
        let entries = parse_log(&out);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].parents.len(), 2);
        assert!(entries[0].chat.is_some() && entries[1].chat.is_none());
        assert!(entries[1].parents.is_empty());
    }

    #[test]
    fn groups_blame_lines_by_commit() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let out = format!(
            "{a} 1 1 2\nauthor Kim\nauthor-time 1690000000\nsummary Start\nfilename x\n\tone\n{a} 2 2\n\ttwo\n{b} 3 3 1\nauthor Jen\nauthor-time 1700000000\nsummary Change\nfilename x\n\tthree\n"
        );
        let (groups, shas) = parse_blame(&out);
        assert_eq!(shas.len(), 2);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["lines"], json!(["one", "two"]));
        assert_eq!(groups[0]["start"], 1);
        assert_eq!(groups[1]["author"], "Jen");
        assert_eq!(groups[1]["start"], 3);
    }
}
