//! Agents: the main model hands parts of its work to worker models, which run at the
//! same time and report back. Administrators choose which models may be workers, so
//! the fast, efficient ones do the legwork. A worker gets only its task (not the
//! conversation) and tools that read the chat's workspace, so several can explore a
//! codebase at once without changing it. Each worker is charged on its own usage
//! row, and workers cannot hand work on.

use crate::{
    auth::Identity,
    code::workspace::Shell,
    error::ApiError,
    gateway::{self, Decoder, Gateway},
    models::{self, ParameterOverrides},
    operations,
    sandbox::tools::Outcome,
    ToolCall,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use uuid::Uuid;

pub const TOOL: &str = "delegate";
pub const MAX_AGENTS: usize = 6;
const MAX_STEPS: usize = 30;
const STEP_TOKENS: i32 = 4096;
const TIMEOUT: Duration = Duration::from_secs(300);
/// What the main model reads of each report, and what the reply keeps to show.
const REPORT_CHARS: usize = 12_000;
const SHOWN_CHARS: usize = 4_000;
/// A worker's request stays under this; older tool output gives way first.
const KEPT_BYTES: usize = 160 * 1024;
const OUTPUT_BYTES: usize = 24 * 1024;
const RESPONSE_BYTES: usize = 256 * 1024;
/// Workers running on this replica at once, whatever turns they belong to.
static RUNNING: Semaphore = Semaphore::const_new(24);

const INSTRUCTIONS: &str = "You are a worker helping a lead model. The lead gave you one task, below; you do not see its conversation. Explore the project in the workspace with your tools: list_files, read_file, search and git_log. You cannot change anything. Be quick: search before you read, and read only the parts you need. When you know enough, reply with your report and no tool call. Lead with the answer, cite files as path:line, quote code only where it matters, and say what you could not find. Keep it under 600 words.";

/// The tool the main model calls, offered when the person may use a worker model.
pub fn definition(workers: &[(String, String)]) -> Value {
    let ids: Vec<&str> = workers.iter().map(|(id, _)| id.as_str()).collect();
    let names = workers
        .iter()
        .map(|(id, name)| {
            if id == name {
                id.clone()
            } else {
                format!("{id} ({name})")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    json!({"type":"function","function":{"name":TOOL,"description":format!("Hand parts of the work to faster worker models that run at the same time, each on its own task, and get their reports back. Workers can read the workspace (list, read and search files, git history) but cannot change anything, and they do not see this conversation: give each a self-contained task saying what to look for and what to report. Use it to explore a codebase or answer several independent questions at once, then do the deciding and writing yourself. Up to {MAX_AGENTS} tasks. Workers: {names}."),
        "parameters":{"type":"object","properties":{
            "tasks":{"type":"array","minItems":1,"maxItems":MAX_AGENTS,"items":{"type":"object","properties":{
                "title":{"type":"string","description":"A few words for the person, like \"Map the auth flow\"."},
                "task":{"type":"string","description":"The whole task: what to find, where to start if you know, and what the report should contain."}},
                "required":["title","task"],"additionalProperties":false}},
            "model":{"type":"string","enum":ids,"description":format!("The worker model. Defaults to {}.", ids.first().copied().unwrap_or(""))}},
        "required":["tasks"],"additionalProperties":false}}})
}

pub fn summary(args: &Value) -> String {
    let n = args["tasks"].as_array().map_or(0, Vec::len);
    format!("Sent {n} agent{}", if n == 1 { "" } else { "s" })
}

pub struct Ctx<'a, S: Shell> {
    pub pool: &'a PgPool,
    pub gateway: &'a Gateway,
    pub user: &'a Identity,
    pub conversation: Uuid,
    pub shell: &'a S,
    /// The worker models this person may use, by id and name.
    pub workers: &'a [(String, String)],
}

struct Task {
    title: String,
    task: String,
}

fn tasks(args: &Value) -> Result<Vec<Task>, String> {
    let list = args["tasks"]
        .as_array()
        .filter(|l| !l.is_empty())
        .ok_or("Give at least one task")?;
    if list.len() > MAX_AGENTS {
        return Err(format!("Give at most {MAX_AGENTS} tasks at once"));
    }
    list.iter()
        .map(|t| {
            let task = t["task"].as_str().unwrap_or("").trim();
            if task.is_empty() || task.len() > 8000 {
                return Err("Each task needs a description of up to 8,000 characters".into());
            }
            let title: String = t["title"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .take(80)
                .collect();
            Ok(Task {
                title: if title.is_empty() {
                    task.chars().take(60).collect()
                } else {
                    title
                },
                task: task.to_owned(),
            })
        })
        .collect()
}

/// The agents' progress as the reply shows it.
type Board = Arc<Mutex<Vec<Value>>>;

fn update(
    board: &Board,
    progress: &mpsc::UnboundedSender<Value>,
    i: usize,
    change: impl FnOnce(&mut Value),
) {
    let snapshot = {
        let mut agents = board.lock().unwrap();
        change(&mut agents[i]);
        Value::Array(agents.clone())
    };
    let _ = progress.send(snapshot);
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n[…cut]");
    out
}

/// Runs delegate: every task at once, each on its own worker. `progress` gets the
/// agents' state after each step, for the live card.
pub async fn run<S: Shell>(
    ctx: &Ctx<'_, S>,
    raw: &str,
    progress: mpsc::UnboundedSender<Value>,
) -> Outcome {
    let args: Value = serde_json::from_str(raw.trim()).unwrap_or(json!({}));
    let fail = |message: String| Outcome {
        ok: false,
        detail: message.clone(),
        summary: summary(&args),
        output: format!("Error: {message}"),
        preview: None,
        extra: json!({}),
    };
    let tasks = match tasks(&args) {
        Ok(tasks) => tasks,
        Err(e) => return fail(e),
    };
    let Some((model, name)) = (match args["model"].as_str() {
        Some(asked) => ctx.workers.iter().find(|(id, _)| id == asked),
        None => ctx.workers.first(),
    }) else {
        let ids: Vec<&str> = ctx.workers.iter().map(|(id, _)| id.as_str()).collect();
        return fail(format!(
            "Choose one of the worker models: {}",
            ids.join(", ")
        ));
    };
    let board: Board = Arc::new(Mutex::new(
        tasks
            .iter()
            .map(|t| json!({"title":t.title,"model":name,"status":"queued","steps":0,"doing":"Waiting to start"}))
            .collect(),
    ));
    let _ = progress.send(Value::Array(board.lock().unwrap().clone()));
    let results = futures_util::future::join_all(
        tasks
            .iter()
            .enumerate()
            .map(|(i, t)| worker(ctx, i, t, model, &board, &progress)),
    )
    .await;
    let mut output = format!(
        "{} worker{} ({name}) reported:\n",
        tasks.len(),
        if tasks.len() == 1 { "" } else { "s" }
    );
    let mut done = 0;
    for (i, (task, result)) in tasks.iter().zip(&results).enumerate() {
        match result {
            Ok(report) => {
                done += 1;
                output.push_str(&format!(
                    "\n## {}. {}\n{}\n",
                    i + 1,
                    task.title,
                    clip(report, REPORT_CHARS)
                ));
            }
            Err(e) => output.push_str(&format!(
                "\n## {}. {}\nThis worker failed: {e}\n",
                i + 1,
                task.title
            )),
        }
    }
    let agents = board.lock().unwrap().clone();
    let steps: i64 = agents.iter().filter_map(|a| a["steps"].as_i64()).sum();
    let summary = if done == tasks.len() {
        format!("{done} agent{} reported", if done == 1 { "" } else { "s" })
    } else {
        format!("{done} of {} agents reported", tasks.len())
    };
    Outcome {
        ok: done > 0,
        detail: format!("{summary} after {steps} steps on {name}"),
        summary,
        output,
        preview: None,
        extra: json!({"agents":agents,"workerModel":name}),
    }
}

async fn worker<S: Shell>(
    ctx: &Ctx<'_, S>,
    i: usize,
    task: &Task,
    model: &str,
    board: &Board,
    progress: &mpsc::UnboundedSender<Value>,
) -> Result<String, String> {
    let permit = RUNNING
        .acquire()
        .await
        .map_err(|_| "Workers are shutting down".to_string())?;
    update(board, progress, i, |a| {
        a["status"] = json!("running");
        a["doing"] = json!("Starting");
    });
    let mut usage = json!({});
    let generation = Uuid::new_v4();
    let result = match reserve(ctx, generation, model, task).await {
        Ok(resolved) => {
            let work = steps(ctx, i, task, &resolved, board, progress, &mut usage);
            let result = tokio::time::timeout(TIMEOUT, work)
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "It ran out of time after {} minutes",
                        TIMEOUT.as_secs() / 60
                    ))
                });
            settle(ctx.pool, generation, &usage, result.is_ok()).await;
            result
        }
        Err(e) => Err(e),
    };
    drop(permit);
    let tokens = usage["total_tokens"].as_i64();
    update(board, progress, i, |a| {
        a["tokens"] = json!(tokens);
        match &result {
            Ok(report) => {
                a["status"] = json!("completed");
                a["doing"] = json!("Reported");
                a["report"] = json!(clip(report, SHOWN_CHARS));
            }
            Err(e) => {
                a["status"] = json!("error");
                a["doing"] = json!(e);
            }
        }
    });
    result
}

/// Checks the person may use the model now and reserves the worker's usage row.
async fn reserve<S: Shell>(
    ctx: &Ctx<'_, S>,
    generation: Uuid,
    model: &str,
    task: &Task,
) -> Result<models::ResolvedRequest, String> {
    let resolved = models::resolve_request(
        ctx.pool,
        ctx.gateway,
        ctx.user,
        model,
        ParameterOverrides::default(),
    )
    .await
    .map_err(|e| format!("The worker model is not available: {}", e.1))?;
    if resolved.capabilities.supports_function_calling == Some(false) {
        return Err("The worker model cannot use tools".into());
    }
    let reserved: Result<(), ApiError> = async {
        let mut tx = ctx.pool.begin().await?;
        let enabled: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id=$1 AND enabled AND NOT password_change_required FOR UPDATE",
        )
        .bind(ctx.user.id)
        .fetch_optional(&mut *tx)
        .await?;
        if enabled.is_none() {
            return Err(ApiError::unauthorized());
        }
        operations::reserve_agent_in(
            &mut tx,
            ctx.user.id,
            generation,
            &resolved.model,
            STEP_TOKENS,
            INSTRUCTIONS.len() + task.task.len(),
            TIMEOUT.as_secs() as i64 + 60,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    reserved.map_err(|e| e.1)?;
    Ok(resolved)
}

async fn settle(pool: &PgPool, generation: Uuid, usage: &Value, ok: bool) {
    let saved: Result<(), ApiError> = async {
        let mut tx = pool.begin().await?;
        operations::finish_in(
            &mut tx,
            generation,
            Some(usage),
            if ok { "completed" } else { "error" },
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    if saved.is_err() {
        tracing::warn!("could not settle a worker's usage");
    }
}

fn add_usage(total: &mut Value, step: &Value) {
    for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
        if let Some(n) = step[key].as_u64().filter(|n| *n <= 1_000_000_000) {
            total[key] = json!(total[key].as_u64().unwrap_or(0) + n);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn steps<S: Shell>(
    ctx: &Ctx<'_, S>,
    i: usize,
    task: &Task,
    resolved: &models::ResolvedRequest,
    board: &Board,
    progress: &mpsc::UnboundedSender<Value>,
    usage: &mut Value,
) -> Result<String, String> {
    let mut messages = vec![
        json!({"role":"system","content":INSTRUCTIONS}),
        json!({"role":"user","content":task.task}),
    ];
    for step in 1..=MAX_STEPS {
        let last = step == MAX_STEPS;
        if last {
            messages.push(json!({"role":"user","content":"That was your last step. Write your report now from what you found."}));
        }
        let mut payload = json!({"model":resolved.model,"messages":messages,"max_tokens":STEP_TOKENS,
            "temperature":resolved.parameters.temperature,"top_p":resolved.parameters.top_p,
            "stream":true,"stream_options":{"include_usage":true}});
        if !last {
            payload["tools"] = tools();
        }
        if resolved.capabilities.supports_reasoning != Some(false) {
            payload["reasoning_effort"] = json!("low");
        }
        let (content, calls, reported) = request(ctx, payload).await?;
        if let Some(reported) = reported {
            add_usage(usage, &reported);
        }
        update(board, progress, i, |a| {
            a["steps"] = json!(step);
            a["tokens"] = usage["total_tokens"].clone();
        });
        if calls.is_empty() {
            let report = content.trim();
            return if report.is_empty() {
                Err("It ended without a report".into())
            } else {
                Ok(report.to_owned())
            };
        }
        messages.push(json!({"role":"assistant","content":if content.is_empty() { Value::Null } else { json!(content) },
            "tool_calls":calls.iter().map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}})).collect::<Vec<_>>()}));
        for call in &calls {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or(json!({}));
            update(board, progress, i, |a| {
                a["doing"] = json!(doing(&call.name, &args))
            });
            let output = tool(ctx.shell, &call.name, &args).await;
            messages.push(json!({"role":"tool","tool_call_id":call.id,"content":output}));
        }
        trim(&mut messages);
    }
    Err("It used all its steps without a report".into())
}

/// One request to the gateway: the streamed text, the tool calls and the usage.
async fn request<S: Shell>(
    ctx: &Ctx<'_, S>,
    payload: Value,
) -> Result<(String, Vec<ToolCall>, Option<Value>), String> {
    let response = ctx
        .gateway
        .chat(ctx.user, ctx.conversation, payload)
        .await
        .map_err(|e| format!("The worker model did not answer: {}", e.1))?;
    let mut stream = response.bytes_stream();
    let mut decoder = Decoder::default();
    let (mut content, mut calls, mut usage, mut bytes) = (String::new(), Vec::new(), None, 0usize);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "The worker's stream broke off".to_string())?;
        bytes += chunk.len();
        if bytes > RESPONSE_BYTES {
            return Err("The worker's answer was too long".into());
        }
        for event in decoder
            .push(&chunk)
            .map_err(|_| "The worker's stream was invalid".to_string())?
        {
            if event == "[DONE]" {
                return Ok((content, calls, usage));
            }
            let v: Value = serde_json::from_str(&event)
                .map_err(|_| "The worker's stream was invalid".to_string())?;
            if v.get("error").is_some() {
                return Err("The worker model reported an error".into());
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                usage = Some(u.clone());
            }
            let Some(choice) = v.pointer("/choices/0") else {
                continue;
            };
            if let Some(deltas) = choice
                .pointer("/delta/tool_calls")
                .and_then(Value::as_array)
            {
                crate::accumulate_tool_calls(&mut calls, deltas)?;
            }
            for (channel, delta) in gateway::text_deltas(choice) {
                if channel == "delta" {
                    content.push_str(delta);
                }
            }
        }
    }
    Err("The worker's stream ended early".into())
}

/// Past KEPT_BYTES, the oldest tool output gives way to a note.
fn trim(messages: &mut [Value]) {
    let size = |m: &[Value]| m.iter().map(|v| v.to_string().len()).sum::<usize>();
    let mut total = size(messages);
    for m in messages.iter_mut() {
        if total <= KEPT_BYTES {
            break;
        }
        if m["role"] == "tool" && m["content"].as_str().is_some_and(|c| c.len() > 200) {
            let before = m.to_string().len();
            m["content"] =
                json!("[Older output removed to save room. Run the tool again if you need it.]");
            total = total + m.to_string().len() - before;
        }
    }
}

/// The worker's tools: they only read.
fn tools() -> Value {
    json!([
        {"type":"function","function":{"name":"list_files","description":"List files and folders (with sizes), skipping .git, dependencies and build output.","parameters":{"type":"object","properties":{"path":{"type":"string","description":"Folder to list; defaults to the project root."},"depth":{"type":"integer","minimum":1,"maximum":6,"description":"How deep to go; default 3."}},"additionalProperties":false}}},
        {"type":"function","function":{"name":"read_file","description":"Read lines of a file, numbered. Up to 300 lines at a time.","parameters":{"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer","minimum":1,"description":"First line; default 1."},"end":{"type":"integer","minimum":1,"description":"Last line; default start + 299."}},"required":["path"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"search","description":"Search file contents. Returns path:line: text for each match, up to 150.","parameters":{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","description":"Folder or file to search; default the whole project."},"regex":{"type":"boolean","description":"Treat the pattern as an extended regular expression; default plain text."},"glob":{"type":"string","description":"Only files matching this name pattern, like *.rs"}},"required":["pattern"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"git_log","description":"Recent commits, newest first, for the project or one path.","parameters":{"type":"object","properties":{"path":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}}
    ])
}

fn doing(name: &str, args: &Value) -> String {
    let path = args["path"].as_str().unwrap_or("");
    let short: String = path.chars().take(80).collect();
    match name {
        "list_files" if path.is_empty() || path == "." => "Listing the project".into(),
        "list_files" => format!("Listing {short}"),
        "read_file" => format!("Reading {short}"),
        "search" => format!(
            "Searching for “{}”",
            args["pattern"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(60)
                .collect::<String>()
        ),
        "git_log" => "Reading the history".into(),
        other => format!("Asked for {}", other.chars().take(40).collect::<String>()),
    }
}

/// A path inside the project, or the project itself.
fn place(value: &Value) -> Result<String, String> {
    let path = value
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(".");
    let path = path.trim_start_matches("./");
    if path.is_empty() || path == "." {
        return Ok(".".into());
    }
    crate::code::check_path(path)
        .map_err(|_| format!("{path} is not a path inside the project"))?;
    Ok(path.trim_end_matches('/').to_owned())
}

const SKIP: &str = ".git node_modules .venv venv __pycache__ .next dist build target .cache .turbo";

const LIST: &str = r#"d=$1; [ -d "$d" ] || { echo "There is no folder $d."; exit 0; }
set -f; prune=""; for n in $OB_SKIP; do prune="$prune -o -name $n"; done; prune=${prune# -o }
find "$d" -mindepth 1 -maxdepth "$2" \( $prune \) -prune -o \( -type d -printf '%p/\n' -o -type f -printf '%p  %s bytes\n' \) 2>/dev/null | sed 's|^\./||' | sort | head -n 400"#;

const READ: &str = r#"f=$1; [ -f "$f" ] || { echo "There is no file $f."; exit 0; }
if [ -s "$f" ] && ! grep -qI . "$f"; then echo "$f is a binary file."; exit 0; fi
n=$(wc -l < "$f"); s=$2; e=$3
awk -v s="$s" -v e="$e" 'NR>=s && NR<=e { printf "%d\t%s\n", NR, substr($0, 1, 500) }' "$f"
[ "$e" -gt "$n" ] && e=$n; echo "[lines $s-$e of $n]""#;

const SEARCH: &str = r#"p=$1; d=$2; mode=$3; glob=$4
set -f; set -- -rnI; [ "$mode" = regex ] && set -- "$@" -E || set -- "$@" -F
for n in $OB_SKIP; do set -- "$@" --exclude-dir="$n"; done
[ -n "$glob" ] && set -- "$@" --include="$glob"
out=$(grep "$@" -e "$p" -- "$d" 2>/dev/null | sed 's|^\./||' | cut -c1-300 | head -n 150)
[ -n "$out" ] && printf '%s\n' "$out" || echo "No matches.""#;

const LOG: &str = r#"[ -d .git ] || { echo "This project is not a git repository."; exit 0; }
if [ -n "$2" ]; then git -c core.quotepath=off log --date=short --format='%h %ad %an  %s' -n "$1" -- "$2"
else git -c core.quotepath=off log --date=short --format='%h %ad %an  %s' -n "$1"; fi"#;

/// Runs one of the worker's tools; anything wrong reads as text for the model.
pub async fn tool(sh: &impl Shell, name: &str, args: &Value) -> String {
    let number =
        |key: &str, default: i64, max: i64| args[key].as_i64().unwrap_or(default).clamp(1, max);
    let call: Result<(&str, Vec<String>), String> = (|| {
        Ok(match name {
            "list_files" => (
                LIST,
                vec![place(&args["path"])?, number("depth", 3, 6).to_string()],
            ),
            "read_file" => {
                let path = place(&args["path"])?;
                if path == "." {
                    return Err("Name a file to read".into());
                }
                let start = number("start", 1, 10_000_000);
                let end = args["end"]
                    .as_i64()
                    .unwrap_or(start + 299)
                    .clamp(start, start + 299);
                (READ, vec![path, start.to_string(), end.to_string()])
            }
            "search" => {
                let pattern = args["pattern"].as_str().unwrap_or("");
                if pattern.is_empty() || pattern.len() > 500 || pattern.contains('\n') {
                    return Err("Give a one-line pattern of up to 500 characters".into());
                }
                let glob = args["glob"].as_str().unwrap_or("");
                if glob.len() > 100 || glob.contains('/') {
                    return Err("The glob is a file name pattern, like *.rs".into());
                }
                let mode = if args["regex"].as_bool() == Some(true) {
                    "regex"
                } else {
                    "text"
                };
                (
                    SEARCH,
                    vec![
                        pattern.into(),
                        place(&args["path"])?,
                        mode.into(),
                        glob.into(),
                    ],
                )
            }
            "git_log" => {
                let path = place(&args["path"])?;
                (
                    LOG,
                    vec![
                        number("limit", 20, 50).to_string(),
                        if path == "." { String::new() } else { path },
                    ],
                )
            }
            other => {
                return Err(format!(
                    "There is no tool named {other}. Use list_files, read_file, search or git_log."
                ))
            }
        })
    })();
    let (script, values) = match call {
        Ok(call) => call,
        Err(e) => return e,
    };
    let script = format!("OB_SKIP='{SKIP}'\n{script}");
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();
    match sh.exec(&script, &refs, None, OUTPUT_BYTES).await {
        Ok(raw) => {
            let mut text =
                String::from_utf8_lossy(&raw.stdout[..raw.stdout.len().min(OUTPUT_BYTES)])
                    .into_owned();
            if raw.truncated || raw.stdout.len() > OUTPUT_BYTES {
                text.push_str("\n[…output cut]");
            }
            if text.trim().is_empty() {
                text = if raw.stderr.trim().is_empty() {
                    "(nothing)".into()
                } else {
                    raw.stderr.chars().take(2000).collect()
                };
            }
            text
        }
        Err(e) => format!("The tool could not run: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_are_bounded() {
        assert!(tasks(&json!({"tasks":[]})).is_err());
        let many: Vec<Value> = (0..7)
            .map(|i| json!({"title":"t","task":format!("t{i}")}))
            .collect();
        assert!(tasks(&json!({ "tasks": many })).is_err());
        let one = tasks(&json!({"tasks":[{"task":"Find where jobs are scheduled"}]})).unwrap();
        assert_eq!(one[0].title, "Find where jobs are scheduled");
    }

    #[test]
    fn paths_stay_inside() {
        assert_eq!(place(&json!(null)).unwrap(), ".");
        assert_eq!(place(&json!("./src/")).unwrap(), "src");
        for bad in ["/etc", "../x", "a/../../b", "-rf"] {
            assert!(place(&json!(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn old_output_gives_way() {
        let big = "x".repeat(60 * 1024);
        let mut messages: Vec<Value> = (0..5)
            .map(|_| json!({"role":"tool","content":big}))
            .collect();
        trim(&mut messages);
        assert!(messages[0]["content"]
            .as_str()
            .unwrap()
            .starts_with("[Older"));
        assert_eq!(messages[4]["content"].as_str().unwrap().len(), big.len());
    }
}
