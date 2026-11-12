//! Files on a Slurm cluster, reached through a file session: a small job Chat submits
//! as the person, on demand, which connects out to Chat and carries out the chat's
//! file operations under the roots the cluster allows, until it has been idle for a
//! while or the job ends. Nothing on the cluster listens. slurmrestd submits the job
//! with the person's own token, so the job has exactly their permissions, and the
//! job's hello must name that same user. The job runs `cluster_agent.py`, shipped
//! inside the batch script; each operation is a row in `cluster_calls` that the job
//! claims, carries out and answers.
use super::{mcp::Tool, slurm, wakeups::Wakeups, Connections};
use crate::error::ApiError;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

/// The program the session job runs.
pub const AGENT: &str = include_str!("cluster_agent.py");
/// How long the first call of a session waits for its job to start and answer.
const START_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long a call waits once the session is live.
const CALL_WAIT: Duration = Duration::from_secs(3 * 60);
/// How often a call waiting for its session's job to start looks the job up, so a
/// job that ended before it reached Chat is reported at once; also the longest one
/// look may take, so a slow slurmrestd does not hold up the answer.
const JOB_CHECK: Duration = Duration::from_secs(5);
/// The session job's name, which a look-up checks so an old job that once had the
/// same id is not taken for it.
const JOB_NAME: &str = "chat-files";
/// A live session not heard from for this long is taken as gone: longer than any
/// one operation may take, since a job busy with one does not poll.
const STALE_SECONDS: f64 = 300.0;
/// How long the job's poll for work waits before answering "nothing yet".
const POLL_WAIT: Duration = Duration::from_secs(50);
/// How long a wait goes without a wakeup before it looks for itself.
const FALLBACK: Duration = Duration::from_secs(5);
/// The longest result the job may post; the model reads the first 48 KB.
pub const RESULT_LIMIT: usize = 320 * 1024;
/// The job exits after this long without a call.
const IDLE_SECONDS: i64 = 20 * 60;
const DEFAULT_HOURS: i64 = 8;
const MAX_HOURS: i64 = 24 * 7;
const DEFAULT_ROOTS: &str = "~";
const REMOVE_LIMIT: usize = 200;
const WRITE_LIMIT: usize = 1024 * 1024;

/// What a cluster set to reach files runs its session jobs with.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub partition: Option<String>,
    pub qos: Option<String>,
    pub hours: i64,
    /// Comma-separated roots the session may reach; `~` and `$USER` expand on the
    /// cluster.
    pub roots: String,
}

/// The options of a Slurm connection set to reach files, or None.
pub fn options(options: &Value) -> Option<Options> {
    if options["files"] != true {
        return None;
    }
    let text = |key: &str| {
        options[key]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    Some(Options {
        partition: text("filesPartition"),
        qos: text("filesQos"),
        hours: options["filesHours"]
            .as_i64()
            .filter(|h| (1..=MAX_HOURS).contains(h))
            .unwrap_or(DEFAULT_HOURS),
        roots: text("filesRoots").unwrap_or_else(|| DEFAULT_ROOTS.into()),
    })
}

fn name_like(value: &Value, what: &str) -> Result<Option<String>, ApiError> {
    let Some(value) = value.as_str().map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 63
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(ApiError::bad(format!("Enter a valid {what}")));
    }
    Ok(Some(value.to_owned()))
}

/// Adds the files options to a Slurm connection's cleaned options.
pub fn clean(options: &Value, clean: &mut Value) -> Result<(), ApiError> {
    if options["files"] != true {
        return Ok(());
    }
    clean["files"] = json!(true);
    if let Some(partition) = name_like(&options["filesPartition"], "partition name")? {
        clean["filesPartition"] = json!(partition);
    }
    if let Some(qos) = name_like(&options["filesQos"], "QoS name")? {
        clean["filesQos"] = json!(qos);
    }
    match &options["filesHours"] {
        Value::Null => {}
        value => {
            let hours = value
                .as_i64()
                .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
                .filter(|h| (1..=MAX_HOURS).contains(h))
                .ok_or_else(|| {
                    ApiError::bad(format!("The session length is 1 to {MAX_HOURS} hours"))
                })?;
            if hours != DEFAULT_HOURS {
                clean["filesHours"] = json!(hours);
            }
        }
    }
    if let Some(roots) = options["filesRoots"]
        .as_str()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        let parts: Vec<&str> = roots
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty()
            || roots.len() > 1024
            || parts.iter().any(|p| {
                p.len() > 255
                    || !(p.starts_with('/') || p.starts_with('~'))
                    || p.chars().any(|c| c.is_control() || "\"'`\\".contains(c))
            })
        {
            return Err(ApiError::bad(
                "Roots are absolute paths or ~, separated by commas; $USER stands for the Slurm user",
            ));
        }
        let roots = parts.join(",");
        if roots != DEFAULT_ROOTS {
            clean["filesRoots"] = json!(roots);
        }
    }
    Ok(())
}

/// What the model is told about the files it can reach.
pub fn about(options: &Options) -> String {
    format!(
        "with your files under {} reachable through its files tools",
        options.roots.replace(',', ", ")
    )
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    read_only: bool,
    properties: Value,
    required: &[&str],
) -> Tool {
    Tool {
        name: name.into(),
        title: Some(title.into()),
        description: description.into(),
        input_schema: json!({"type":"object","properties":properties,"required":required}),
        read_only,
    }
}

/// The files tools of a cluster set to reach files. The first call in a while
/// starts a session job, which can take a minute or more in a queue.
pub fn tools() -> Vec<Tool> {
    let path = |what: &str| json!({"type":"string","description":what});
    vec![
        tool("files_list", "List files", "The entries of a directory on the cluster, as the person's own account sees them: type, size, modified time and name, directories first; up to 500. Paths are absolute, or relative to the home directory; ~ is the home directory.", true, json!({
            "path": path("Directory to list; defaults to the home directory"),
            "hidden": {"type":"boolean","description":"Also list entries whose names start with a dot"}
        }), &[]),
        tool("files_find", "Find files", "Files and directories under a path whose names match a pattern, with optional size and age limits; up to 500 matches, within 60 seconds.", true, json!({
            "path": path("Directory to search under; defaults to the home directory"),
            "name": {"type":"string","description":"Shell pattern for the name, such as *.log or checkpoint-*; defaults to everything"},
            "larger_than_mb": {"type":"number","description":"Only entries at least this many MB"},
            "older_than_days": {"type":"number","description":"Only entries not modified in this many days"},
            "limit": {"type":"integer","minimum":1,"maximum":500}
        }), &[]),
        tool("files_read", "Read file", "The text of a file, up to 48 KB at a time (256 KB at most), from an offset; says so when the file is binary. Read in parts for a long file.", true, json!({
            "path": path("The file"),
            "offset": {"type":"integer","minimum":0,"description":"Byte offset to start at"},
            "max_bytes": {"type":"integer","minimum":1,"maximum":262144}
        }), &["path"]),
        tool("files_search", "Search in files", "Lines matching a regular expression in text files under a path (or in one file), with file and line number; up to 200 lines, within 60 seconds. Files over 8 MB and binary files are skipped.", true, json!({
            "path": path("Directory or file to search; defaults to the home directory"),
            "pattern": {"type":"string","description":"Regular expression (Python syntax)"},
            "name": {"type":"string","description":"Only files whose names match this shell pattern, such as *.out"},
            "limit": {"type":"integer","minimum":1,"maximum":200}
        }), &["pattern"]),
        tool("files_usage", "Disk usage", "What each entry of a directory holds on disk, largest first, with the total, for finding what takes the space. A large tree takes a while (up to 120 seconds); measure a subdirectory for a quicker answer.", true, json!({
            "path": path("Directory to measure; defaults to the home directory")
        }), &[]),
        tool("files_write", "Write file", "Writes text to a file, creating directories on the way. An existing file is replaced only with overwrite.", false, json!({
            "path": path("The file to write"),
            "content": {"type":"string","description":"The whole file's text, up to 1 MB"},
            "overwrite": {"type":"boolean","description":"Replace the file if it exists"}
        }), &["path","content"]),
        tool("files_move", "Move file", "Moves or renames a file or directory. A destination that is an existing directory receives it; an existing file is never replaced.", false, json!({
            "from": path("What to move"),
            "to": path("Where to; a directory or a new path")
        }), &["from","to"]),
        tool("files_remove", "Move to trash", "Moves files or directories to the person's trash directory on the cluster (~/.chat-trash/<date>), where they keep their paths and can be moved back. Nothing is deleted outright. Up to 200 paths.", false, json!({
            "paths": {"type":"array","minItems":1,"maxItems":200,"items":{"type":"string"},"description":"The files or directories to move to the trash"}
        }), &["paths"]),
    ]
}

fn given_path(args: &Value, key: &str) -> Result<String, String> {
    let path = args[key].as_str().map(str::trim).unwrap_or("");
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(format!("{key} must be a path"));
    }
    Ok(path.to_owned())
}

fn size(bytes: usize) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// What the person is asked to allow, read from the arguments.
pub fn changes(tool: &str, args: &Value) -> Result<Vec<Value>, String> {
    match tool {
        "files_write" => {
            let path = given_path(args, "path")?;
            let content = args["content"].as_str().ok_or("content must be text")?;
            if content.len() > WRITE_LIMIT {
                return Err(format!(
                    "content is {}; a write is at most {}",
                    size(content.len()),
                    size(WRITE_LIMIT)
                ));
            }
            let mut facts = vec![json!(["Size", size(content.len())])];
            if args["overwrite"] == true {
                facts.push(json!(["Replaces the file", "if it exists"]));
            }
            Ok(vec![json!({"verb":"Write","what":path,"facts":facts})])
        }
        "files_move" => {
            let from = given_path(args, "from")?;
            let to = given_path(args, "to")?;
            Ok(vec![
                json!({"verb":"Move","what":from,"facts":[["To", to]]}),
            ])
        }
        "files_remove" => {
            let paths: Vec<String> = args["paths"]
                .as_array()
                .ok_or("paths must be a list")?
                .iter()
                .map(|p| {
                    let path = p.as_str().map(str::trim).unwrap_or("");
                    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                        Err("paths must be paths".to_string())
                    } else {
                        Ok(path.to_owned())
                    }
                })
                .collect::<Result<_, _>>()?;
            if paths.is_empty() || paths.len() > REMOVE_LIMIT {
                return Err(format!("paths lists 1 to {REMOVE_LIMIT} paths"));
            }
            let what = match paths.len() {
                1 => paths[0].clone(),
                n => format!("{n} files or directories"),
            };
            let facts: Vec<Value> = paths
                .iter()
                .take(10)
                .map(|p| json!(["Path", p]))
                .chain(
                    (paths.len() > 10)
                        .then(|| json!(["And", format!("{} more", paths.len() - 10)])),
                )
                .collect();
            Ok(vec![
                json!({"verb":"Move to trash","what":what,"facts":facts}),
            ])
        }
        _ => Ok(vec![]),
    }
}

/// Where a session's calls are filed and its job started.
pub(super) struct Files {
    pub pool: PgPool,
    pub user: Uuid,
    /// Chat's own address, which the job connects to.
    pub origin: Url,
    pub wakeups: Wakeups,
}

/// What a job waits on: its session's next call.
fn session_key(session: Uuid) -> String {
    format!("session:{session}")
}
/// What a chat waits on: a call's result.
fn call_key(call: Uuid) -> String {
    format!("call:{call}")
}

/// A call's answer so far, with its session's state and error.
type CallRow = (Option<bool>, Option<String>, String, Option<String>);

#[derive(sqlx::FromRow)]
struct Open {
    id: Uuid,
    state: String,
    job_id: Option<i64>,
}

impl Files {
    /// Runs one files tool: finds or starts the person's session on the cluster,
    /// files the call and waits for the job to answer it.
    pub async fn run(
        &self,
        client: &mut slurm::Client,
        connection: Uuid,
        options: &Options,
        generation: Uuid,
        tool: &str,
        arguments: &Value,
    ) -> Result<(bool, String), String> {
        let op = tool
            .strip_prefix("files_")
            .ok_or_else(|| format!("There is no files tool named {tool}"))?;
        let (session, started) = self.session(client, connection, options).await?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO cluster_calls(id,session_id,generation_id,op,arguments) VALUES($1,$2,$3,$4,$5)")
            .bind(id).bind(session.id).bind(generation).bind(op).bind(arguments)
            .execute(&self.pool).await
            .map_err(|e| format!("The call could not be sent to the cluster: {e}"))?;
        self.wakeups
            .send(&self.pool, &session_key(session.id))
            .await;
        let wait = if session.state == "live" {
            CALL_WAIT
        } else {
            START_WAIT
        };
        let deadline = tokio::time::Instant::now() + wait;
        let job = session.job_id.or(started);
        // Until the job says hello, it is looked up now and then: one that ended
        // first never will, and how it ended says why.
        let mut checked = tokio::time::Instant::now();
        let mut waiting: Option<slurm::Standing> = None;
        let outcome = loop {
            if tokio::time::Instant::now() >= deadline {
                break None;
            }
            let row: Result<Option<CallRow>, _> = sqlx::query_as(
                "SELECT c.ok, c.output, s.state, s.error FROM cluster_calls c JOIN cluster_sessions s ON s.id=c.session_id WHERE c.id=$1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await;
            match row {
                Ok(Some((Some(ok), output, _, _))) => {
                    break Some(Ok((ok, output.unwrap_or_default())))
                }
                Ok(Some((None, _, state, error))) if state == "ended" => {
                    break Some(Err(error.unwrap_or_else(|| "The file session ended".into())));
                }
                Ok(Some((None, _, state, _)))
                    if state == "starting" && checked.elapsed() >= JOB_CHECK =>
                {
                    checked = tokio::time::Instant::now();
                    let left = deadline.saturating_duration_since(checked).min(JOB_CHECK);
                    let looked = match job {
                        Some(job) => tokio::time::timeout(left, client.standing(job))
                            .await
                            .ok()
                            .flatten()
                            .filter(is_session_job)
                            .map(|standing| (job, standing)),
                        None => None,
                    };
                    if let Some((job, standing)) = looked {
                        let gone = !standing.active();
                        if gone || standing.held_for_good() {
                            let why = ended_early(job, &standing, &self.origin);
                            // Unless it said hello and ended in between.
                            let ended = sqlx::query("UPDATE cluster_sessions SET state='ended', error=$2 WHERE id=$1 AND state='starting'")
                                .bind(session.id).bind(&why).execute(&self.pool).await
                                .map(|r| r.rows_affected() > 0)
                                .unwrap_or(false);
                            if ended {
                                if !gone {
                                    let _ = client.cancel(job).await;
                                }
                                break Some(Err(why));
                            }
                        } else {
                            waiting = Some(standing);
                        }
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break Some(Err("The call was dropped".into())),
                Err(_) => {}
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            self.wakeups.wait(&call_key(id), left.min(FALLBACK)).await;
        };
        let _ = sqlx::query("DELETE FROM cluster_calls WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await;
        match outcome {
            Some(Ok((ok, output))) => Ok((
                ok,
                match started {
                    Some(job) => format!("(A file session started on the cluster as job {job}; it stays for later calls.)\n{output}"),
                    None => output,
                },
            )),
            Some(Err(error)) => Err(error),
            None => {
                // The session is no good: the next call starts a new one.
                let why = if session.state == "live" {
                    "The file session on the cluster did not answer within 3 minutes. Try again; a new session starts."
                        .to_string()
                } else {
                    if let Some(job) = job {
                        let _ = client.cancel(job).await;
                    }
                    not_started(job, waiting.as_ref())
                };
                let _ = sqlx::query("UPDATE cluster_sessions SET state='ended', error=$2 WHERE id=$1 AND state<>'ended'")
                    .bind(session.id).bind(&why).execute(&self.pool).await;
                // A job still polling learns it at once.
                self.wakeups
                    .send(&self.pool, &session_key(session.id))
                    .await;
                Err(why)
            }
        }
    }

    /// The person's open session on the cluster, started if there is none. The
    /// job id is given when this call started it.
    async fn session(
        &self,
        client: &mut slurm::Client,
        connection: Uuid,
        options: &Options,
    ) -> Result<(Open, Option<i64>), String> {
        let database = |e: sqlx::Error| format!("Chat could not look up the file session: {e}");
        // A session not heard from, one whose job never started, or one past its
        // time is over.
        sqlx::query(
            "UPDATE cluster_sessions SET state='ended', error=COALESCE(error, CASE WHEN state='live' THEN 'The file session stopped answering' ELSE 'The file session job did not start' END) \
             WHERE connection_id=$1 AND user_id=$2 AND state<>'ended' AND ( \
               (state='live' AND last_seen_at < NOW() - make_interval(secs => $3)) OR \
               (state='starting' AND created_at < NOW() - make_interval(secs => $4)) OR \
               ends_at < NOW())",
        )
        .bind(connection)
        .bind(self.user)
        .bind(STALE_SECONDS)
        .bind(START_WAIT.as_secs_f64())
        .execute(&self.pool)
        .await
        .map_err(database)?;
        // A running job keeps the directories it was started with: once the cluster's
        // setting changes, its session ends (the job stops at its next poll) and a
        // new one starts with the directories set now.
        let changed: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE cluster_sessions SET state='ended', error='The directories Chat may reach on this cluster changed, so a new file session starts' \
             WHERE connection_id=$1 AND user_id=$2 AND state<>'ended' AND roots IS DISTINCT FROM $3 RETURNING id",
        )
        .bind(connection)
        .bind(self.user)
        .bind(&options.roots)
        .fetch_all(&self.pool)
        .await
        .map_err(database)?;
        for session in changed {
            self.wakeups.send(&self.pool, &session_key(session)).await;
        }
        let open = || async {
            sqlx::query_as::<_, Open>(
                "SELECT id, state, job_id FROM cluster_sessions WHERE connection_id=$1 AND user_id=$2 AND state<>'ended'",
            )
            .bind(connection)
            .bind(self.user)
            .fetch_optional(&self.pool)
            .await
        };
        if let Some(session) = open().await.map_err(database)? {
            return Ok((session, None));
        }
        let token = {
            let bytes: [u8; 32] = rand::random();
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        let id = Uuid::new_v4();
        let inserted = sqlx::query(
            "INSERT INTO cluster_sessions(id,connection_id,user_id,token_hash,state,username,ends_at,roots) \
             VALUES($1,$2,$3,$4,'starting',$5,NOW() + make_interval(hours => $6),$7) \
             ON CONFLICT (connection_id, user_id) WHERE state <> 'ended' DO NOTHING",
        )
        .bind(id)
        .bind(connection)
        .bind(self.user)
        .bind(&hash)
        .bind(client.user())
        .bind(options.hours as i32)
        .bind(&options.roots)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        if inserted.rows_affected() == 0 {
            // Another reply started one just now.
            return open()
                .await
                .map_err(database)?
                .map(|s| (s, None))
                .ok_or_else(|| "The file session could not be started; try again".into());
        }
        let job = json!({
            "name": JOB_NAME,
            "current_working_directory": format!("/home/{}", client.user()),
            "environment": [
                "PATH=/usr/local/bin:/usr/bin:/bin",
                format!("OBLETH_CHAT_URL={}", self.origin.as_str().trim_end_matches('/')),
                format!("OBLETH_CHAT_SESSION={token}"),
                format!("OBLETH_CHAT_IDLE={IDLE_SECONDS}"),
                format!("OBLETH_CHAT_ROOTS={}", options.roots),
            ],
            "time_limit": {"set": true, "number": options.hours * 60},
            "cpus_per_task": 1,
            "memory_per_node": {"set": true, "number": 1024},
            "standard_output": "/dev/null",
            "standard_error": "/dev/null",
        });
        let mut job = job;
        if let Some(partition) = &options.partition {
            job["partition"] = json!(partition);
        }
        if let Some(qos) = &options.qos {
            job["qos"] = json!(qos);
        }
        let submitted = client.submit_job(session_script(), job).await;
        let job_id = match submitted {
            Ok((job_id, _)) => job_id,
            Err(error) => {
                let _ = sqlx::query("DELETE FROM cluster_sessions WHERE id=$1")
                    .bind(id)
                    .execute(&self.pool)
                    .await;
                return Err(format!("The file session could not start: {error}"));
            }
        };
        let _ = sqlx::query("UPDATE cluster_sessions SET job_id=$2 WHERE id=$1")
            .bind(id)
            .bind(job_id)
            .execute(&self.pool)
            .await;
        Ok((
            Open {
                id,
                state: "starting".into(),
                job_id: Some(job_id),
            },
            Some(job_id),
        ))
    }
}

/// How the session job exits when it cannot serve; Chat reads the code back from
/// Slurm, since the job could not tell Chat itself. Kept in step with the agent.
const EXIT_UNRESOLVED: i64 = 3;
const EXIT_UNREACHABLE: i64 = 4;
const EXIT_REFUSED: i64 = 5;
const EXIT_NO_PYTHON: i64 = 6;
const EXIT_OLD_PYTHON: i64 = 7;
const EXIT_SERVER: i64 = 8;

/// A look-up that found this session's job, not an older one that once had its id.
fn is_session_job(standing: &slurm::Standing) -> bool {
    let recent = |t: i64| chrono::Utc::now().timestamp() - t < 2 * START_WAIT.as_secs() as i64;
    (standing.name.is_empty() || standing.name == JOB_NAME) && standing.submitted.is_none_or(recent)
}

fn on_node(node: &str) -> String {
    match node {
        "" | "None" | "None assigned" | "(null)" => String::new(),
        node => format!(" on {node}"),
    }
}

/// Why a session job ended, or can never start, before it reached Chat: read from
/// how it ended, the agent's own exit codes first. What only an administrator can
/// fix says so; what is over by itself says to try again.
fn ended_early(job: i64, standing: &slurm::Standing, origin: &Url) -> String {
    let host = origin.host_str().unwrap_or("Chat");
    let address = origin.as_str().trim_end_matches('/');
    let node = on_node(&standing.node);
    let state = if standing.state.is_empty() {
        "ended".to_owned()
    } else {
        standing.state.clone()
    };
    if standing.held_for_good() {
        return format!(
            "The file session job {job} cannot start with this cluster's file session settings: Slurm holds it ({}) and waiting will not clear that, so it was cancelled. An administrator changes the partition, QoS or session length for file sessions on the cluster's connection.",
            standing.reason
        );
    }
    let direct = "The session job connects straight to Chat, without a proxy";
    let (cause, fix) = match (standing.state.as_str(), standing.exit) {
        (_, Some(EXIT_UNRESOLVED)) => (
            format!("the node could not look up Chat's address, {host}. {direct}, so the cluster's compute nodes must resolve {host} themselves and reach {address}"),
            true,
        ),
        (_, Some(EXIT_UNREACHABLE)) => (
            format!("the node could not connect to Chat at {address}, most likely because of a firewall or a certificate the node does not trust. {direct}, so the cluster's compute nodes must reach {address} themselves"),
            true,
        ),
        (_, Some(EXIT_REFUSED)) => (
            format!("the server at {address} turned the session away: Chat itself, or a proxy, firewall or load balancer in front of it"),
            true,
        ),
        (_, Some(EXIT_SERVER)) => (
            format!("the server at {address} answered with an error: Chat itself, or a proxy or load balancer in front of it, which may be restarting"),
            false,
        ),
        (_, Some(EXIT_NO_PYTHON)) => ("python3 is not installed on the node".to_owned(), true),
        (_, Some(EXIT_OLD_PYTHON)) => ("the node's python3 is older than 3.6".to_owned(), true),
        (s, _) if s.contains("CANCELLED") => ("it was cancelled, by a person or by Slurm".to_owned(), false),
        (s, _) if s.contains("TIMEOUT") || s.contains("DEADLINE") => {
            ("it reached its time limit".to_owned(), false)
        }
        (s, _) if s.contains("OUT_OF_MEMORY") => ("it ran out of memory".to_owned(), false),
        (s, _) if s.contains("NODE_FAIL") || s.contains("BOOT_FAIL") => {
            ("its node failed".to_owned(), false)
        }
        (s, _) if s.contains("PREEMPTED") => ("it was preempted".to_owned(), false),
        (_, Some(code)) if code != 0 => (format!("it exited with code {code}"), false),
        _ => match standing.signal {
            Some(signal) if signal != 0 => (format!("it was stopped by signal {signal}"), false),
            _ => ("it exited without saying why".to_owned(), false),
        },
    };
    let next = if fix {
        "An administrator fixes that; then try again, and a new session starts."
    } else {
        "Try again; a new session starts."
    };
    format!(
        "The file session job {job}{node} ended ({state}) before it reached Chat: {cause}. {next}"
    )
}

/// Why a session's job had not reached Chat when the wait ran out.
fn not_started(job: Option<i64>, waiting: Option<&slurm::Standing>) -> String {
    let named = job.map(|j| format!(" {j}")).unwrap_or_default();
    match waiting {
        Some(w) if w.state.contains("RUNNING") => format!(
            "The file session's job{named} started{}, but its program neither reached Chat nor ended within 10 minutes, so it was cancelled. Something on the node may have hung, such as a slow file system; an administrator can look at the node.",
            on_node(&w.node)
        ),
        _ => {
            let reason = waiting
                .map(|w| w.reason.as_str())
                .filter(|r| !r.is_empty() && *r != "None");
            let held = reason
                .map(|r| format!(" Slurm kept it waiting: {r}."))
                .unwrap_or_default();
            let after = match reason {
                None | Some("Priority" | "Resources") => " The partition and QoS set for file sessions on this cluster are busy; try again later.",
                Some(_) => " If waiting does not clear that, an administrator changes the partition, QoS or session length for file sessions on the cluster's connection.",
            };
            format!(
                "The file session's job{named} did not start within 10 minutes, so it was cancelled.{held}{after}"
            )
        }
    }
}

/// The batch script of a session job: the agent, read by python3 from the script
/// itself. Without python3 the job tells Chat why it cannot serve, so the chat
/// learns it rather than waiting.
pub fn session_script() -> String {
    let end = format!("OBLETH_CHAT_AGENT_{}", Uuid::new_v4().simple());
    format!(
        r#"#!/bin/bash
# Chat's file session: carries out one chat's file operations as you, from this node,
# until it has been idle for 20 minutes. It connects out to Chat; nothing listens here.
if ! command -v python3 >/dev/null 2>&1; then
  curl -s --connect-timeout 10 -m 30 -X POST -H "Authorization: Bearer $OBLETH_CHAT_SESSION" -H 'Content-Type: application/json' \
    -d "{{\"reason\":\"python3 is not installed on $(hostname -s), so the file session cannot run there\"}}" "$OBLETH_CHAT_URL/api/cluster-sessions/bye" >/dev/null 2>&1
  exit {EXIT_NO_PYTHON}
fi
if ! python3 -c 'import sys; sys.exit(sys.version_info < (3, 6))' >/dev/null 2>&1; then
  curl -s --connect-timeout 10 -m 30 -X POST -H "Authorization: Bearer $OBLETH_CHAT_SESSION" -H 'Content-Type: application/json' \
    -d "{{\"reason\":\"python3 on $(hostname -s) is older than 3.6, so the file session cannot run there\"}}" "$OBLETH_CHAT_URL/api/cluster-sessions/bye" >/dev/null 2>&1
  exit {EXIT_OLD_PYTHON}
fi
exec python3 - <<'{end}'
{AGENT}
{end}
"#
    )
}

/// The session a job's request belongs to.
#[derive(sqlx::FromRow)]
struct Session {
    id: Uuid,
    state: String,
    username: String,
}

async fn session_of(s: &crate::AppState, headers: &HeaderMap) -> Result<Session, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(ApiError::unauthorized)?;
    let hash = Sha256::digest(token.as_bytes()).to_vec();
    let session: Session =
        sqlx::query_as("SELECT id, state, username FROM cluster_sessions WHERE token_hash=$1")
            .bind(&hash)
            .fetch_optional(&s.store.pool)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
    if session.state == "ended" {
        return Err(ApiError(
            StatusCode::GONE,
            "This file session has ended".into(),
        ));
    }
    Ok(session)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    username: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    version: i32,
}

/// The job's first request: who it runs as and where. A job running as someone
/// other than the Slurm user the token was saved for ends the session.
pub async fn hello(
    State(s): State<crate::AppState>,
    headers: HeaderMap,
    Json(body): Json<Hello>,
) -> Result<Json<Value>, ApiError> {
    let session = session_of(&s, &headers).await?;
    if body.username != session.username {
        let why = format!(
            "The file session job ran as {}, not {}: the Slurm token saved for this cluster belongs to another account",
            clip(&body.username, 64),
            session.username
        );
        sqlx::query("UPDATE cluster_sessions SET state='ended', error=$2 WHERE id=$1")
            .bind(session.id)
            .bind(&why)
            .execute(&s.store.pool)
            .await?;
        return Err(ApiError(StatusCode::CONFLICT, why));
    }
    sqlx::query("UPDATE cluster_sessions SET state='live', host=$2, agent_version=$3, started_at=NOW(), last_seen_at=NOW(), error=NULL WHERE id=$1")
        .bind(session.id)
        .bind(clip(&body.host, 253))
        .bind(body.version)
        .execute(&s.store.pool)
        .await?;
    Ok(Json(
        json!({"idle": IDLE_SECONDS, "poll": POLL_WAIT.as_secs()}),
    ))
}

/// The next operation for the job, waiting a while for one; 204 when none came.
pub async fn next(
    State(s): State<crate::AppState>,
    headers: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let session = session_of(&s, &headers).await?;
    sqlx::query("UPDATE cluster_sessions SET last_seen_at=NOW() WHERE id=$1")
        .bind(session.id)
        .execute(&s.store.pool)
        .await?;
    let wakeups = s.connections.as_ref().map(Connections::wakeups);
    let deadline = tokio::time::Instant::now() + POLL_WAIT;
    let mut first = true;
    loop {
        // Woken, or looking by itself: a session ended meanwhile sends the job away.
        if !first {
            let state: Option<String> =
                sqlx::query_scalar("SELECT state FROM cluster_sessions WHERE id=$1")
                    .bind(session.id)
                    .fetch_optional(&s.store.pool)
                    .await?;
            if state.as_deref() != Some("live") {
                return Err(ApiError(
                    StatusCode::GONE,
                    "This file session has ended".into(),
                ));
            }
        }
        first = false;
        let claimed: Option<(Uuid, String, Value)> = sqlx::query_as(
            "UPDATE cluster_calls SET claimed_at=NOW() WHERE id=(SELECT id FROM cluster_calls WHERE session_id=$1 AND ok IS NULL AND claimed_at IS NULL ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING id, op, arguments",
        )
        .bind(session.id)
        .fetch_optional(&s.store.pool)
        .await?;
        if let Some((id, op, arguments)) = claimed {
            return Ok(Json(json!({"id":id,"op":op,"arguments":arguments})).into_response());
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() || s.shutdown.is_cancelled() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        match &wakeups {
            Some(wakeups) => {
                wakeups
                    .wait(&session_key(session.id), left.min(FALLBACK))
                    .await;
            }
            None => tokio::time::sleep(left.min(FALLBACK)).await,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Result_ {
    ok: bool,
    #[serde(default)]
    output: String,
}

/// The job's result for one call.
pub async fn resolve(
    State(s): State<crate::AppState>,
    headers: HeaderMap,
    Path(call): Path<Uuid>,
    Json(body): Json<Result_>,
) -> Result<StatusCode, ApiError> {
    let session = session_of(&s, &headers).await?;
    let done = sqlx::query(
        "UPDATE cluster_calls SET ok=$3, output=$4 WHERE id=$1 AND session_id=$2 AND ok IS NULL",
    )
    .bind(call)
    .bind(session.id)
    .bind(body.ok)
    .bind(clip(&body.output, RESULT_LIMIT))
    .execute(&s.store.pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This call is no longer waiting".into(),
        ));
    }
    let _ = sqlx::query("UPDATE cluster_sessions SET last_seen_at=NOW() WHERE id=$1")
        .bind(session.id)
        .execute(&s.store.pool)
        .await;
    if let Some(wakeups) = s.connections.as_ref().map(Connections::wakeups) {
        wakeups.send(&s.store.pool, &call_key(call)).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct Bye {
    #[serde(default)]
    reason: String,
}

/// The job is leaving: idle, ended, or unable to serve.
pub async fn bye(
    State(s): State<crate::AppState>,
    headers: HeaderMap,
    Json(body): Json<Bye>,
) -> Result<StatusCode, ApiError> {
    let session = session_of(&s, &headers).await?;
    let why = match body.reason.trim() {
        "" => "The file session ended".to_string(),
        reason => format!("The file session ended: {}", clip(reason, 300)),
    };
    sqlx::query("UPDATE cluster_sessions SET state='ended', error=$2 WHERE id=$1")
        .bind(session.id)
        .bind(why)
        .execute(&s.store.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The person's open session on a cluster, for the Clusters page: its state, job,
/// node and times; None when there is none.
pub async fn current(pool: &PgPool, connection: Uuid, user: Uuid) -> Option<Value> {
    #[derive(sqlx::FromRow)]
    struct Current {
        state: String,
        job_id: Option<i64>,
        host: Option<String>,
        created_at: DateTime<Utc>,
        started_at: Option<DateTime<Utc>>,
        last_seen_at: Option<DateTime<Utc>>,
        ends_at: DateTime<Utc>,
    }
    let row: Current = sqlx::query_as(
        "SELECT state, job_id, host, created_at, started_at, last_seen_at, ends_at FROM cluster_sessions WHERE connection_id=$1 AND user_id=$2 AND state<>'ended'",
    )
    .bind(connection)
    .bind(user)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    Some(
        json!({"state":row.state,"jobId":row.job_id,"host":row.host,"createdAt":row.created_at,"startedAt":row.started_at,"lastSeenAt":row.last_seen_at,"endsAt":row.ends_at}),
    )
}

/// Ends a person's sessions on a cluster, as when their credential is removed. A
/// job still running learns it at its next poll and exits.
pub async fn end_sessions(pool: &PgPool, wakeups: &Wakeups, connection: Uuid, user: Uuid) {
    let ended: Vec<Uuid> = sqlx::query_scalar("UPDATE cluster_sessions SET state='ended', error='The file session was ended' WHERE connection_id=$1 AND user_id=$2 AND state<>'ended' RETURNING id")
        .bind(connection)
        .bind(user)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for session in ended {
        wakeups.send(pool, &session_key(session)).await;
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} more bytes)", &text[..end], text.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_read_with_their_defaults() {
        assert_eq!(options(&json!({})), None);
        assert_eq!(options(&json!({"files":false})), None);
        assert_eq!(
            options(&json!({"files":true})),
            Some(Options {
                partition: None,
                qos: None,
                hours: DEFAULT_HOURS,
                roots: "~".into()
            })
        );
        assert_eq!(
            options(
                &json!({"files":true,"filesPartition":"htc","filesQos":"chat","filesHours":2,"filesRoots":"~,/scratch/$USER"})
            ),
            Some(Options {
                partition: Some("htc".into()),
                qos: Some("chat".into()),
                hours: 2,
                roots: "~,/scratch/$USER".into()
            })
        );
    }

    #[test]
    fn options_are_cleaned_and_defaults_left_out() {
        let mut out = json!({});
        clean(&json!({"files":false,"filesPartition":"x"}), &mut out).unwrap();
        assert_eq!(out, json!({}));
        let mut out = json!({});
        clean(
            &json!({"files":true,"filesPartition":" htc ","filesQos":"","filesHours":"8","filesRoots":" ~ , /scratch/$USER "}),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"files":true,"filesPartition":"htc","filesRoots":"~,/scratch/$USER"})
        );
        let mut out = json!({});
        clean(&json!({"files":true,"filesHours":12}), &mut out).unwrap();
        assert_eq!(out, json!({"files":true,"filesHours":12}));
        for bad in [
            json!({"files":true,"filesPartition":"a b"}),
            json!({"files":true,"filesQos":"q/os"}),
            json!({"files":true,"filesHours":0}),
            json!({"files":true,"filesHours":1000}),
            json!({"files":true,"filesRoots":"scratch"}),
            json!({"files":true,"filesRoots":"/a\"b"}),
        ] {
            assert!(clean(&bad, &mut json!({})).is_err(), "{bad}");
        }
        let mut out = json!({});
        assert!(clean(&json!({"files":true,"filesRoots":"/x, "}), &mut out).is_ok());
        assert_eq!(out["filesRoots"], "/x");
    }

    #[test]
    fn a_review_names_the_path_and_what_the_change_does() {
        let write = changes(
            "files_write",
            &json!({"path":"~/run.sh","content":"echo hi\n"}),
        )
        .unwrap();
        assert_eq!(write[0]["verb"], "Write");
        assert_eq!(write[0]["what"], "~/run.sh");
        assert_eq!(write[0]["facts"], json!([["Size", "8 B"]]));
        let over = changes(
            "files_write",
            &json!({"path":"a","content":"x","overwrite":true}),
        )
        .unwrap();
        assert_eq!(over[0]["facts"][1][0], "Replaces the file");
        assert!(changes("files_write", &json!({"path":"","content":"x"})).is_err());
        assert!(changes(
            "files_write",
            &json!({"path":"a","content":"x".repeat(WRITE_LIMIT + 1)})
        )
        .is_err());
        let moved = changes("files_move", &json!({"from":"a","to":"b"})).unwrap();
        assert_eq!(moved[0]["what"], "a");
        assert_eq!(moved[0]["facts"], json!([["To", "b"]]));
        let one = changes("files_remove", &json!({"paths":["old.log"]})).unwrap();
        assert_eq!(one[0]["verb"], "Move to trash");
        assert_eq!(one[0]["what"], "old.log");
        let paths: Vec<String> = (0..12).map(|i| format!("f{i}")).collect();
        let many = changes("files_remove", &json!({"paths":paths})).unwrap();
        assert_eq!(many[0]["what"], "12 files or directories");
        assert_eq!(many[0]["facts"].as_array().unwrap().len(), 11);
        assert_eq!(many[0]["facts"][10], json!(["And", "2 more"]));
        assert!(changes("files_remove", &json!({"paths":[]})).is_err());
        assert!(changes("files_remove", &json!({"paths":"a"})).is_err());
        assert_eq!(
            changes("files_list", &json!({})).unwrap(),
            Vec::<Value>::new()
        );
    }

    #[test]
    fn the_session_script_carries_the_agent_whole() {
        let script = session_script();
        assert!(script.starts_with("#!/bin/bash\n"));
        assert!(script.contains("exec python3 - <<'OBLETH_CHAT_AGENT_"));
        assert!(script.contains(AGENT));
        assert!(script.contains("/api/cluster-sessions/bye"));
        assert!(!AGENT.contains("OBLETH_CHAT_AGENT_"));
    }

    /// Runs one operation of the agent as the command line does, under temporary
    /// roots, and returns its exit status and output.
    fn agent(roots: &str, op: &str, args: Value) -> (bool, String) {
        agent_with(roots, op, args, &[])
    }

    fn agent_with(roots: &str, op: &str, args: Value, env: &[(&str, &str)]) -> (bool, String) {
        let dir = std::env::temp_dir().join(format!("chat-agent-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("agent.py");
        std::fs::write(&script, AGENT).unwrap();
        let out = std::process::Command::new("python3")
            .arg("-I")
            .arg(&script)
            .arg("--op")
            .arg(op)
            .arg("--args")
            .arg(args.to_string())
            .env("OBLETH_CHAT_ROOTS", roots)
            .envs(env.iter().copied())
            .output()
            .unwrap();
        std::fs::remove_dir_all(&dir).ok();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim_end().to_string(),
        )
    }

    fn python3() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn standing(state: &str, node: &str, exit: Option<i64>) -> slurm::Standing {
        slurm::Standing {
            name: JOB_NAME.into(),
            state: state.into(),
            node: node.into(),
            exit,
            submitted: Some(chrono::Utc::now().timestamp()),
            ..Default::default()
        }
    }

    /// A job that ended before it reached Chat is explained by how it ended, the
    /// agent's own exit codes first; only what an administrator must fix says so.
    #[test]
    fn a_session_job_that_ended_early_is_explained() {
        let origin = Url::parse("https://chat.example.edu/").unwrap();
        let why = |s: slurm::Standing| ended_early(41, &s, &origin);
        assert_eq!(
            why(standing("FAILED", "c12", Some(3))),
            "The file session job 41 on c12 ended (FAILED) before it reached Chat: the node could not look up Chat's address, chat.example.edu. The session job connects straight to Chat, without a proxy, so the cluster's compute nodes must resolve chat.example.edu themselves and reach https://chat.example.edu. An administrator fixes that; then try again, and a new session starts."
        );
        let text = why(standing("FAILED", "c12", Some(4)));
        assert!(
            text.contains("could not connect to Chat at https://chat.example.edu, most likely"),
            "{text}"
        );
        assert!(why(standing("FAILED", "c12", Some(5)))
            .contains("turned the session away: Chat itself, or a proxy"));
        let text = why(standing("FAILED", "c12", Some(8)));
        assert!(
            text.contains("answered with an error")
                && text.ends_with("Try again; a new session starts."),
            "{text}"
        );
        assert!(why(standing("FAILED", "c12", Some(6)))
            .contains("python3 is not installed on the node"));
        assert!(why(standing("FAILED", "c12", Some(7))).contains("older than 3.6"));
        let text = why(standing("CANCELLED", "None", Some(0)));
        assert!(text.contains("job 41 ended (CANCELLED) before it reached Chat: it was cancelled, by a person or by Slurm. Try again"), "{text}");
        assert!(why(standing("OUT_OF_MEMORY", "c1", Some(0))).contains("ran out of memory"));
        assert!(why(standing("FAILED", "c1", Some(1))).contains("exited with code 1. Try again;"));
        assert!(!standing("FAILED", "", None).active());
        assert!(!standing("COMPLETED", "", None).active());
        assert!(standing("PENDING", "", None).active());
        assert!(standing("RUNNING", "c1", None).active());
        // Unknown is not taken as ended.
        assert!(standing("", "", None).active());
    }

    /// A job Slurm will never start is given up at once, with what to change; one
    /// that only waits its turn is not.
    #[test]
    fn a_session_job_held_for_good_is_given_up() {
        let origin = Url::parse("https://chat.example.edu").unwrap();
        let mut held = standing("PENDING", "", None);
        held.reason = "PartitionTimeLimit".into();
        assert!(held.held_for_good());
        let text = ended_early(9, &held, &origin);
        assert!(text.contains("cannot start with this cluster's file session settings: Slurm holds it (PartitionTimeLimit)"), "{text}");
        held.reason = "InvalidQOS".into();
        assert!(held.held_for_good());
        for reason in [
            "Priority",
            "Resources",
            "QOSMaxJobsPerUserLimit",
            "AssocGrpCpuLimit",
        ] {
            held.reason = reason.into();
            assert!(!held.held_for_good(), "{reason}");
        }
    }

    /// An old job that once had the same id, under another name or long ago, is not
    /// taken for the session's.
    #[test]
    fn only_the_session_job_is_taken_for_it() {
        assert!(is_session_job(&standing("FAILED", "c1", Some(3))));
        let mut other = standing("FAILED", "c1", Some(3));
        other.name = "train".into();
        assert!(!is_session_job(&other));
        let mut old = standing("FAILED", "c1", Some(3));
        old.submitted = Some(chrono::Utc::now().timestamp() - 86_400);
        assert!(!is_session_job(&old));
    }

    /// A start that runs out says whether the job was queued, and why, or running
    /// without reaching Chat.
    #[test]
    fn a_session_that_never_started_says_where_it_stood() {
        let mut queued = standing("PENDING", "", None);
        queued.reason = "Priority".into();
        let text = not_started(Some(9), Some(&queued));
        assert!(
            text.contains("job 9 did not start within 10 minutes"),
            "{text}"
        );
        assert!(text.contains("Slurm kept it waiting: Priority. The partition and QoS set for file sessions on this cluster are busy"), "{text}");
        queued.reason = "QOSMaxJobsPerUserLimit".into();
        let text = not_started(Some(9), Some(&queued));
        assert!(
            text.contains("If waiting does not clear that, an administrator changes"),
            "{text}"
        );
        let text = not_started(Some(9), Some(&standing("RUNNING", "c3", None)));
        assert!(
            text.contains("started on c3, but its program neither reached Chat nor ended"),
            "{text}"
        );
        assert!(not_started(None, None).contains("did not start within 10 minutes"));
    }

    /// Runs the agent as the job would, with only the job's own environment.
    fn agent_exit(url: &str) -> (Option<i32>, String) {
        let mut child = std::process::Command::new("python3")
            .arg("-")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("OBLETH_CHAT_URL", url)
            .env("OBLETH_CHAT_SESSION", "x")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(AGENT.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Answers one request with the status line given.
    fn answer_once(status: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                );
            }
        });
        url
    }

    /// The agent exits with its own code when Chat's name does not resolve, its
    /// address does not answer, or the answer is a refusal or a server error.
    #[test]
    fn the_agent_exits_with_why_it_could_not_reach_chat() {
        if !python3() {
            eprintln!("python3 is not installed; skipping");
            return;
        }
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        for (url, code) in [
            // .invalid never resolves (RFC 2606).
            ("http://chat.invalid".to_owned(), EXIT_UNRESOLVED),
            (format!("http://127.0.0.1:{closed}"), EXIT_UNREACHABLE),
            (answer_once("409 Conflict"), EXIT_REFUSED),
            (answer_once("503 Service Unavailable"), EXIT_SERVER),
        ] {
            let (exit, stderr) = agent_exit(&url);
            assert_eq!(exit, Some(code as i32), "{url}: {stderr}");
        }
    }

    /// The script's own exit codes match what Chat reads back.
    #[test]
    fn the_script_exits_with_its_own_codes() {
        let script = session_script();
        assert!(
            script.contains(&format!("exit {EXIT_NO_PYTHON}\n")),
            "{script}"
        );
        assert!(
            script.contains(&format!("exit {EXIT_OLD_PYTHON}\n")),
            "{script}"
        );
        assert!(
            script.contains("sys.exit(sys.version_info < (3, 6))"),
            "{script}"
        );
        for (name, code) in [
            ("EXIT_UNRESOLVED", EXIT_UNRESOLVED),
            ("EXIT_UNREACHABLE", EXIT_UNREACHABLE),
            ("EXIT_REFUSED", EXIT_REFUSED),
            ("EXIT_SERVER", EXIT_SERVER),
        ] {
            assert!(AGENT.contains(&format!("{name} = {code}\n")), "{name}");
        }
    }

    #[test]
    fn the_agent_lists_reads_finds_searches_and_measures_under_its_roots() {
        if !python3() {
            eprintln!("python3 is not installed; skipping");
            return;
        }
        let root = std::env::temp_dir().join(format!("chat-root-{}", Uuid::new_v4().simple()));
        let project = root.join("project");
        std::fs::create_dir_all(project.join("runs")).unwrap();
        std::fs::write(
            project.join("run.sh"),
            "#!/bin/bash\nsrun python train.py\n",
        )
        .unwrap();
        std::fs::write(
            project.join("runs/out.log"),
            "step 1\nerror: out of memory\nstep 2\n",
        )
        .unwrap();
        std::fs::write(project.join("runs/model.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(project.join(".hidden"), "x").unwrap();
        let roots = root.to_string_lossy().to_string();

        let (ok, out) = agent(&roots, "list", json!({"path": project.to_string_lossy()}));
        assert!(ok, "{out}");
        assert!(out.contains("(2 entries, hidden ones left out):"), "{out}");
        assert!(out.lines().nth(1).unwrap().starts_with("dir "), "{out}");
        assert!(out.contains("runs/") && out.contains("run.sh"), "{out}");
        let (_, out) = agent(
            &roots,
            "list",
            json!({"path": project.to_string_lossy(), "hidden": true}),
        );
        assert!(out.contains(".hidden"), "{out}");

        let (ok, out) = agent(
            &roots,
            "read",
            json!({"path": project.join("run.sh").to_string_lossy()}),
        );
        assert!(ok, "{out}");
        assert!(out.contains("srun python train.py"), "{out}");
        let (ok, out) = agent(
            &roots,
            "read",
            json!({"path": project.join("run.sh").to_string_lossy(), "offset": 12, "max_bytes": 4}),
        );
        assert!(ok, "{out}");
        assert!(
            out.contains("bytes 12–16; more follows at offset 16"),
            "{out}"
        );
        assert!(out.ends_with("srun"), "{out}");
        let (ok, out) = agent(
            &roots,
            "read",
            json!({"path": project.join("runs/model.bin").to_string_lossy()}),
        );
        assert!(ok && out.contains("binary file of 4 B"), "{out}");

        let (ok, out) = agent(
            &roots,
            "find",
            json!({"path": project.to_string_lossy(), "name": "*.log"}),
        );
        assert!(ok, "{out}");
        assert!(out.starts_with("1 match under"), "{out}");
        assert!(out.contains("runs/out.log"), "{out}");
        let (_, out) = agent(
            &roots,
            "find",
            json!({"path": project.to_string_lossy(), "larger_than_mb": 1}),
        );
        assert!(out.starts_with("0 matches"), "{out}");

        let (ok, out) = agent(
            &roots,
            "search",
            json!({"path": project.to_string_lossy(), "pattern": "out of (memory|time)"}),
        );
        assert!(ok, "{out}");
        assert!(out.starts_with("1 line matches in 3 files"), "{out}");
        assert!(out.contains("out.log:2: error: out of memory"), "{out}");
        let (ok, out) = agent(
            &roots,
            "search",
            json!({"path": project.to_string_lossy(), "pattern": "("}),
        );
        assert!(
            !ok && out.contains("not a valid regular expression"),
            "{out}"
        );

        let (ok, out) = agent(&roots, "usage", json!({"path": project.to_string_lossy()}));
        assert!(ok, "{out}");
        assert!(out.lines().nth(1).unwrap().ends_with("  runs/"), "{out}");
        // Without du, the walk measures the same tree, in file bytes.
        let (ok, walked) = agent_with(
            &roots,
            "usage",
            json!({"path": project.to_string_lossy()}),
            &[("OBLETH_CHAT_NO_DU", "1")],
        );
        assert!(ok, "{walked}");
        assert!(walked.contains(" in files:"), "{walked}");
        assert!(
            walked.lines().nth(1).unwrap().ends_with("  runs/"),
            "{walked}"
        );

        // Outside the roots, or missing, is refused in words.
        let (ok, out) = agent(&roots, "list", json!({"path": "/etc"}));
        assert!(
            !ok && out.contains("is not in a directory Chat may reach on this cluster"),
            "{out}"
        );
        // One directory to a line, and where they are set.
        assert!(out.contains("Chat may reach:\n  "), "{out}");
        assert!(out.ends_with("on the Clusters page."), "{out}");
        let (ok, out) = agent(
            &roots,
            "read",
            json!({"path": project.join("nothing").to_string_lossy()}),
        );
        assert!(!ok && out.starts_with("There is no "), "{out}");
        let (ok, out) = agent(&roots, "nothing", json!({}));
        assert!(
            !ok && out.contains("no file operation named nothing"),
            "{out}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_agent_writes_moves_and_trashes_without_deleting() {
        if !python3() {
            return;
        }
        let root = std::env::temp_dir().join(format!("chat-root-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        let roots = root.to_string_lossy().to_string();
        let file = root.join("new/dir/notes.md");
        let (ok, out) = agent(
            &roots,
            "write",
            json!({"path": file.to_string_lossy(), "content": "hello\n"}),
        );
        assert!(ok && out.starts_with("Wrote "), "{out}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
        let (ok, out) = agent(
            &roots,
            "write",
            json!({"path": file.to_string_lossy(), "content": "again\n"}),
        );
        assert!(!ok && out.contains("set overwrite"), "{out}");
        let (ok, out) = agent(
            &roots,
            "write",
            json!({"path": file.to_string_lossy(), "content": "again\n", "overwrite": true}),
        );
        assert!(ok && out.starts_with("Replaced "), "{out}");

        let moved = root.join("new/notes-old.md");
        let (ok, out) = agent(
            &roots,
            "move",
            json!({"from": file.to_string_lossy(), "to": moved.to_string_lossy()}),
        );
        assert!(ok, "{out}");
        assert!(moved.exists() && !file.exists());
        let (ok, out) = agent(
            &roots,
            "move",
            json!({"from": moved.to_string_lossy(), "to": root.join("new/dir").to_string_lossy()}),
        );
        assert!(ok && out.ends_with("new/dir/notes-old.md"), "{out}");

        // The trash lives in the home directory, which is a root here.
        let home = std::env::temp_dir().join(format!("chat-home-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let roots = format!("{},{}", home.to_string_lossy(), root.to_string_lossy());
        let target = root.join("new/dir/notes-old.md");
        let script = home.join("agent.py");
        std::fs::write(&script, AGENT).unwrap();
        let out = std::process::Command::new("python3")
            .arg("-I")
            .arg(&script)
            .arg("--op")
            .arg("remove")
            .arg("--args")
            .arg(json!({"paths":[target.to_string_lossy()]}).to_string())
            .env("OBLETH_CHAT_ROOTS", &roots)
            .env("HOME", &home)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{text}");
        assert!(!target.exists());
        assert!(text.contains("Moved 1 to the trash under"), "{text}");
        let trash = home.join(".chat-trash");
        let kept = walkdir(&trash);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].ends_with("new/dir/notes-old.md"), "{kept:?}");
        // A root itself is never moved away.
        let out = std::process::Command::new("python3")
            .arg("-I")
            .arg(&script)
            .arg("--op")
            .arg("remove")
            .arg("--args")
            .arg(json!({"paths":[root.to_string_lossy()]}).to_string())
            .env("OBLETH_CHAT_ROOTS", &roots)
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("is a root Chat may reach"));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&home).ok();
    }

    fn walkdir(dir: &std::path::Path) -> Vec<String> {
        let mut out = vec![];
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(walkdir(&path));
                } else {
                    out.push(path.to_string_lossy().to_string());
                }
            }
        }
        out
    }

    #[test]
    fn clipping_keeps_utf8_whole() {
        assert_eq!(clip("héllo", 100), "héllo");
        let cut = clip("héllo", 2);
        assert!(cut.starts_with("h… ("), "{cut}");
    }
}
