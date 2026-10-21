//! Research in a project: an experiment tree whose nodes are git branches in one
//! chat's workspace, and runs of the project's fixed command on a node's committed
//! code. A run starts in the background from a snapshot of its commit, so several
//! can run at once; a supervisor on each replica watches them, parses the metrics
//! they print and freezes a node once a run answers it.
//!
//! The rules the model follows come from alphaXiv's OpenResearch playbook (MIT).

pub mod tools;

use std::{collections::BTreeMap, time::Duration};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    error::ApiError,
    sandbox::{Sandbox, Sandboxes},
    AppState,
};

/// Runs of one research at a time; a workspace shares one pod's CPU and memory.
pub const MAX_RUNNING: i64 = 3;
const LOG_TAIL: usize = 32 * 1024;
const DIFF_LIMIT: usize = 48 * 1024;
const MAX_METRICS: usize = 40;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Research {
    pub id: Uuid,
    pub project_id: Uuid,
    pub owner_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub goal: String,
    pub metric: String,
    pub direction: String,
    pub run_command: String,
}

impl Research {
    pub fn better(&self, a: f64, b: f64) -> bool {
        if self.direction == "min" {
            a < b
        } else {
            a > b
        }
    }
}

pub async fn for_project(pool: &PgPool, project: Uuid) -> Result<Option<Research>, ApiError> {
    Ok(sqlx::query_as::<_, Research>("SELECT id,project_id,owner_id,conversation_id,goal,metric,direction,run_command FROM research WHERE project_id=$1")
        .bind(project)
        .fetch_optional(pool)
        .await?)
}

// ---- Scripts run in the workspace (in the chat's project folder) ----

/// Run folders live outside the project, like checkpoints, so downloads skip them.
const RUN_DIR: &str = "R=${OBLETH_RESEARCH_DIR:-/workspace/.obleth-research}/$OBLETH_SLUG/runs/$1";
pub(crate) const GIT: &str = "git() { command git -c user.name=obleth -c user.email=research@obleth.invalid -c core.quotepath=off -c advice.detachedHead=false \"$@\"; }";

pub(crate) fn launch_script() -> String {
    format!(
        "{RUN_DIR}\n{GIT}\nrm -rf \"$R\" && mkdir -p \"$R\" && cat > \"$R/cmd\" || exit 1\n\
         git worktree prune 2>/dev/null\n\
         git worktree add -q --detach -f \"$R/src\" \"$2\" > \"$R/log\" 2>&1 || {{ cat \"$R/log\"; exit 3; }}\n\
         nohup setsid sh -c 'echo $$ > \"$1/pid\"; cd \"$1/src\" && sh \"$1/cmd\" > \"$1/log\" 2>&1 < /dev/null; echo $? > \"$1/exit.tmp\" && mv \"$1/exit.tmp\" \"$1/exit\"' sh \"$R\" > /dev/null 2>&1 < /dev/null &\n\
         echo started"
    )
}
pub(crate) fn poll_script() -> String {
    format!(
        "{RUN_DIR}\n[ -d \"$R\" ] || {{ echo lost; exit 0; }}\n\
         if [ -f \"$R/exit\" ]; then echo \"exit $(cat \"$R/exit\")\"; \
         elif [ -f \"$R/pid\" ] && kill -0 \"$(cat \"$R/pid\")\" 2>/dev/null; then echo running; \
         elif [ -f \"$R/pid\" ]; then echo lost; else echo running; fi\n\
         tail -c {LOG_TAIL} \"$R/log\" 2>/dev/null"
    )
}
pub(crate) fn cancel_script() -> String {
    format!(
        "{RUN_DIR}\nif [ -f \"$R/pid\" ]; then P=$(cat \"$R/pid\"); env kill -TERM -- -\"$P\" 2>/dev/null; sleep 2; env kill -KILL -- -\"$P\" 2>/dev/null; fi\n\
         [ -f \"$R/exit\" ] || echo 143 > \"$R/exit\""
    )
}
pub(crate) fn cleanup_script() -> String {
    format!("{RUN_DIR}\n{GIT}\ngit worktree remove --force \"$R/src\" 2>/dev/null; rm -rf \"$R/src\"; git worktree prune 2>/dev/null; true")
}

/// The metrics a run printed: the last `name=value` or `name: value` for each name,
/// which also matches JSON lines such as `{"decode_tok_s": 1958.4}`.
pub fn parse_metrics(log: &str) -> BTreeMap<String, f64> {
    let mut found = BTreeMap::new();
    for line in log.lines() {
        let b = line.as_bytes();
        for i in 0..b.len() {
            if b[i] != b'=' && b[i] != b':' {
                continue;
            }
            let mut j = i;
            while j > 0 && b[j - 1] == b' ' {
                j -= 1;
            }
            if j > 0 && b[j - 1] == b'"' {
                j -= 1;
            }
            let end = j;
            while j > 0
                && (b[j - 1].is_ascii_alphanumeric() || b[j - 1] == b'_' || b[j - 1] == b'.')
            {
                j -= 1;
            }
            let key = &line[j..end];
            if key.is_empty()
                || key.len() > 64
                || !(key.as_bytes()[0].is_ascii_alphabetic() || key.as_bytes()[0] == b'_')
            {
                continue;
            }
            let mut k = i + 1;
            while k < b.len() && (b[k] == b' ' || b[k] == b'"') {
                k += 1;
            }
            let start = k;
            if k < b.len() && (b[k] == b'-' || b[k] == b'+') {
                k += 1;
            }
            let digits = k;
            while k < b.len() && (b[k].is_ascii_digit() || b[k] == b'.') {
                k += 1;
            }
            if k == digits || line[digits..k].matches('.').count() > 1 {
                continue;
            }
            if k < b.len() && (b[k] == b'e' || b[k] == b'E') {
                let mut m = k + 1;
                if m < b.len() && (b[m] == b'-' || b[m] == b'+') {
                    m += 1;
                }
                let exp = m;
                while m < b.len() && b[m].is_ascii_digit() {
                    m += 1;
                }
                if m > exp {
                    k = m;
                }
            }
            // A version such as 1.2.3 or a time such as 12:30 is not a value.
            if k < b.len() && (b[k].is_ascii_digit() || b[k] == b'.' || b[k] == b':') {
                continue;
            }
            if let Ok(value) = line[start..k].parse::<f64>() {
                if value.is_finite() && (found.len() < MAX_METRICS || found.contains_key(key)) {
                    found.insert(key.to_owned(), value);
                }
            }
        }
    }
    found
}

// ---- Runs ----

pub struct Launched {
    pub number: i32,
    pub id: Uuid,
}

/// Starts a run of `sha` for an experiment. The caller checked the node may run.
pub async fn launch(
    pool: &PgPool,
    sb: &Sandboxes,
    sandbox: &Sandbox,
    research: &Research,
    experiment: Uuid,
    parent_branch: Option<&str>,
    sha: &str,
) -> Result<Launched, String> {
    let (diff_stat, diff) = match parent_branch {
        Some(parent) => {
            let script = format!(
                "{GIT}\ngit diff --stat \"$1\" \"$2\" | tail -n 1; echo '---'; git diff \"$1\" \"$2\" | head -c {DIFF_LIMIT}"
            );
            match crate::sandbox::tools::sh(
                sb,
                sandbox,
                &script,
                &[parent, sha],
                None,
                Duration::from_secs(30),
            )
            .await
            {
                Ok((out, _, _)) => {
                    let (stat, diff) = out.split_once("---\n").unwrap_or((&out, ""));
                    (stat.trim().to_owned(), diff.to_owned())
                }
                Err(_) => (String::new(), String::new()),
            }
        }
        None => (String::new(), String::new()),
    };
    let id = Uuid::new_v4();
    let number: i32 = sqlx::query_scalar(
        "INSERT INTO research_runs(id,research_id,experiment_id,number,commit_sha,status,diff,diff_stat)
         SELECT $1,$2,$3,COALESCE(MAX(number),0)+1,$4,'running',$5,$6 FROM research_runs WHERE research_id=$2
         RETURNING number",
    )
    .bind(id)
    .bind(research.id)
    .bind(experiment)
    .bind(sha)
    .bind(&diff)
    .bind(&diff_stat)
    .fetch_one(pool)
    .await
    .map_err(|_| "The run could not be recorded".to_string())?;
    let n = number.to_string();
    let out = crate::sandbox::tools::sh(
        sb,
        sandbox,
        &launch_script(),
        &[&n, sha],
        Some(research.run_command.as_bytes()),
        Duration::from_secs(60),
    )
    .await;
    match out {
        Ok((stdout, _, Some(0))) if stdout.contains("started") => {
            sb.touch(sandbox).await;
            Ok(Launched { number, id })
        }
        other => {
            let why = match other {
                Ok((stdout, stderr, _)) => format!("{stdout}{stderr}"),
                Err(e) => e,
            };
            let why: String = why.trim().chars().take(1000).collect();
            let _ = sqlx::query("UPDATE research_runs SET status='failed',finished_at=NOW(),log_tail=$2,reported=TRUE WHERE id=$1")
                .bind(id)
                .bind(format!("The run could not start: {why}"))
                .execute(pool)
                .await;
            Err(format!("Run {number} could not start: {why}"))
        }
    }
}

/// Checks one running run in its workspace, and records how it ended once it has.
/// Returns whether it is still running. Replicas and waiting tools share the work
/// through `checked_at`, so a run is checked at most every few seconds.
pub async fn check(pool: &PgPool, sb: &Sandboxes, run: Uuid) -> bool {
    let claimed = sqlx::query(
        "UPDATE research_runs r SET checked_at=NOW() FROM research x
         WHERE r.id=$1 AND r.status='running' AND x.id=r.research_id
           AND (r.checked_at IS NULL OR r.checked_at < NOW() - INTERVAL '4 seconds')
         RETURNING r.number, r.experiment_id, x.owner_id, x.conversation_id, x.metric",
    )
    .bind(run)
    .fetch_optional(pool)
    .await;
    let row = match claimed {
        Ok(Some(row)) => row,
        // Not running any more, or someone else checked it just now.
        Ok(None) => {
            return sqlx::query_scalar::<_, bool>(
                "SELECT status='running' FROM research_runs WHERE id=$1",
            )
            .bind(run)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
        }
        Err(_) => return true,
    };
    let number: i32 = row.get("number");
    let experiment: Uuid = row.get("experiment_id");
    let owner: Uuid = row.get("owner_id");
    let metric: String = row.get("metric");
    let sandbox = match row.get::<Option<Uuid>, _>("conversation_id") {
        Some(conversation) => sb.get(owner, conversation).await.ok().flatten(),
        None => None,
    };
    let Some(sandbox) = sandbox else {
        finish(
            pool,
            run,
            experiment,
            "lost",
            None,
            "The workspace for this run was deleted.",
            &metric,
        )
        .await;
        return false;
    };
    if sandbox.status != "running" {
        finish(
            pool,
            run,
            experiment,
            "lost",
            None,
            "The workspace stopped while this run was going, so the run ended.",
            &metric,
        )
        .await;
        return false;
    }
    let n = number.to_string();
    let Ok((out, _, _)) = crate::sandbox::tools::sh(
        sb,
        &sandbox,
        &poll_script(),
        &[&n],
        None,
        Duration::from_secs(30),
    )
    .await
    else {
        return true;
    };
    // A running experiment keeps its workspace awake.
    sb.touch(&sandbox).await;
    let (state, log) = out.split_once('\n').unwrap_or((out.as_str(), ""));
    let log = tail(log, LOG_TAIL);
    match state.trim() {
        "running" => {
            let _ = sqlx::query("UPDATE research_runs SET log_tail=$2, metrics=$3 WHERE id=$1")
                .bind(run)
                .bind(&log)
                .bind(json!(parse_metrics(&log)))
                .execute(pool)
                .await;
            true
        }
        "lost" => {
            finish(pool, run, experiment, "lost", None, &format!("{log}\n[The run's process is gone without an exit code; the workspace may have restarted.]"), &metric).await;
            false
        }
        state => {
            let code = state
                .strip_prefix("exit ")
                .and_then(|c| c.trim().parse::<i32>().ok());
            let status = if code == Some(0) {
                "completed"
            } else {
                "failed"
            };
            finish(pool, run, experiment, status, code, &log, &metric).await;
            let _ = crate::sandbox::tools::sh(
                sb,
                &sandbox,
                &cleanup_script(),
                &[&n],
                None,
                Duration::from_secs(60),
            )
            .await;
            false
        }
    }
}

async fn finish(
    pool: &PgPool,
    run: Uuid,
    experiment: Uuid,
    status: &str,
    code: Option<i32>,
    log: &str,
    metric: &str,
) {
    let metrics = parse_metrics(log);
    let value = metrics.get(metric).copied();
    let _ = sqlx::query("UPDATE research_runs SET status=CASE WHEN status='cancelled' THEN status ELSE $2 END, exit_code=$3, log_tail=$4, metrics=$5, metric=$6, finished_at=NOW() WHERE id=$1")
        .bind(run)
        .bind(status)
        .bind(code)
        .bind(log)
        .bind(json!(metrics))
        .bind(value)
        .execute(pool)
        .await;
    // A run that measured the node's metric answered it; a crash leaves it open to repair.
    if status == "completed" && value.is_some() {
        let _ = sqlx::query("UPDATE experiments SET frozen=TRUE WHERE id=$1")
            .bind(experiment)
            .execute(pool)
            .await;
    }
}

fn tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

/// Stops a run. Returns false when it was not running.
pub async fn cancel(pool: &PgPool, sb: &Sandboxes, research: &Research, run: Uuid) -> bool {
    let number: Option<i32> = sqlx::query_scalar(
        "UPDATE research_runs SET status='cancelled', finished_at=NOW(), reported=TRUE WHERE id=$1 AND research_id=$2 AND status='running' RETURNING number",
    )
    .bind(run)
    .bind(research.id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some(number) = number else { return false };
    if let Some(conversation) = research.conversation_id {
        if let Ok(Some(sandbox)) = sb.get(research.owner_id, conversation).await {
            let n = number.to_string();
            let _ = crate::sandbox::tools::sh(
                sb,
                &sandbox,
                &cancel_script(),
                &[&n],
                None,
                Duration::from_secs(20),
            )
            .await;
            let _ = crate::sandbox::tools::sh(
                sb,
                &sandbox,
                &cleanup_script(),
                &[&n],
                None,
                Duration::from_secs(60),
            )
            .await;
        }
    }
    true
}

/// Watches running experiments on every replica; `check` keeps them from doubling up.
pub async fn worker(pool: PgPool, sb: Sandboxes, shutdown: CancellationToken) {
    loop {
        let due: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM research_runs WHERE status='running' AND (checked_at IS NULL OR checked_at < NOW() - INTERVAL '10 seconds') ORDER BY checked_at NULLS FIRST LIMIT 20")
            .fetch_all(&pool)
            .await
            .unwrap_or_default();
        for run in due {
            check(&pool, &sb, run).await;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
            _ = shutdown.cancelled() => break,
        }
    }
}

// ---- What the page and the model see ----

pub async fn state_json(pool: &PgPool, research: &Research) -> Result<Value, ApiError> {
    let experiments: Vec<Value> = sqlx::query("SELECT id,parent_id,title,description,branch,frozen,created_at FROM experiments WHERE research_id=$1 ORDER BY created_at")
        .bind(research.id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|r| json!({"id":r.get::<Uuid,_>("id"),"parentId":r.get::<Option<Uuid>,_>("parent_id"),"title":r.get::<String,_>("title"),"description":r.get::<String,_>("description"),"branch":r.get::<String,_>("branch"),"frozen":r.get::<bool,_>("frozen"),"createdAt":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at")}))
        .collect();
    let runs: Vec<Value> = sqlx::query("SELECT id,experiment_id,number,commit_sha,status,exit_code,metric,metrics,diff_stat,started_at,finished_at FROM research_runs WHERE research_id=$1 ORDER BY number")
        .bind(research.id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|r| json!({"id":r.get::<Uuid,_>("id"),"experimentId":r.get::<Uuid,_>("experiment_id"),"number":r.get::<i32,_>("number"),"commit":r.get::<String,_>("commit_sha"),"status":r.get::<String,_>("status"),"exitCode":r.get::<Option<i32>,_>("exit_code"),"metric":r.get::<Option<f64>,_>("metric"),"metrics":r.get::<Value,_>("metrics"),"diffStat":r.get::<String,_>("diff_stat"),"startedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("started_at"),"finishedAt":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("finished_at")}))
        .collect();
    let workspace: Option<(Uuid, String)> = match research.conversation_id {
        Some(c) => {
            sqlx::query_as("SELECT id,title FROM conversations WHERE id=$1")
                .bind(c)
                .fetch_optional(pool)
                .await?
        }
        None => None,
    };
    Ok(json!({
        "research": {"id":research.id,"goal":research.goal,"metric":research.metric,"direction":research.direction,"runCommand":research.run_command,"ownerId":research.owner_id},
        "workspace": workspace.map(|(id, title)| json!({"conversationId":id,"title":title})),
        "experiments": experiments,
        "runs": runs,
    }))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/projects/{id}/research", get(project_research))
        .route("/api/research/runs/{id}", get(run_detail))
        .route("/api/research/runs/{id}/cancel", post(cancel_run))
        .with_state(state)
}

async fn visible_project(pool: &PgPool, user: Uuid, project: Uuid) -> Result<(), ApiError> {
    let seen: Option<i32> = sqlx::query_scalar(&format!(
        "SELECT 1 FROM projects p WHERE p.id=$2 AND {}",
        crate::projects::VISIBLE
    ))
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?;
    seen.map(|_| ()).ok_or_else(ApiError::not_found)
}

async fn project_research(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    visible_project(&s.store.pool, user.id, project).await?;
    match for_project(&s.store.pool, project).await? {
        Some(research) => Ok(Json(state_json(&s.store.pool, &research).await?)),
        None => Ok(Json(json!({"research":null}))),
    }
}

async fn run_of(
    pool: &PgPool,
    user: Uuid,
    run: Uuid,
) -> Result<(Research, sqlx::postgres::PgRow), ApiError> {
    let row = sqlx::query("SELECT r.*, x.project_id FROM research_runs r JOIN research x ON x.id=r.research_id WHERE r.id=$1")
        .bind(run)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let project: Uuid = row.get("project_id");
    visible_project(pool, user, project).await?;
    let research = for_project(pool, project)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((research, row))
}

async fn run_detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(run): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let (research, r) = run_of(&s.store.pool, user.id, run).await?;
    Ok(Json(json!({"run":{
        "id":r.get::<Uuid,_>("id"),"experimentId":r.get::<Uuid,_>("experiment_id"),"number":r.get::<i32,_>("number"),"commit":r.get::<String,_>("commit_sha"),"status":r.get::<String,_>("status"),"exitCode":r.get::<Option<i32>,_>("exit_code"),
        "metric":r.get::<Option<f64>,_>("metric"),"metrics":r.get::<Value,_>("metrics"),"log":r.get::<String,_>("log_tail"),"diff":r.get::<String,_>("diff"),"diffStat":r.get::<String,_>("diff_stat"),
        "startedAt":r.get::<chrono::DateTime<chrono::Utc>,_>("started_at"),"finishedAt":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("finished_at"),
        "canCancel": research.owner_id == user.id && r.get::<String,_>("status") == "running",
    }})))
}

async fn cancel_run(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(run): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let (research, _) = run_of(&s.store.pool, user.id, run).await?;
    if research.owner_id != user.id {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the person running this research can stop its runs".into(),
        ));
    }
    let sb = s.sandboxes.as_ref().ok_or_else(ApiError::not_found)?;
    if cancel(&s.store.pool, sb, &research, run).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(
            StatusCode::CONFLICT,
            "This run is not running".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_metrics;

    #[test]
    fn reads_the_last_value_of_each_metric() {
        let log = "2026-10-20 12:30:45 INFO: starting\nstep=1 loss=2.5\nstep=2 loss=1.25e-1\n\
                   {\"decode_tok_s\": 1958.4, \"p95_s\": 1.71}\nversion=1.2.3 name=bench\nthroughput: 12 tok/s";
        let m = parse_metrics(log);
        assert_eq!(m["step"], 2.0);
        assert_eq!(m["loss"], 0.125);
        assert_eq!(m["decode_tok_s"], 1958.4);
        assert_eq!(m["p95_s"], 1.71);
        assert_eq!(m["throughput"], 12.0);
        assert!(!m.contains_key("version"), "versions are not values");
        assert!(!m.contains_key("name"));
        assert!(!m.contains_key("INFO"));
        assert!(!m.contains_key("12"), "times are not metrics");
    }
}
