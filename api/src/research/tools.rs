//! The research tools a chat with a workspace gets inside a project, and the rules
//! it follows (adapted from alphaXiv's OpenResearch playbook, MIT).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{check, for_project, launch, paper, Research, GIT, MAX_RUNNING};
use crate::sandbox::{
    tools::{sh, Outcome},
    Sandbox, Sandboxes,
};

pub const NAMES: [&str; 8] = [
    "research_setup",
    "research_status",
    "experiment_create",
    "experiment_checkout",
    "experiment_run",
    "experiment_wait",
    "run_log",
    "read_paper",
];
pub fn is_tool(name: &str) -> bool {
    NAMES.contains(&name)
}

// The git steps, in the chat's project folder. Kept here so tests run them as-is.
/// Commits the code as it is as the baseline, on research/baseline.
pub(crate) const SETUP: &str = "if [ ! -d .git ]; then git init -q && printf 'node_modules/\\n.venv/\\n__pycache__/\\n.next/\\ndist/\\nbuild/\\n' >> .git/info/exclude || exit 1; fi\n\
        git add -A && { git diff --cached --quiet || git commit -q -m 'Research baseline'; }\n\
        git rev-parse --verify -q HEAD >/dev/null || git commit -q --allow-empty -m 'Research baseline'\n\
        git checkout -q -B research/baseline && git rev-parse --short=12 HEAD";
/// The checked-out branch, then a line when there are uncommitted changes.
pub(crate) const STATE: &str =
    "git rev-parse --abbrev-ref HEAD; git status --porcelain | head -n 1";
pub(crate) const COMMIT: &str = "git add -A && git commit -q -m \"$1\"";
/// Branches $1 from $2, bringing uncommitted changes along.
pub(crate) const BRANCH_CARRY: &str = "git stash push -q -u -m research-carry && git checkout -q -b \"$1\" \"$2\" && { git stash pop -q || { echo 'The carried changes conflict with the parent; resolve them in the files.'; exit 4; }; }";
pub(crate) const BRANCH: &str = "git checkout -q -b \"$1\" \"$2\"";
pub(crate) const CHECKOUT: &str = "git checkout -q \"$1\"";
/// Commits branch $1's work when it is checked out, then prints its commit.
/// Fetches page $1 into the research's paper folder unless it is there already, then
/// prints it. Exits 3 when it can't be fetched, with curl's reason.
pub(crate) const FETCH_PAGE: &str = "d=\"${OBLETH_RESEARCH_DIR:-/workspace/.obleth-research}/$OBLETH_SLUG/papers\"\n\
        f=\"$d/$(printf %s \"$1\" | sha256sum | cut -c1-16).html\"\n\
        if [ ! -s \"$f\" ]; then mkdir -p \"$d\" && curl -fsSL --proto '=http,https' --proto-redir '=http,https' --max-time 60 --max-filesize 16000000 -A 'obleth-chat read_paper' -o \"$f.part\" -- \"$1\" && mv \"$f.part\" \"$f\" || { rm -f \"$f.part\"; exit 3; }; fi\n\
        cat \"$f\"";
pub(crate) const COMMIT_FOR_RUN: &str = "if [ \"$(git rev-parse --abbrev-ref HEAD)\" = \"$1\" ] && [ -n \"$(git status --porcelain)\" ]; then git add -A && git commit -q -m \"$2\" || exit 1; fi\ngit rev-parse --short=12 \"$1\"";

const WAIT_DEFAULT: u64 = 600;
const WAIT_MAX: u64 = 1800;
/// How long the run check may take before a result is reported without it.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// How much of a run's change and output the check reads.
const CHECK_DIFF: usize = 16 * 1024;
const CHECK_LOG: usize = 8 * 1024;
/// Largest page read_paper reads, with room for the shell's own output.
const PAGE_LIMIT: usize = 16_100_000;
/// Siblings the placement check reads when an idea goes under the baseline.
const CHECK_SIBLINGS: usize = 12;

pub fn definitions() -> Vec<Value> {
    let node = |what: &str| json!({"type":"string","description":format!("{what}: an experiment id (the first 8 characters are enough) or \"baseline\"")});
    vec![
        json!({"type":"function","function":{"name":"research_setup","description":"Start research in this project, or change its goal. Records the goal, the metric the code prints and whether higher or lower is better, and the one run command every experiment uses. The workspace's current code becomes the baseline, on the branch research/baseline. The metric and run command cannot change once a run has finished.",
            "parameters":{"type":"object","properties":{
                "goal":{"type":"string","description":"What the research is after, in a sentence or two, with any limits"},
                "metric":{"type":"string","description":"The name the code prints the result under, as metric=value, such as decode_tok_s"},
                "direction":{"type":"string","enum":["max","min"],"description":"max when higher is better, min when lower is better"},
                "run_command":{"type":"string","description":"The shell command that runs an experiment from a clean checkout of its code and prints the metric, such as: python bench.py"}},
                "required":["goal","metric","direction","run_command"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"research_status","description":"The experiment tree: every node with its id, branch, whether it is frozen, and its runs with their results. The source of truth; read it at the start of each round.",
            "parameters":{"type":"object","properties":{},"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"experiment_create","description":"Create a child experiment of a node for one hypothesis: a new branch from the parent's code, checked out so you can edit it. Uncommitted changes come along.",
            "parameters":{"type":"object","properties":{
                "parent":node("The node this builds on"),
                "title":{"type":"string","description":"The idea in a few words, such as FP8 KV cache"},
                "description":{"type":"string","description":"The concrete change you will make on this node and why"},
                "builds_on":{"type":"string","description":"What the parent established that this idea keeps and builds on, such as: the 3e-4 learning rate that won the last round. Required unless the parent is the baseline."},
                "confirm":{"type":"boolean","description":"true only when a placement check doubted this parent and you are sure it is right"}},
                "required":["parent","title"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"experiment_checkout","description":"Check out a node's branch to read it, or to repair a node whose run failed.",
            "parameters":{"type":"object","properties":{"experiment":node("The node")},"required":["experiment"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"experiment_run","description":"Commit the node's changes and start a run of the run command on its code, in the background. Several nodes can run at once (up to three). Returns at once; then call experiment_wait.",
            "parameters":{"type":"object","properties":{"experiment":node("The node to run")},"required":["experiment"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"experiment_wait","description":"Wait until the next run finishes and get its results, compared with its parent and the best so far. Returns early on the first finish; call it again for the next one. Says drained when no runs are going.",
            "parameters":{"type":"object","properties":{"seconds":{"type":"integer","description":"How long to wait at most, up to 1800 (default 600)"}},"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"read_paper","description":"Read a paper or any web page with headings by section, such as an arXiv paper an idea comes from. The first read returns its outline and the sections that best answer what you are looking for; call again with sections to read others. Cite what you use by section, such as §3.2.",
            "parameters":{"type":"object","properties":{
                "url":{"type":"string","description":"The page: an arXiv link (abs, pdf or html) or any web page with headings"},
                "looking_for":{"type":"string","description":"What you want from it, such as: the learning-rate schedule and warmup they trained with"},
                "sections":{"type":"array","items":{"type":"string"},"description":"Sections to read in full, by id or number from the outline, such as [\"s7\", \"3.2\"]"}},
                "required":["url"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"run_log","description":"The last lines of a run's output.",
            "parameters":{"type":"object","properties":{
                "run":{"type":"integer","description":"The run number"},
                "lines":{"type":"integer","description":"How many lines from the end, up to 400 (default 80)"}},
                "required":["run"],"additionalProperties":false}}}),
    ]
}

/// Whether this chat may drive the project's research, and the research if it has any.
pub enum Standing {
    /// No research yet; this person owns the project and may start it here.
    Absent,
    Here(Research),
    /// Someone else's research, or one that runs in another chat.
    Elsewhere(String),
}

pub async fn standing(
    pool: &PgPool,
    user: Uuid,
    conversation: Uuid,
    project: Uuid,
) -> Option<Standing> {
    let research = for_project(pool, project).await.ok()?;
    match research {
        None => {
            let owns: bool = sqlx::query_scalar("SELECT owner_id=$1 FROM projects WHERE id=$2")
                .bind(user)
                .bind(project)
                .fetch_optional(pool)
                .await
                .ok()??;
            owns.then_some(Standing::Absent)
        }
        Some(r) if r.owner_id == user && r.conversation_id.is_none_or(|c| c == conversation) => {
            Some(Standing::Here(r))
        }
        Some(r) => {
            let title: Option<String> = match r.conversation_id {
                Some(c) => sqlx::query_scalar("SELECT title FROM conversations WHERE id=$1")
                    .bind(c)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            Some(Standing::Elsewhere(match title {
                Some(t) if r.owner_id == user => format!("This project's research runs in your chat “{t}”; continue it there."),
                _ => "This project's research is run by someone else; it is shown on the project's Research tab.".into(),
            }))
        }
    }
}

pub async fn prompt(pool: &PgPool, standing: &Standing, sandbox: &Sandbox) -> String {
    let research = match standing {
        Standing::Elsewhere(why) => return format!("# Research\n{why}"),
        Standing::Absent => return format!("# Research\nThis project can hold research: an experiment tree whose nodes are git branches of this workspace's code, each measured by running one fixed command. Only when the person wants something measured and improved (faster, more accurate, smaller), first agree on the goal, the metric the code will print, whether higher or lower is better, and the run command (ask with ask_user when unsure), make the code run once by hand and print the metric as name=value, then call research_setup. Shared data or environments go under {} with absolute paths.", sandbox.dir()),
        Standing::Here(r) => r,
    };
    let tree = tree_text(pool, research).await;
    let last: Option<Value> = sqlx::query_scalar("SELECT report_check FROM research WHERE id=$1")
        .bind(research.id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten();
    let last = last
        .as_ref()
        .and_then(super::report::prompt)
        .map(|text| format!("\n\n{text}"))
        .unwrap_or_default();
    format!(
        "# Research\nThis project's research runs in this workspace. Goal: {goal}\nMetric: {metric} ({better} is better), printed by the code as {metric}=<number>. Run command, the same on every node: `{command}`\n\n{tree}\n\n\
## Rules\n\
1. Never change a node once a run has answered it: it is frozen, and its branch is the code that result came from. A disappointing number is still a result. To try something else, create a child with experiment_create and change the child.\n\
2. The run command and environment are a fixed contract. Never vary them between nodes or pass settings through environment variables; put every knob in committed code or config files and vary those.\n\
3. Grow the tree downward. A round is a few sibling options of one decision under one parent (which learning rate? which cache type?). The next round branches from that round's winner, never from the baseline again. A long row of children under the baseline, or a chain of options that do not build on each other, is the wrong shape. Before making X a child of Y, name what Y established that X builds on, in builds_on; if you cannot, they are siblings.\n\
4. A run that crashes, runs out of memory or times out answered nothing: the node stays open. Check it out, fix it and run it again. After two repairs in a row that answer nothing, or the same failure on a second node, stop and ask the person.\n\
5. Each run starts from a clean checkout of the node's commit in its own folder, without untracked or ignored files such as downloaded data. The project's .venv and node_modules are linked into it: install packages in the project folder beforehand, not in the run command. Use absolute paths under {dir} for shared data such as model weights. Make the code print its settings and final metrics.\n\
6. The loop: read research_status, create one round's children, edit each on its branch, start their runs (up to {max} at once), then call experiment_wait. Each time a run finishes, read its result before anything else and decide: repair the node, run the next option, promote the winner (the next round branches from it), or stop. Stop when the goal is met or after about three runs in a row without a gain.\n\
7. Report only results you read from runs, citing them as \"run N\", and label inferences as such. When a round ends or you stop, give one line per node: what it tested, its status and its result.\n\
8. When an idea comes from a paper, read it with read_paper, saying what you are looking for, and take settings from the section that gives them, citing it (§3.2) in the node's description.{last}",
        goal = research.goal,
        metric = research.metric,
        better = if research.direction == "min" { "lower" } else { "higher" },
        command = research.run_command,
        dir = sandbox.dir(),
        max = MAX_RUNNING,
    )
}

struct Node {
    id: Uuid,
    parent: Option<Uuid>,
    title: String,
    branch: String,
    frozen: bool,
}

async fn nodes(pool: &PgPool, research: &Research) -> Vec<Node> {
    sqlx::query("SELECT id,parent_id,title,branch,frozen FROM experiments WHERE research_id=$1 ORDER BY created_at")
        .bind(research.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| Node { id: r.get("id"), parent: r.get("parent_id"), title: r.get("title"), branch: r.get("branch"), frozen: r.get("frozen") })
        .collect()
}

struct RunRow {
    number: i32,
    experiment: Uuid,
    status: String,
    metric: Option<f64>,
}

async fn runs(pool: &PgPool, research: &Research) -> Vec<RunRow> {
    sqlx::query("SELECT number,experiment_id,status,metric FROM research_runs WHERE research_id=$1 ORDER BY number")
        .bind(research.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| RunRow { number: r.get("number"), experiment: r.get("experiment_id"), status: r.get("status"), metric: r.get("metric") })
        .collect()
}

fn short(id: Uuid) -> String {
    id.simple().to_string()[..8].to_owned()
}
fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else if v.abs() >= 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

/// The best measured result for a node, and for the whole tree.
fn best_of(research: &Research, runs: &[RunRow], node: Option<Uuid>) -> Option<(f64, i32)> {
    runs.iter()
        .filter(|r| r.status == "completed" && node.is_none_or(|n| r.experiment == n))
        .filter_map(|r| r.metric.map(|m| (m, r.number)))
        .reduce(|a, b| if research.better(b.0, a.0) { b } else { a })
}

async fn tree_text(pool: &PgPool, research: &Research) -> String {
    let nodes = nodes(pool, research).await;
    let runs = runs(pool, research).await;
    if nodes.is_empty() {
        return "The tree is empty.".into();
    }
    let best = best_of(research, &runs, None);
    let mut out = String::from("## Experiment tree\n");
    fn walk(
        out: &mut String,
        nodes: &[Node],
        runs: &[RunRow],
        best: Option<(f64, i32)>,
        parent: Option<Uuid>,
        depth: usize,
    ) {
        for n in nodes.iter().filter(|n| n.parent == parent) {
            let mine: Vec<String> = runs
                .iter()
                .filter(|r| r.experiment == n.id)
                .map(|r| match (r.status.as_str(), r.metric) {
                    ("completed", Some(m)) => format!(
                        "run {}: {}{}",
                        r.number,
                        number(m),
                        if best.is_some_and(|b| b.1 == r.number) {
                            " (best)"
                        } else {
                            ""
                        }
                    ),
                    (status, _) => format!("run {}: {status}", r.number),
                })
                .collect();
            out.push_str(&format!(
                "{}- {} [{}] {} · {}{}\n",
                "  ".repeat(depth),
                n.title,
                if n.parent.is_none() {
                    "baseline".to_string()
                } else {
                    short(n.id)
                },
                n.branch,
                if n.frozen { "frozen" } else { "open" },
                if mine.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", mine.join(", "))
                }
            ));
            walk(out, nodes, runs, best, Some(n.id), depth + 1);
        }
    }
    walk(&mut out, &nodes, &runs, best, None, 0);
    out.trim_end().to_owned()
}

fn find<'a>(nodes: &'a [Node], key: &str) -> Result<&'a Node, String> {
    let key = key.trim().to_lowercase();
    if key == "baseline" || key == "root" {
        return nodes
            .iter()
            .find(|n| n.parent.is_none())
            .ok_or_else(|| "There is no baseline yet: call research_setup.".into());
    }
    let hex: String = key.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let matches: Vec<&Node> = if hex.len() >= 6 {
        nodes
            .iter()
            .filter(|n| n.id.simple().to_string().starts_with(&hex))
            .collect()
    } else {
        nodes
            .iter()
            .filter(|n| n.title.to_lowercase() == key || n.branch.to_lowercase() == key)
            .collect()
    };
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "No experiment matches “{key}”. Use an id from research_status."
        )),
        _ => Err(format!(
            "Several experiments match “{key}”; use more of the id."
        )),
    }
}

fn slug(title: &str) -> String {
    let mut s = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 32 {
            break;
        }
    }
    s.trim_matches('-').to_owned()
}

pub struct Ctx<'a> {
    pub pool: &'a PgPool,
    pub sb: &'a Sandboxes,
    pub sandbox: &'a Sandbox,
    pub user: Uuid,
    pub conversation: Uuid,
    pub project: Uuid,
    /// Asks decisions as the person; None skips the run check.
    pub ask: Option<crate::decisions::Asker<'a>>,
}

fn outcome(ok: bool, summary: impl Into<String>, output: impl Into<String>) -> Outcome {
    let output = output.into();
    Outcome {
        ok,
        summary: summary.into(),
        detail: if ok {
            String::new()
        } else {
            output.chars().take(300).collect()
        },
        output,
        preview: None,
        extra: json!({}),
    }
}

pub async fn run(ctx: &Ctx<'_>, name: &str, arguments: &str) -> Outcome {
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    let standing = standing(ctx.pool, ctx.user, ctx.conversation, ctx.project).await;
    let research = match (&standing, name) {
        (Some(Standing::Elsewhere(why)), _) => {
            return outcome(false, "Research is elsewhere", why.clone())
        }
        (None, _) => {
            return outcome(
                false,
                "No research here",
                "Only the project's owner can start research in it.",
            )
        }
        (Some(Standing::Absent), "research_setup") => None,
        (Some(Standing::Absent), _) => {
            return outcome(
                false,
                "No research yet",
                "There is no research in this project yet: call research_setup first.",
            )
        }
        (Some(Standing::Here(r)), _) => Some(r.clone()),
    };
    match name {
        "research_setup" => setup(ctx, research.as_ref(), &args).await,
        "research_status" => {
            let r = research.unwrap();
            outcome(
                true,
                "Read the experiment tree",
                tree_text(ctx.pool, &r).await,
            )
        }
        "experiment_create" => create(ctx, &research.unwrap(), &args).await,
        "experiment_checkout" => checkout(ctx, &research.unwrap(), &args).await,
        "experiment_run" => start(ctx, &research.unwrap(), &args).await,
        "experiment_wait" => wait(ctx, &research.unwrap(), &args).await,
        "run_log" => log(ctx, &research.unwrap(), &args).await,
        "read_paper" => read_paper(ctx, &research.unwrap(), &args).await,
        _ => outcome(
            false,
            "Unknown tool",
            format!("There is no tool named {name}."),
        ),
    }
}

async fn git(ctx: &Ctx<'_>, script: &str, args: &[&str]) -> Result<String, String> {
    match sh(
        ctx.sb,
        ctx.sandbox,
        &format!("{GIT}\n{script}"),
        args,
        None,
        Duration::from_secs(60),
    )
    .await
    {
        Ok((out, _, Some(0))) => Ok(out),
        Ok((out, err, _)) => Err(format!("{out}{err}").trim().chars().take(1500).collect()),
        Err(e) => Err(e),
    }
}

fn text(args: &Value, key: &str, max: usize) -> String {
    args[key]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(max)
        .collect()
}

async fn setup(ctx: &Ctx<'_>, existing: Option<&Research>, args: &Value) -> Outcome {
    let goal = text(args, "goal", 2000);
    let metric = text(args, "metric", 64);
    let direction = text(args, "direction", 3);
    let command = text(args, "run_command", 2000);
    let valid_metric = metric
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && metric
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if goal.is_empty()
        || !valid_metric
        || !matches!(direction.as_str(), "max" | "min")
        || command.is_empty()
    {
        return outcome(false, "Research not set up", "Pass a goal, a metric name of letters, digits and underscores, a direction of max or min, and a run command.");
    }
    if let Some(r) = existing {
        let finished: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM research_runs WHERE research_id=$1 AND status IN ('completed','failed')")
            .bind(r.id).fetch_one(ctx.pool).await.unwrap_or(0);
        if finished > 0
            && (r.metric != metric || r.direction != direction || r.run_command != command)
        {
            return outcome(false, "Research unchanged", format!("Runs have finished, so the metric ({}, {}) and run command (`{}`) are fixed. Only the goal can change.", r.metric, r.direction, r.run_command));
        }
        let _ = sqlx::query("UPDATE research SET goal=$2, metric=$3, direction=$4, run_command=$5, conversation_id=COALESCE(conversation_id,$6), updated_at=NOW() WHERE id=$1")
            .bind(r.id).bind(&goal).bind(&metric).bind(&direction).bind(&command).bind(ctx.conversation)
            .execute(ctx.pool).await;
        return outcome(
            true,
            "Updated the research",
            format!(
                "Updated. Goal: {goal}. Metric: {metric} ({direction}). Run command: `{command}`."
            ),
        );
    }
    let sha = match git(ctx, SETUP, &[]).await {
        Ok(out) => out.trim().to_owned(),
        Err(e) => {
            return outcome(
                false,
                "Research not set up",
                format!("The workspace's code could not be committed as the baseline: {e}"),
            )
        }
    };
    let id = Uuid::new_v4();
    let mut tx = match ctx.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return outcome(false, "Research not set up", "The database is unavailable."),
    };
    let inserted = sqlx::query("INSERT INTO research(id,project_id,owner_id,conversation_id,goal,metric,direction,run_command) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(ctx.project).bind(ctx.user).bind(ctx.conversation).bind(&goal).bind(&metric).bind(&direction).bind(&command)
        .execute(&mut *tx).await;
    let baseline = sqlx::query("INSERT INTO experiments(id,research_id,title,description,branch) VALUES($1,$2,'Baseline','The code as it was when research started.','research/baseline')")
        .bind(Uuid::new_v4()).bind(id).execute(&mut *tx).await;
    if inserted.is_err() || baseline.is_err() || tx.commit().await.is_err() {
        return outcome(
            false,
            "Research not set up",
            "The research could not be saved; it may have been started in another chat just now.",
        );
    }
    outcome(true, "Set up research", format!("Research is set up. The baseline is commit {sha} on research/baseline, checked out now. Next: run the baseline with experiment_run(\"baseline\") to establish the starting result."))
}

/// Before switching branches: commit work on an open node, or carry it along.
async fn settle(ctx: &Ctx<'_>, nodes: &[Node]) -> Result<(String, bool), String> {
    let state = git(ctx, STATE, &[]).await?;
    let mut lines = state.lines();
    let current = lines.next().unwrap_or("").trim().to_owned();
    let dirty = lines.next().is_some_and(|l| !l.trim().is_empty());
    if dirty {
        if let Some(n) = nodes.iter().find(|n| n.branch == current && !n.frozen) {
            git(ctx, COMMIT, &[&format!("Work on {}", n.title)]).await?;
            return Ok((current, false));
        }
    }
    Ok((current, dirty))
}

async fn create(ctx: &Ctx<'_>, research: &Research, args: &Value) -> Outcome {
    let nodes = nodes(ctx.pool, research).await;
    let parent = match find(&nodes, args["parent"].as_str().unwrap_or("")) {
        Ok(p) => p,
        Err(e) => return outcome(false, "No such parent", e),
    };
    let title = text(args, "title", 120);
    if title.is_empty() {
        return outcome(
            false,
            "No title",
            "Give the experiment a title: the idea in a few words.",
        );
    }
    let description = text(args, "description", 4000);
    let builds_on = text(args, "builds_on", 1000);
    let above = parent.parent.and_then(|g| nodes.iter().find(|n| n.id == g));
    if let Some(above) = above.filter(|_| builds_on.is_empty()) {
        return outcome(false, "Say what it builds on", format!("Say in builds_on what “{}” established that this idea keeps and builds on. If it builds on nothing “{}” established, it is an alternative to it and belongs beside it: make it a child of “{}” [{}].", parent.title, parent.title, above.title, label(above)));
    }
    let idea = Idea {
        title: &title,
        description: &description,
        builds_on: &builds_on,
    };
    let placement = place(ctx, research, &nodes, parent, &idea).await;
    let confirmed = args["confirm"].as_bool() == Some(true);
    if let Some((_, Some(doubt))) = placement.as_ref().filter(|_| !confirmed) {
        return outcome(false, "Placement doubted", doubt.clone());
    }
    let placement = placement.map(|(mut found, doubt)| {
        if doubt.is_some() {
            found["confirmed"] = json!(true);
        }
        found
    });
    let id = Uuid::new_v4();
    let branch = format!(
        "research/{}-{}",
        {
            let s = slug(&title);
            if s.is_empty() {
                "idea".into()
            } else {
                s
            }
        },
        &short(id)[..4]
    );
    let carry = match settle(ctx, &nodes).await {
        Ok((_, dirty)) => dirty,
        Err(e) => return outcome(false, "Could not switch branches", e),
    };
    let script = if carry { BRANCH_CARRY } else { BRANCH };
    if let Err(e) = git(ctx, script, &[&branch, &parent.branch]).await {
        return outcome(
            false,
            "Experiment not created",
            format!("The branch could not be created: {e}"),
        );
    }
    if sqlx::query("INSERT INTO experiments(id,research_id,parent_id,title,description,branch,builds_on,placement) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(research.id).bind(parent.id).bind(&title).bind(&description).bind(&branch).bind(&builds_on).bind(&placement)
        .execute(ctx.pool).await.is_err()
    {
        return outcome(false, "Experiment not created", "The experiment could not be saved.");
    }
    outcome(true, format!("Created “{title}”"), format!("Created “{title}” [{}] on branch {branch}, from “{}”, and checked it out{}. Make the change in its files, then experiment_run(\"{}\").", short(id), parent.title, if carry { ", bringing your uncommitted changes along" } else { "" }, short(id)))
}

fn label(node: &Node) -> String {
    if node.parent.is_none() {
        "baseline".into()
    } else {
        short(node.id)
    }
}

/// A new experiment as experiment_create describes it.
struct Idea<'a> {
    title: &'a str,
    description: &'a str,
    builds_on: &'a str,
}

/// A node's best result, as the placement check reads it.
fn result_of(research: &Research, runs: &[RunRow], node: Uuid) -> String {
    match best_of(research, runs, Some(node)) {
        Some((value, run)) => format!("{}={} (run {run})", research.metric, number(value)),
        None => "not measured yet".into(),
    }
}

/// Asks whether a new idea belongs under its parent, when the tree check is on. A child
/// of a node should build on what the node established; an idea under the baseline, once
/// another node beats it, should be another option of the round already there. Returns
/// what to record, and what to tell the model when the answer doubts the placement.
async fn place(
    ctx: &Ctx<'_>,
    research: &Research,
    nodes: &[Node],
    parent: &Node,
    idea: &Idea<'_>,
) -> Option<(Value, Option<String>)> {
    let ask = ctx.ask?;
    let settings = ask.settings().await?;
    if !settings.on() || !settings.check_tree {
        return None;
    }
    let runs = runs(ctx.pool, research).await;
    let new =
        json!({"title": idea.title, "description": idea.description, "builds_on": idea.builds_on});
    let (state, question, advice) = if parent.parent.is_none() {
        let (value, run) = best_of(research, &runs, None)?;
        let winner = runs.iter().find(|r| r.number == run)?.experiment;
        let winner = nodes
            .iter()
            .find(|n| n.id == winner && n.parent.is_some())?;
        let round: Vec<(Uuid, String, String)> = sqlx::query_as("SELECT id, title, description FROM experiments WHERE parent_id=$1 ORDER BY created_at LIMIT $2")
            .bind(parent.id)
            .bind(CHECK_SIBLINGS as i64)
            .fetch_all(ctx.pool)
            .await
            .ok()?;
        let state = json!({
            "goal": research.goal,
            "baseline": {"result": result_of(research, &runs, parent.id)},
            "round_under_baseline": round.iter().map(|(id, title, description)| json!({"title": title, "description": description, "result": result_of(research, &runs, *id)})).collect::<Vec<_>>(),
            "best_so_far": {"title": winner.title, "result": result_of(research, &runs, winner.id)},
            "new_experiment": new,
        });
        let question = json!({
            "type": "boolean",
            "instructions": "This research grows a tree of experiments. Under the baseline, one round tries options of one decision as siblings (which learning rate? which cache type?), and the next round branches from the round's winner, never from the baseline again. A new experiment is about to be added under the baseline. Is it another option of a decision the experiments in round_under_baseline already try?",
            "criteria": {
                "true": "Yes: it answers the same question as some of round_under_baseline, with a different choice.",
                "false": "No: it changes something none of them try, so it starts a new decision, which belongs under the best result so far."
            }
        });
        let advice = [
            "is another option of the round under the baseline".to_string(),
            format!("A new decision branches from the round's winner, never from the baseline again: “{}” [{}] has the best result so far (run {run}: {}).", winner.title, short(winner.id), number(value)),
            "is another option of that round".to_string(),
        ];
        (state, question, advice)
    } else {
        let (description, builds_on): (String, String) =
            sqlx::query_as("SELECT description, builds_on FROM experiments WHERE id=$1")
                .bind(parent.id)
                .fetch_one(ctx.pool)
                .await
                .ok()?;
        let above = nodes.iter().find(|n| Some(n.id) == parent.parent)?;
        let state = json!({
            "goal": research.goal,
            "parent": {"title": parent.title, "description": description, "builds_on": builds_on, "result": result_of(research, &runs, parent.id)},
            "new_experiment": new,
        });
        let question = json!({
            "type": "boolean",
            "instructions": "This research grows a tree of experiments, each a change to its parent's code. A child keeps what its parent established and changes one more thing; an alternative to the parent is its sibling. A new experiment is about to be made a child of parent. Does it build on what the parent established?",
            "criteria": {
                "true": "Yes: it keeps the parent's change and adds to it or refines it, and builds_on names something the parent's change established.",
                "false": "No: it doesn't need the parent's change, replaces or undoes it, or tries another option of the same decision; or builds_on names something the parent didn't establish."
            }
        });
        let advice = [
            format!("builds on “{}”", parent.title),
            format!("A child keeps what its parent established and changes one more thing; an alternative to “{}” belongs beside it, as a child of “{}” [{}].", parent.title, above.title, label(above)),
            format!("does build on “{}”", parent.title),
        ];
        (state, question, advice)
    };
    let found = ask
        .ask(&settings, state, json!({"fits": question}), CHECK_TIMEOUT)
        .await
        .and_then(|response| crate::decisions::yes_no(&response, "fits"));
    Some(match found {
        Ok(v) => {
            let who = if v.model.is_empty() {
                "A check".to_string()
            } else {
                format!("A check by {}", v.model)
            };
            let [claim, instead, sure] = &advice;
            let doubt = v.sure_no().then(|| format!("{who} doubts “{}” {claim}: it gives {:.0}% that it fits. {instead} Nothing was created. If it {sure}, call experiment_create again with the same arguments and confirm: true.", idea.title, v.yes * 100.0));
            (
                json!({"model": v.model, "fits": v.yes, "confidence": v.confidence}),
                doubt,
            )
        }
        Err(why) => (json!({"skipped": why}), None),
    })
}

/// The page at `url`, fetched in the workspace once and read from there after.
async fn fetch(ctx: &Ctx<'_>, url: &str) -> Result<String, String> {
    let script = crate::sandbox::tools::in_project(ctx.sb, ctx.sandbox, FETCH_PAGE);
    let out = tokio::time::timeout(
        Duration::from_secs(90),
        ctx.sb.exec_with(
            ctx.sandbox,
            &["sh", "-c", &script, "sh", url],
            None,
            PAGE_LIMIT,
        ),
    )
    .await
    .map_err(|_| "Fetching it took longer than 90 seconds.".to_string())?
    .map_err(|e| format!("The workspace could not fetch it: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    match out.exit_code {
        Some(0) if !out.truncated => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Some(0) => Err("The page is larger than 16 MB.".into()),
        _ if url.starts_with("https://arxiv.org/html/") && stderr.contains("404") => Err("arXiv has no HTML version of this paper; it makes one only when it can convert the paper's LaTeX. Download the PDF in the workspace with run_command and read it with a PDF-to-text tool instead.".into()),
        _ => Err(format!("It could not be fetched: {}", stderr.trim().chars().take(300).collect::<String>())),
    }
}

async fn read_paper(ctx: &Ctx<'_>, research: &Research, args: &Value) -> Outcome {
    let url = match paper::source(args["url"].as_str().unwrap_or("")) {
        Ok(url) => url,
        Err(e) => return outcome(false, "No paper link", e),
    };
    let html = match fetch(ctx, &url).await {
        Ok(html) => html,
        Err(e) => return outcome(false, "Paper not read", format!("{url}: {e}")),
    };
    let paper = paper::Paper::from_html(&html);
    if paper.sections.is_empty() {
        return outcome(false, "No sections", format!("{url} has no headings to read it by. Read its text with run_command and curl instead."));
    }
    let title = paper.title.clone().unwrap_or_else(|| url.clone());
    let wanted: Vec<String> = args["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_f64().map(|n| n.to_string()))
        })
        .take(8)
        .collect();
    let looking_for = text(args, "looking_for", 500);
    let mut picks: Vec<(usize, Option<f64>)> = vec![];
    let mut missing = vec![];
    let mut found = String::new();
    let mut note = String::new();
    if !wanted.is_empty() {
        for key in &wanted {
            match paper.find(key) {
                Some(i) if !picks.iter().any(|(p, _)| *p == i) => picks.push((i, None)),
                Some(_) => {}
                None => missing.push(key.as_str()),
            }
        }
    } else if !looking_for.is_empty() {
        let settings = match ctx.ask {
            Some(ask) => ask.settings().await.map(|s| (ask, s)),
            None => None,
        };
        match settings {
            Some((ask, settings)) if settings.on() && settings.pick_sections => {
                match paper::pick(ask, &settings, &paper, &looking_for, &research.goal).await {
                    Ok((model, ranked)) => {
                        picks = paper::top(&ranked).into_iter().map(|(i, p)| (i, Some(p))).collect();
                        found = format!(
                            " for “{looking_for}”{}",
                            if model.is_empty() { String::new() } else { format!(", picked by {model}") }
                        );
                    }
                    Err(why) => note = format!("\n\nSections weren't picked for you ({why}). Name the ones you want from the outline in sections."),
                }
            }
            _ => note = "\n\nName the sections you want from the outline in sections.".into(),
        }
    }
    if picks.is_empty() && missing.is_empty() {
        picks.push((paper.start(), None));
    }
    let shown = picks
        .iter()
        .map(|(i, p)| {
            format!(
                "### {} · {}{}\n{}",
                paper::id(*i),
                paper.heading(*i),
                p.map(|p| format!(" · {:.0}%", p * 100.0))
                    .unwrap_or_default(),
                paper.body(*i)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let outline = if wanted.is_empty() {
        format!("\n\n## Outline\n{}", paper.outline())
    } else {
        String::new()
    };
    if !missing.is_empty() {
        note.push_str(&format!(
            "\n\nNo section matches {}; use ids or numbers from the outline.",
            missing
                .iter()
                .map(|m| format!("“{m}”"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let count = paper.sections.len();
    let summary = match picks.len() {
        0 => format!("Read “{title}”"),
        1 => format!("Read “{title}” · {}", paper.heading(picks[0].0)),
        n => format!("Read {n} sections of “{title}”"),
    };
    outcome(
        !picks.is_empty(),
        summary,
        format!("“{title}” ({url}), {count} sections.{outline}\n\n## Sections{found}\n{shown}{note}\n\nRead others with read_paper and sections. Cite what you use by section, such as §3.2."),
    )
}

async fn checkout(ctx: &Ctx<'_>, research: &Research, args: &Value) -> Outcome {
    let nodes = nodes(ctx.pool, research).await;
    let node = match find(&nodes, args["experiment"].as_str().unwrap_or("")) {
        Ok(n) => n,
        Err(e) => return outcome(false, "No such experiment", e),
    };
    match settle(ctx, &nodes).await {
        Ok((current, true)) if current != node.branch => return outcome(false, "Uncommitted changes", format!("There are uncommitted changes on {current}, which is not an open experiment. Move them into a new child with experiment_create, or discard them, before switching.")),
        Err(e) => return outcome(false, "Could not switch branches", e),
        _ => {}
    }
    if let Err(e) = git(ctx, CHECKOUT, &[&node.branch]).await {
        return outcome(false, "Not checked out", e);
    }
    let note = if node.frozen {
        " It is frozen: read it, but put any change in a child."
    } else {
        ""
    };
    outcome(
        true,
        format!("Checked out “{}”", node.title),
        format!("Checked out “{}” on {}.{note}", node.title, node.branch),
    )
}

async fn start(ctx: &Ctx<'_>, research: &Research, args: &Value) -> Outcome {
    let nodes = nodes(ctx.pool, research).await;
    let node = match find(&nodes, args["experiment"].as_str().unwrap_or("")) {
        Ok(n) => n,
        Err(e) => return outcome(false, "No such experiment", e),
    };
    if node.frozen {
        return outcome(false, "Node is frozen", format!("“{}” is frozen: a run already answered it. Create a child with experiment_create for the next idea.", node.title));
    }
    let running: Vec<(i32, Uuid)> = sqlx::query_as(
        "SELECT number, experiment_id FROM research_runs WHERE research_id=$1 AND status='running'",
    )
    .bind(research.id)
    .fetch_all(ctx.pool)
    .await
    .unwrap_or_default();
    if running.iter().any(|r| r.1 == node.id) {
        return outcome(
            false,
            "Already running",
            format!(
                "“{}” is already running; wait for it with experiment_wait.",
                node.title
            ),
        );
    }
    if running.len() as i64 >= MAX_RUNNING {
        return outcome(false, "Too many runs", format!("{MAX_RUNNING} runs are going, the most at once. Call experiment_wait for one to finish."));
    }
    // Commit the node's work when it is checked out.
    let committed = git(
        ctx,
        COMMIT_FOR_RUN,
        &[&node.branch, &format!("Run {}", node.title)],
    )
    .await;
    let sha = match committed {
        Ok(out) => out.trim().to_owned(),
        Err(e) => {
            return outcome(
                false,
                "Not started",
                format!("The node's changes could not be committed: {e}"),
            )
        }
    };
    let parent = node
        .parent
        .and_then(|p| nodes.iter().find(|n| n.id == p))
        .map(|p| p.branch.as_str());
    match launch(
        ctx.pool,
        ctx.sb,
        ctx.sandbox,
        research,
        node.id,
        parent,
        &sha,
    )
    .await
    {
        Ok(l) => {
            let mut out = format!("Run {} started on “{}” at commit {sha}. Call experiment_wait to hear when it finishes.", l.number, node.title);
            if parent.is_some() {
                if let Ok(Some(stat)) = sqlx::query_scalar::<_, String>(
                    "SELECT diff_stat FROM research_runs WHERE id=$1",
                )
                .bind(l.id)
                .fetch_optional(ctx.pool)
                .await
                {
                    if stat.is_empty() {
                        out.push_str(" Note: its code is the same as its parent's, so this run tests nothing new unless the result varies between runs.");
                    } else {
                        out.push_str(&format!(" Change from parent: {stat}."));
                    }
                }
            }
            outcome(
                true,
                format!("Started run {} · “{}”", l.number, node.title),
                out,
            )
        }
        Err(e) => outcome(false, "Run did not start", e),
    }
}

/// Finished runs not yet told to the model, in the order they ran.
async fn report(ctx: &Ctx<'_>, research: &Research) -> Option<(String, String)> {
    let rows = sqlx::query("UPDATE research_runs SET reported=TRUE WHERE research_id=$1 AND status<>'running' AND NOT reported RETURNING id, number, experiment_id, status, exit_code, metric, metrics, log_tail, diff, started_at, finished_at")
        .bind(research.id).fetch_all(ctx.pool).await.ok()?;
    if rows.is_empty() {
        return None;
    }
    let nodes = nodes(ctx.pool, research).await;
    let all = runs(ctx.pool, research).await;
    let mut rows = rows;
    rows.sort_by_key(|r| r.get::<i32, _>("number"));
    let mut text = vec![];
    let mut summary = vec![];
    for r in &rows {
        let n: i32 = r.get("number");
        let node = nodes
            .iter()
            .find(|x| x.id == r.get::<Uuid, _>("experiment_id"));
        let title = node.map(|x| x.title.as_str()).unwrap_or("an experiment");
        let status: String = r.get("status");
        let minutes = r
            .get::<Option<chrono::DateTime<chrono::Utc>>, _>("finished_at")
            .map(|f| {
                (f - r.get::<chrono::DateTime<chrono::Utc>, _>("started_at")).num_seconds() as f64
                    / 60.0
            })
            .unwrap_or(0.0);
        let log: String = r.get("log_tail");
        let last: Vec<&str> = log
            .lines()
            .rev()
            .take(if status == "completed" { 12 } else { 30 })
            .collect();
        let last = last.into_iter().rev().collect::<Vec<_>>().join("\n");
        let mut line = format!("Run {n} · “{title}” · {status} after {minutes:.1} min");
        match (status.as_str(), r.get::<Option<f64>, _>("metric")) {
            ("completed", Some(m)) => {
                line.push_str(&format!(" · {}={}", research.metric, number(m)));
                let parent_best = node
                    .and_then(|x| x.parent)
                    .and_then(|p| best_of(research, &all, Some(p)));
                if let Some((pb, pn)) = parent_best {
                    let change = if pb != 0.0 {
                        (m - pb) / pb.abs() * 100.0
                    } else {
                        0.0
                    };
                    line.push_str(&format!(
                        " · {:+.1}% against its parent (run {pn}: {})",
                        change,
                        number(pb)
                    ));
                }
                if let Some((b, bn)) = best_of(research, &all, None) {
                    line.push_str(&if bn == n {
                        " · the best so far".to_string()
                    } else {
                        format!(" · best so far is run {bn}: {}", number(b))
                    });
                }
                line.push_str(". The node is now frozen.");
                // A baseline, or a node with its parent's code, has no change to check.
                let diff: String = r.get("diff");
                let doubt = match node.filter(|x| x.parent.is_some() && !diff.trim().is_empty()) {
                    Some(x) => check_run(ctx, research, r.get("id"), x, &diff, &log, m).await,
                    None => None,
                };
                summary.push(format!(
                    "run {n} · {}{}",
                    number(m),
                    if doubt.is_some() {
                        " · check doubts it"
                    } else {
                        ""
                    }
                ));
                if let Some(note) = doubt {
                    line.push_str(&note);
                }
            }
            ("completed", None) => {
                line.push_str(&format!(". It finished but did not print {}=<number>, so it answered nothing and the node stays open: make the code print the metric and run it again.", research.metric));
                summary.push(format!("run {n} · no metric"));
            }
            ("failed", _) => {
                line.push_str(&format!(
                    " with exit code {}. It answered nothing; the node stays open to repair.",
                    r.get::<Option<i32>, _>("exit_code").unwrap_or(-1)
                ));
                summary.push(format!("run {n} failed"));
            }
            _ => {
                line.push_str(". The node stays open.");
                summary.push(format!("run {n} {status}"));
            }
        }
        let metrics = r.get::<Value, _>("metrics");
        if let Some(m) = metrics.as_object().filter(|m| m.len() > 1) {
            line.push_str(&format!(
                "\nOther values it printed: {}",
                m.iter()
                    .filter(|(k, _)| **k != research.metric)
                    .take(12)
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !last.trim().is_empty() {
            line.push_str(&format!("\nLast lines of its output:\n{last}"));
        }
        text.push(line);
    }
    Some((text.join("\n\n"), summary.join(", ")))
}

/// Asks whether a finished run really tested its idea. A run can print the metric while
/// its change never took effect (an option not read, a default used, no steps taken), and
/// its number then says nothing about the idea. Records what the check found on the run,
/// or why there was no check, and returns a note for the model when it doubts the run.
/// The node stays frozen either way: the note says to read before trusting the number.
async fn check_run(
    ctx: &Ctx<'_>,
    research: &Research,
    run: Uuid,
    node: &Node,
    diff: &str,
    log: &str,
    value: f64,
) -> Option<String> {
    let ask = ctx.ask?;
    let settings = ask.settings().await?;
    if !settings.on() || !settings.check_runs {
        return None;
    }
    let description: String = sqlx::query_scalar("SELECT description FROM experiments WHERE id=$1")
        .bind(node.id)
        .fetch_one(ctx.pool)
        .await
        .unwrap_or_default();
    let state = json!({
        "goal": research.goal,
        "idea": {"title": node.title, "description": description},
        "change_from_parent": super::head(diff, CHECK_DIFF),
        "output": super::tail(log, CHECK_LOG),
        "result": format!("{}={}", research.metric, number(value)),
    });
    let questions = json!({"tested": {
        "type": "boolean",
        "instructions": "This is one run in a research project. It measured one idea: the change from its parent's code, described under idea and shown under change_from_parent. Judge from the change and the run's output whether the run really tested that idea.",
        "criteria": {
            "true": "Yes: nothing in the output contradicts that the change took effect, and where the output shows settings or behavior the change affects, they match the idea.",
            "false": "No: the output shows the change did not take effect or the run did no real work: it printed a default or the parent's value instead of the changed one, warned that an option was unknown or unused, never reached the changed code, or did nothing (zero steps, no data)."
        }
    }});
    let found = ask
        .ask(&settings, state, questions, CHECK_TIMEOUT)
        .await
        .and_then(|response| crate::decisions::yes_no(&response, "tested"));
    let (decision, note) = match found {
        Ok(v) => {
            let who = if v.model.is_empty() {
                "A check".to_string()
            } else {
                format!("A check by {}", v.model)
            };
            let note = (v.yes < 0.5).then(|| format!(
                "\n{who} doubts this run tested its idea: it gives {:.0}% that it did. Before trusting {}={}, read the output with run_log and compare it with the change. If the change did not take effect, the node is still frozen: put the fix in a child and run that.",
                v.yes * 100.0,
                research.metric,
                number(value)
            ));
            (
                json!({"model": v.model, "tested": v.yes, "confidence": v.confidence}),
                note,
            )
        }
        Err(why) => (json!({"skipped": why}), None),
    };
    let _ = sqlx::query("UPDATE research_runs SET decision=$2 WHERE id=$1")
        .bind(run)
        .bind(decision)
        .execute(ctx.pool)
        .await;
    note
}

async fn wait(ctx: &Ctx<'_>, research: &Research, args: &Value) -> Outcome {
    let seconds = args["seconds"]
        .as_u64()
        .unwrap_or(WAIT_DEFAULT)
        .clamp(10, WAIT_MAX);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some((text, summary)) = report(ctx, research).await {
            let still: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM research_runs WHERE research_id=$1 AND status='running'",
            )
            .bind(research.id)
            .fetch_one(ctx.pool)
            .await
            .unwrap_or(0);
            let tail = if still > 0 {
                format!("\n\nStill running: {still}. Decide what this result means, refill the free slot if the round has more options, then call experiment_wait again.")
            } else {
                "\n\nNo runs are going now.".to_string()
            };
            return outcome(
                true,
                format!("Finished: {summary}"),
                format!("{text}{tail}"),
            );
        }
        let running: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM research_runs WHERE research_id=$1 AND status='running'",
        )
        .bind(research.id)
        .fetch_all(ctx.pool)
        .await
        .unwrap_or_default();
        if running.is_empty() {
            return outcome(true, "No runs going", "drained: no runs in flight. Start runs with experiment_run, or stop and summarize.");
        }
        if Instant::now() >= deadline {
            let rows: Vec<(i32, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as("SELECT r.number, e.title, r.started_at FROM research_runs r JOIN experiments e ON e.id=r.experiment_id WHERE r.research_id=$1 AND r.status='running' ORDER BY r.number")
                .bind(research.id).fetch_all(ctx.pool).await.unwrap_or_default();
            let list = rows
                .iter()
                .map(|(n, t, s)| {
                    format!(
                        "run {n} “{t}” ({} min so far)",
                        (chrono::Utc::now() - *s).num_minutes()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return outcome(true, "Still running", format!("Nothing finished in {seconds} s. Still running: {list}. Call experiment_wait again, read progress with run_log, or end your reply and tell the person the runs are going; their results appear on the project's Research tab."));
        }
        for run in &running {
            check(ctx.pool, ctx.sb, *run).await;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn log(ctx: &Ctx<'_>, research: &Research, args: &Value) -> Outcome {
    let Some(n) = args["run"].as_i64() else {
        return outcome(false, "No run", "Pass the run number.");
    };
    let lines = args["lines"].as_u64().unwrap_or(80).clamp(1, 400) as usize;
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, status FROM research_runs WHERE research_id=$1 AND number=$2")
            .bind(research.id)
            .bind(n as i32)
            .fetch_optional(ctx.pool)
            .await
            .ok()
            .flatten();
    let Some((id, status)) = row else {
        return outcome(false, "No such run", format!("There is no run {n}."));
    };
    if status == "running" {
        check(ctx.pool, ctx.sb, id).await;
    }
    let log: String = sqlx::query_scalar("SELECT log_tail FROM research_runs WHERE id=$1")
        .bind(id)
        .fetch_one(ctx.pool)
        .await
        .unwrap_or_default();
    let last: Vec<&str> = log.lines().rev().take(lines).collect();
    let text = last.into_iter().rev().collect::<Vec<_>>().join("\n");
    outcome(
        true,
        format!("Read run {n}'s output"),
        if text.trim().is_empty() {
            format!("Run {n} ({status}) has printed nothing yet.")
        } else {
            format!("Run {n} ({status}), last {lines} lines:\n{text}")
        },
    )
}

/// One-line label for a research step before it finishes.
pub fn summary(name: &str, args: &Value) -> String {
    match name {
        "research_setup" => "Setting up research".into(),
        "research_status" => "Reading the experiment tree".into(),
        "experiment_create" => format!(
            "Creating “{}”",
            args["title"].as_str().unwrap_or("an experiment")
        ),
        "experiment_checkout" => "Checking out an experiment".into(),
        "experiment_run" => "Starting a run".into(),
        "experiment_wait" => "Waiting for a run to finish".into(),
        "run_log" => format!("Reading run {}'s output", args["run"].as_i64().unwrap_or(0)),
        "read_paper" => "Reading a paper".into(),
        _ => name.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the workspace scripts with a local sh and git, as the workspace would.
    fn local(
        dir: &std::path::Path,
        runs: &std::path::Path,
        script: &str,
        args: &[&str],
        stdin: Option<&str>,
    ) -> (String, i32) {
        use std::io::Write;
        let full = format!(
            "export OBLETH_SLUG=demo; mkdir -p {d} && cd {d} || exit 1\n{script}",
            d = dir.display()
        );
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&full)
            .arg("sh")
            .args(args)
            .env("OBLETH_RESEARCH_DIR", runs)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.unwrap_or("").as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            out.status.code().unwrap_or(-1),
        )
    }
    fn git_script(script: &str) -> String {
        format!("{GIT}\n{script}")
    }
    fn wait_exit(dir: &std::path::Path, runs: &std::path::Path, n: &str) -> (String, String) {
        for _ in 0..100 {
            let (out, _) = local(dir, runs, &super::super::poll_script(), &[n], None);
            let (state, log) = out.split_once('\n').unwrap_or((&out, ""));
            if state.starts_with("exit") || state == "lost" {
                return (state.to_owned(), log.to_owned());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("run {n} did not finish");
    }

    #[test]
    fn a_run_that_ends_between_the_poll_checks_is_not_lost() {
        let root = std::env::temp_dir().join(format!("research-{}", Uuid::new_v4().simple()));
        let (dir, runs) = (root.join("demo"), root.join("runs"));
        let run = runs.join("demo/runs/1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("pid"), "999999999").unwrap();
        // The run writes its exit file and ends just after the first look for it.
        let ends_now = format!(
            "kill() {{ echo 0 > \"$R/exit\"; return 1; }}\n{}",
            super::super::poll_script()
        );
        let (out, _) = local(&dir, &runs, &ends_now, &["1"], None);
        assert_eq!(out.lines().next(), Some("exit 0"));
        // With no exit file the process is really gone.
        std::fs::remove_file(run.join("exit")).unwrap();
        let (out, _) = local(&dir, &runs, &super::super::poll_script(), &["1"], None);
        assert_eq!(out.lines().next(), Some("lost"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_workspace_scripts_branch_run_and_cancel() {
        if std::process::Command::new("setsid")
            .arg("true")
            .status()
            .is_err()
        {
            return;
        }
        let root = std::env::temp_dir().join(format!("research-{}", Uuid::new_v4().simple()));
        let (dir, runs) = (root.join("demo"), root.join("runs"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.sh"), "BATCH=16\n").unwrap();
        std::fs::write(
            dir.join("run.sh"),
            ". ./config.sh\necho \"batch=$BATCH\"\necho \"score=$((BATCH * 10))\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("node_modules/big.js"), "x").unwrap();

        let (sha, code) = local(&dir, &runs, &git_script(SETUP), &[], None);
        assert_eq!(code, 0, "{sha}");
        let base = sha.trim().to_owned();
        assert_eq!(base.len(), 12);
        let (state, _) = local(&dir, &runs, &git_script(STATE), &[], None);
        assert_eq!(
            state.trim(),
            "research/baseline",
            "clean and on the baseline"
        );

        let (out, code) = local(
            &dir,
            &runs,
            &super::super::launch_script(),
            &["1", &base],
            Some("sh run.sh"),
        );
        assert_eq!((out.trim(), code), ("started", 0));
        let (state, log) = wait_exit(&dir, &runs, "1");
        assert_eq!(state, "exit 0", "{log}");
        assert_eq!(super::super::parse_metrics(&log)["score"], 160.0);
        assert!(
            runs.join("demo/runs/1/src/node_modules")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "the project's node_modules is linked into the run"
        );
        local(&dir, &runs, &super::super::cleanup_script(), &["1"], None);
        assert!(
            dir.join("node_modules/big.js").exists(),
            "removing the run leaves the project's node_modules alone"
        );
        assert!(
            !runs.join("demo/runs/1/src").exists(),
            "the run's checkout is removed"
        );
        assert!(runs.join("demo/runs/1/log").exists(), "its log stays");

        // An edit made on the frozen baseline is carried into a new child.
        std::fs::write(dir.join("config.sh"), "BATCH=32\n").unwrap();
        let (state, _) = local(&dir, &runs, &git_script(STATE), &[], None);
        assert_eq!(state.lines().count(), 2, "dirty: {state}");
        let (out, code) = local(
            &dir,
            &runs,
            &git_script(BRANCH_CARRY),
            &["research/batch-32-0001", "research/baseline"],
            None,
        );
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("config.sh")).unwrap(),
            "BATCH=32\n"
        );
        let (sha, code) = local(
            &dir,
            &runs,
            &git_script(COMMIT_FOR_RUN),
            &["research/batch-32-0001", "Run batch 32"],
            None,
        );
        assert_eq!(code, 0, "{sha}");
        let child = sha.trim().to_owned();
        assert_ne!(child, base);
        let (base_now, _) = local(
            &dir,
            &runs,
            &git_script("git rev-parse --short=12 research/baseline"),
            &[],
            None,
        );
        assert_eq!(base_now.trim(), base, "the baseline did not change");
        local(
            &dir,
            &runs,
            &super::super::launch_script(),
            &["2", &child],
            Some("sh run.sh"),
        );
        let (state, log) = wait_exit(&dir, &runs, "2");
        assert_eq!(state, "exit 0");
        assert_eq!(super::super::parse_metrics(&log)["score"], 320.0);

        // A long run stops when cancelled.
        local(
            &dir,
            &runs,
            &super::super::launch_script(),
            &["3", &child],
            Some("echo begun; sleep 30; echo score=1"),
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (out, _) = local(&dir, &runs, &super::super::poll_script(), &["3"], None);
        assert!(out.starts_with("running"), "{out}");
        local(&dir, &runs, &super::super::cancel_script(), &["3"], None);
        let (state, log) = wait_exit(&dir, &runs, "3");
        assert_ne!(state, "exit 0", "{state}");
        assert!(
            !log.contains("score=1"),
            "the run was stopped before it finished"
        );

        // Back to the baseline, clean.
        let (out, code) = local(
            &dir,
            &runs,
            &git_script(CHECKOUT),
            &["research/baseline"],
            None,
        );
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("config.sh")).unwrap(),
            "BATCH=16\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn names_branches_and_numbers() {
        assert_eq!(slug("FP8 KV cache, block 256!"), "fp8-kv-cache-block-256");
        assert_eq!(slug("   "), "");
        assert_eq!(number(1958.0), "1958");
        assert_eq!(number(1958.44), "1958.4");
        assert_eq!(number(0.12500), "0.125");
        let nodes = vec![
            Node {
                id: Uuid::parse_str("a41d6e2b-0000-4000-8000-000000000000").unwrap(),
                parent: None,
                title: "Baseline".into(),
                branch: "research/baseline".into(),
                frozen: true,
            },
            Node {
                id: Uuid::parse_str("a41d6e2c-0000-4000-8000-000000000000").unwrap(),
                parent: None,
                title: "x".into(),
                branch: "b".into(),
                frozen: false,
            },
        ];
        assert!(
            find(&nodes, "a41d6e").is_err(),
            "an ambiguous prefix is refused"
        );
        assert_eq!(find(&nodes, "a41d6e2c").unwrap().title, "x");
        assert_eq!(
            find(&nodes, "Baseline").unwrap().branch,
            "research/baseline"
        );
    }

    #[test]
    fn a_page_is_fetched_once_into_the_workspace() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = served.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = [0; 2048];
                let n = stream.read(&mut request).unwrap_or(0);
                let found = String::from_utf8_lossy(&request[..n]).starts_with("GET /paper ");
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (status, body) = if found {
                    ("200 OK", "<h1>Paper</h1><h2>1 Method</h2><p>Warmup.</p>")
                } else {
                    ("404 Not Found", "missing")
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let root = std::env::temp_dir().join(format!("research-{}", Uuid::new_v4().simple()));
        let (dir, runs) = (root.join("demo"), root.join("runs"));
        let url = format!("http://127.0.0.1:{port}/paper");
        let first = local(&dir, &runs, FETCH_PAGE, &[&url], None);
        assert_eq!(
            first,
            ("<h1>Paper</h1><h2>1 Method</h2><p>Warmup.</p>".into(), 0)
        );
        // Read again, it comes from the workspace.
        assert_eq!(local(&dir, &runs, FETCH_PAGE, &[&url], None), first);
        assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 1);
        let (out, code) = local(
            &dir,
            &runs,
            FETCH_PAGE,
            &[&format!("http://127.0.0.1:{port}/gone")],
            None,
        );
        assert_eq!(code, 3);
        assert!(out.contains("404"), "{out}");
        let (_, code) = local(&dir, &runs, FETCH_PAGE, &["file:///etc/hostname"], None);
        assert_eq!(code, 3);
        let papers = runs.join("demo/papers");
        assert_eq!(std::fs::read_dir(&papers).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
