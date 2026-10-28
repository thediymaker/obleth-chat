//! Slurm through slurmrestd with a person's own JWT, so jobs run as their account.
//! Chat uses the API version set on the cluster, or finds the newest one the server
//! offers (v0.0.45 to v0.0.40). It never lists every job on the cluster, which on a
//! large cluster is hundreds of megabytes: a person's jobs come from the accounting
//! database filtered by user, and the queue's size from the scheduler's statistics.
use super::mcp::Tool;
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

const VERSIONS: [&str; 6] = [
    "v0.0.45", "v0.0.44", "v0.0.43", "v0.0.42", "v0.0.41", "v0.0.40",
];
const TIMEOUT: Duration = Duration::from_secs(60);
/// The most Chat reads of one slurmrestd answer.
const MAX_BODY: usize = 32 * 1024 * 1024;

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

/// The cluster's tools. `output` adds Job output, for clusters that bring job output
/// back to Chat.
pub fn tools(output: bool) -> Vec<Tool> {
    let mut tools = vec![
        tool("cluster", "Cluster", "Partitions and nodes of the Slurm cluster: state, CPUs and memory in use, and time limits.", true, json!({}), &[]),
        tool("jobs", "Jobs", "Your running and pending Slurm jobs. For one job, including a finished one, use Job details.", true, json!({"state":{"type":"string","enum":["RUNNING","PENDING"],"description":"Only jobs in this state"}}), &[]),
        tool("job", "Job details", "Details of one job: state, reason, resources, working directory and output files.", true, json!({"job_id":{"type":"integer"}}), &["job_id"]),
        tool("submit", "Submit job", "Submit a batch script as your account. Output goes to slurm-<job id>.out in the working directory unless output is set.", false, json!({
            "script":{"type":"string","description":"The batch script, starting with #!/bin/bash. #SBATCH lines are not read; use the other fields."},
            "name":{"type":"string"},
            "partition":{"type":"string","description":"Defaults to the cluster's default partition"},
            "time_limit_minutes":{"type":"integer","minimum":1},
            "cpus":{"type":"integer","minimum":1,"description":"CPUs per task"},
            "tasks":{"type":"integer","minimum":1},
            "nodes":{"type":"integer","minimum":1},
            "memory_mb":{"type":"integer","minimum":1,"description":"Memory per node"},
            "working_directory":{"type":"string","description":"Defaults to /home/<user>"},
            "output":{"type":"string","description":"Output file path, may use %j for the job id"}
        }), &["script"]),
        tool("cancel", "Cancel job", "Cancel one of your jobs.", false, json!({"job_id":{"type":"integer"}}), &["job_id"]),
    ];
    if output {
        tools.push(tool("output", "Job output", "The end of what a job you submitted from this chat printed (its output file, up to the last 1 MiB), once the job has ended. Use it instead of guessing from the state or exit code.", true, json!({"job_id":{"type":"integer"}}), &["job_id"]));
    }
    tools
}

pub struct Client {
    http: reqwest::Client,
    base: Url,
    user: String,
    token: String,
    version: Option<String>,
    /// Set on the cluster, so Chat uses it rather than looking for one.
    pinned: bool,
}

/// An API version as slurmrestd names it, such as v0.0.45.
pub fn valid_version(version: &str) -> bool {
    version
        .strip_prefix("v0.0.")
        .is_some_and(|n| (2..=3).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A Slurm number: plain, or `{"set":true,"number":n}`; None when unset or infinite.
fn num(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::Object(o)
            if o.get("set") != Some(&json!(false)) && o.get("infinite") != Some(&json!(true)) =>
        {
            o.get("number").and_then(Value::as_i64)
        }
        _ => None,
    }
}
fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(","),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
fn minutes(value: &Value) -> String {
    match num(value) {
        Some(m) if m >= 60 => format!("{}h{:02}m", m / 60, m % 60),
        Some(m) => format!("{m}m"),
        None if value.get("infinite") == Some(&json!(true)) => "unlimited".into(),
        None => "-".into(),
    }
}
fn duration(seconds: i64) -> String {
    let (h, m, s) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else {
        format!("{m}m{s:02}s")
    }
}

impl Client {
    pub fn new(
        http: reqwest::Client,
        base: Url,
        user: String,
        token: String,
        version: Option<String>,
    ) -> Self {
        let mut base = base;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Self {
            http,
            base,
            user,
            token,
            pinned: version.is_some(),
            version,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        let url = self
            .base
            .join(path)
            .map_err(|_| "Invalid Slurm path".to_string())?;
        let mut request = self
            .http
            .request(method, url)
            .timeout(TIMEOUT)
            .header("X-SLURM-USER-NAME", &self.user)
            .header("X-SLURM-USER-TOKEN", &self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                "slurmrestd did not answer in time".to_string()
            } else {
                "Could not reach slurmrestd".to_string()
            }
        })?;
        let status = response.status().as_u16();
        let bytes = super::read_capped(response, MAX_BODY)
            .await
            .map_err(|e| match e {
                super::Read::Closed => "slurmrestd closed the connection".to_string(),
                super::Read::TooLarge => format!(
                    "slurmrestd's answer was larger than {} MiB, so Chat did not read it",
                    MAX_BODY / 1024 / 1024
                ),
            })?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }

    async fn version(&mut self) -> Result<String, String> {
        if let Some(v) = &self.version {
            return Ok(v.clone());
        }
        for v in VERSIONS {
            if let Ok((200, _)) = self
                .send(reqwest::Method::GET, &format!("slurm/{v}/ping"), None)
                .await
            {
                self.version = Some(v.into());
                return Ok(v.into());
            }
        }
        Err(not_offered())
    }

    /// Calls the scheduler (slurmctld) part of the API.
    async fn call_api(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        self.request(method, "slurm", path, body).await
    }

    /// Calls the accounting (slurmdbd) part of the API.
    async fn call_db(&mut self, path: &str) -> Result<Value, String> {
        self.request(reqwest::Method::GET, "slurmdb", path, None)
            .await
    }

    async fn request(
        &mut self,
        method: reqwest::Method,
        api: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let v = self.version().await?;
        let (status, value) = self
            .send(method, &format!("{api}/{v}/{path}"), body)
            .await?;
        let errors: Vec<String> = value
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|e| {
                text(
                    e.get("description")
                        .filter(|d| !d.as_str().unwrap_or("").is_empty())
                        .or(e.get("error"))
                        .unwrap_or(&Value::Null),
                )
            })
            .filter(|e| !e.is_empty())
            .collect();
        match status {
            401 | 403 | 511 => Err(format!("Slurm refused the token for {} (HTTP {status}). Create a new one with `scontrol token`.", self.user)),
            200..=299 if errors.is_empty() => Ok(value),
            _ if !errors.is_empty() => Err(format!("Slurm: {}", errors.join("; "))),
            404 if self.pinned => Err(format!("This server does not offer the Slurm REST API {v}. Change the version on the cluster, or leave it empty to find the newest one.")),
            _ => Err(format!("slurmrestd answered HTTP {status}")),
        }
    }

    /// Whether slurmrestd answers at all, for a cluster shared before anyone has a token.
    pub async fn reachable(&self) -> Result<(), String> {
        let versions = match (&self.version, self.pinned) {
            (Some(v), true) => vec![v.as_str()],
            _ => VERSIONS.to_vec(),
        };
        for v in &versions {
            match self
                .send(reqwest::Method::GET, &format!("slurm/{v}/ping"), None)
                .await?
                .0
            {
                200 | 401 | 403 | 511 => return Ok(()),
                404 => continue,
                status => {
                    return Err(format!(
                        "This does not look like slurmrestd (HTTP {status})"
                    ))
                }
            }
        }
        Err(if self.pinned {
            format!(
                "This server does not offer the Slurm REST API {}",
                versions[0]
            )
        } else {
            not_offered()
        })
    }

    /// Checks the token by listing partitions, which slurmrestd authenticates.
    pub async fn check(&mut self) -> Result<String, String> {
        self.call_api(reqwest::Method::GET, "partitions", None)
            .await?;
        Ok(self.user.clone())
    }

    pub async fn call(&mut self, tool: &str, args: &Value) -> Result<(bool, String), String> {
        match tool {
            "cluster" => self.cluster().await.map(|t| (true, t)),
            "jobs" => self.jobs(args).await.map(|t| (true, t)),
            "job" => self.job(args).await.map(|t| (true, t)),
            "submit" => self.submit(args, None).await.map(|(t, _)| (true, t)),
            "cancel" => {
                let id = job_id(args)?;
                self.call_api(reqwest::Method::DELETE, &format!("job/{id}"), None)
                    .await?;
                Ok((true, format!("Asked Slurm to cancel job {id}.")))
            }
            other => Err(format!("There is no Slurm tool named {other}")),
        }
    }

    async fn cluster(&mut self) -> Result<String, String> {
        let partitions = self
            .call_api(reqwest::Method::GET, "partitions", None)
            .await?;
        let nodes = self.call_api(reqwest::Method::GET, "nodes", None).await?;
        let mut out = vec!["Partitions (name, state, nodes, CPUs, max time):".to_string()];
        for p in partitions["partitions"].as_array().into_iter().flatten() {
            out.push(format!(
                "- {}: {}, nodes {}, {} CPUs, max {}",
                text(&p["name"]),
                text(&p["partition"]["state"]),
                text(&p["nodes"]["configured"]),
                num(&p["cpus"]["total"]).map_or("?".into(), |n| n.to_string()),
                minutes(&p["maximums"]["time"])
            ));
        }
        out.push(
            "Nodes (name, state, CPUs in use/total, memory in use/total MB, partitions):".into(),
        );
        for n in nodes["nodes"].as_array().into_iter().flatten() {
            out.push(format!(
                "- {}: {}, CPUs {}/{}, memory {}/{} MB, {}",
                text(&n["name"]),
                text(&n["state"]),
                num(&n["alloc_cpus"]).unwrap_or(0),
                num(&n["cpus"]).unwrap_or(0),
                num(&n["alloc_memory"]).unwrap_or(0),
                num(&n["real_memory"]).unwrap_or(0),
                text(&n["partitions"])
            ));
        }
        Ok(out.join("\n"))
    }

    /// The person's running and pending jobs, newest first, from the accounting
    /// database, which filters by user (the scheduler's job list cannot).
    async fn own_jobs(&mut self, states: &[&str]) -> Result<Vec<Value>, String> {
        let mut jobs = vec![];
        for state in states {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("users", &self.user)
                .append_pair("state", state)
                .finish();
            let data = self.call_db(&format!("jobs?{query}")).await?;
            jobs.extend(data["jobs"].as_array().cloned().unwrap_or_default());
        }
        jobs.sort_by_key(|j| std::cmp::Reverse(num(&j["job_id"]).unwrap_or(0)));
        Ok(jobs)
    }

    async fn jobs(&mut self, args: &Value) -> Result<String, String> {
        let state = args["state"]
            .as_str()
            .map(str::to_ascii_uppercase)
            .filter(|s| ACTIVE.contains(&s.as_str()));
        let states: Vec<&str> = match &state {
            Some(s) => vec![s.as_str()],
            None => ACTIVE.to_vec(),
        };
        let lines: Vec<String> = self
            .own_jobs(&states)
            .await?
            .iter()
            .map(|j| {
                let job = own_job(j);
                format!(
                    "- {} {} [{}] partition {}, ran {} of {}, nodes {}{}",
                    job["id"],
                    job["name"].as_str().unwrap_or(""),
                    job["state"].as_str().unwrap_or(""),
                    job["partition"].as_str().unwrap_or(""),
                    job["elapsed"].as_str().unwrap_or(""),
                    job["timeLimit"].as_str().unwrap_or(""),
                    job["nodes"]
                        .as_str()
                        .filter(|n| !n.is_empty())
                        .unwrap_or("-"),
                    match job["reason"].as_str() {
                        Some(r) if !r.is_empty() => format!(", reason {r}"),
                        _ => String::new(),
                    }
                )
            })
            .collect();
        Ok(if lines.is_empty() {
            format!(
                "No {} jobs of yours.",
                state
                    .as_deref()
                    .unwrap_or("running or pending")
                    .to_lowercase()
            )
        } else {
            lines.join("\n")
        })
    }

    async fn job(&mut self, args: &Value) -> Result<String, String> {
        let id = job_id(args)?;
        // The scheduler forgets a job minutes after it ends; accounting keeps it.
        let data = match self
            .call_api(reqwest::Method::GET, &format!("job/{id}"), None)
            .await
        {
            Ok(data) => data,
            Err(error) => {
                return match self.accounted(id).await {
                    Some(j) => Ok(finished_job(id, &j)),
                    None => Err(error),
                }
            }
        };
        let Some(j) = data["jobs"].as_array().and_then(|a| a.first()) else {
            return Err(format!("Slurm has no job {id}"));
        };
        let exit = num(&j["exit_code"]["return_code"])
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        Ok([
            format!("Job {id}: {} [{}]", text(&j["name"]), text(&j["job_state"])),
            format!(
                "User {}, account {}, partition {}",
                text(&j["user_name"]),
                text(&j["account"]),
                text(&j["partition"])
            ),
            format!("Reason: {}", text(&j["state_reason"])),
            format!(
                "Nodes {}, CPUs {}, time limit {}",
                text(&j["nodes"]),
                num(&j["cpus"]).unwrap_or(0),
                minutes(&j["time_limit"])
            ),
            format!(
                "Submitted {}, started {}, ended {}",
                time(&j["submit_time"]),
                time(&j["start_time"]),
                time(&j["end_time"])
            ),
            format!("Exit code {exit}"),
            format!(
                "Working directory {}",
                text(&j["current_working_directory"])
            ),
            format!(
                "Output {}, errors {}",
                text(&j["standard_output"]),
                text(&j["standard_error"])
            ),
            format!("Command {}", text(&j["command"])),
        ]
        .join("\n"))
    }

    /// One job from accounting, which keeps jobs after the scheduler forgets them.
    async fn accounted(&mut self, id: i64) -> Option<Value> {
        let data = self.call_db(&format!("job/{id}")).await.ok()?;
        data["jobs"].as_array()?.first().cloned()
    }

    /// The job's state, such as RUNNING or COMPLETED, from the scheduler or accounting.
    pub async fn state_of(&mut self, id: i64) -> Option<String> {
        if let Ok(data) = self
            .call_api(reqwest::Method::GET, &format!("job/{id}"), None)
            .await
        {
            if let Some(j) = data["jobs"].as_array().and_then(|a| a.first()) {
                return Some(text(&j["job_state"]));
            }
        }
        self.accounted(id)
            .await
            .map(|j| text(&j["state"]["current"]))
    }

    /// Submits the job. With `upload`, the job also sends the end of its output there
    /// as it ends. Returns what to tell the model and the job id.
    pub async fn submit(
        &mut self,
        args: &Value,
        upload: Option<&Url>,
    ) -> Result<(String, i64), String> {
        let script = args["script"].as_str().unwrap_or("").trim_start();
        if script.is_empty() || script.len() > 256 * 1024 {
            return Err("The script must be between 1 byte and 256 KB".into());
        }
        let script = if script.starts_with("#!") {
            script.to_owned()
        } else {
            format!("#!/bin/bash\n{script}")
        };
        let directory = args["working_directory"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("/home/{}", self.user));
        let mut job = json!({
            "name": args["name"].as_str().unwrap_or("chat-job"),
            "current_working_directory": directory,
            "environment": ["PATH=/usr/local/bin:/usr/bin:/bin"],
        });
        if let Some(p) = args["partition"].as_str() {
            job["partition"] = json!(p);
        }
        if let Some(m) = args["time_limit_minutes"].as_i64() {
            job["time_limit"] = json!({"set":true,"number":m});
        }
        if let Some(c) = args["cpus"].as_i64() {
            job["cpus_per_task"] = json!(c);
        }
        if let Some(t) = args["tasks"].as_i64() {
            job["tasks"] = json!(t);
        }
        if let Some(n) = args["nodes"].as_i64() {
            job["minimum_nodes"] = json!(n);
        }
        if let Some(m) = args["memory_mb"].as_i64() {
            job["memory_per_node"] = json!({"set":true,"number":m});
        }
        if let Some(o) = args["output"].as_str() {
            job["standard_output"] = json!(o);
        }
        // Where Slurm writes the output, as the job's shell names it, when Chat can
        // follow the pattern: then the job sends the end of that file back.
        let output = args["output"].as_str().unwrap_or(DEFAULT_OUTPUT);
        let script = match (upload, output_path(output)) {
            (Some(upload), Some(path)) => {
                job["standard_output"] = json!(output);
                returning_output(&script, upload, &path)
            }
            _ => script,
        };
        let result = self
            .call_api(
                reqwest::Method::POST,
                "job/submit",
                Some(json!({"script":script,"job":job})),
            )
            .await?;
        let id = num(&result["job_id"]).ok_or("Slurm did not return a job id")?;
        let warnings: Vec<String> = result["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| text(&w["description"]))
            .filter(|w| !w.is_empty())
            .collect();
        let text = format!(
            "Submitted job {id} as {} in {}.{}",
            self.user,
            job["current_working_directory"].as_str().unwrap_or(""),
            if warnings.is_empty() {
                String::new()
            } else {
                format!(" Warnings: {}", warnings.join("; "))
            }
        );
        Ok((text, id))
    }
}

impl Client {
    /// Partitions, nodes, the queue and the person's own jobs, for the Clusters page.
    pub async fn status(&mut self) -> Result<Value, String> {
        let partitions = self
            .call_api(reqwest::Method::GET, "partitions", None)
            .await?;
        let nodes = self.call_api(reqwest::Method::GET, "nodes", None).await?;
        // The queue's size from the scheduler's statistics, not by listing every job.
        let diag = self
            .call_api(reqwest::Method::GET, "diag", None)
            .await
            .unwrap_or(Value::Null);
        let mine = self.own_jobs(&ACTIVE).await?;
        let list = |v: &Value, key: &str| v[key].as_array().cloned().unwrap_or_default();
        let partitions: Vec<Value> = list(&partitions, "partitions")
            .iter()
            .map(|p| {
                json!({
                    "name": text(&p["name"]),
                    "state": text(&p["partition"]["state"]),
                    "nodes": num(&p["nodes"]["total"]),
                    "cpus": num(&p["cpus"]["total"]),
                    "maxTime": minutes(&p["maximums"]["time"]),
                })
            })
            .collect();
        let nodes: Vec<Value> = list(&nodes, "nodes")
            .iter()
            .map(|n| {
                json!({
                    "name": text(&n["name"]),
                    "state": text(&n["state"]),
                    "cpus": num(&n["cpus"]).unwrap_or(0),
                    "allocCpus": num(&n["alloc_cpus"]).unwrap_or(0),
                    "memoryMb": num(&n["real_memory"]).unwrap_or(0),
                    "allocMemoryMb": num(&n["alloc_memory"]).unwrap_or(0),
                    "partitions": text(&n["partitions"]),
                })
            })
            .collect();
        let sum = |key: &str| {
            nodes
                .iter()
                .map(|n| n[key].as_i64().unwrap_or(0))
                .sum::<i64>()
        };
        let mine: Vec<Value> = mine.iter().take(25).map(own_job).collect();
        Ok(json!({
            "user": self.user,
            "partitions": partitions,
            "totals": {
                "nodes": nodes.len(),
                "cpus": sum("cpus"), "allocCpus": sum("allocCpus"),
                "memoryMb": sum("memoryMb"), "allocMemoryMb": sum("allocMemoryMb"),
            },
            "nodes": nodes,
            "queue": {
                "running": num(&diag["statistics"]["jobs_running"]),
                "pending": num(&diag["statistics"]["jobs_pending"]),
            },
            "jobs": mine,
        }))
    }
}

/// Slurm's own default output file, in the working directory.
const DEFAULT_OUTPUT: &str = "slurm-%j.out";
/// The most of a job's output that comes back: its last 1 MiB.
pub const RETURNED_OUTPUT: usize = 1024 * 1024;

/// An output file pattern (`--output`) as a path the job's shell expands, or None for
/// patterns Chat can't follow (such as `%4j`) or characters the shell would read.
fn output_path(pattern: &str) -> Option<String> {
    if pattern.is_empty()
        || pattern
            .chars()
            .any(|c| matches!(c, '"' | '$' | '`' | '\\') || c.is_control())
    {
        return None;
    }
    let mut path = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            path.push(c);
            continue;
        }
        path.push_str(match chars.next()? {
            'j' => "${SLURM_JOB_ID}",
            'x' => "${SLURM_JOB_NAME}",
            'u' => "${USER}",
            'A' => "${SLURM_ARRAY_JOB_ID:-$SLURM_JOB_ID}",
            'a' => "${SLURM_ARRAY_TASK_ID}",
            'N' => "$(hostname -s)",
            '%' => "%",
            _ => return None,
        });
    }
    Some(path)
}

/// The job script Chat submits to bring output back: it runs the person's script
/// unchanged, then uploads the end of the output file to `upload`, a link that can
/// write that one object. It also sends it when the job is cancelled or runs out of
/// time, since Slurm sends SIGTERM before it kills the job.
fn returning_output(script: &str, upload: &Url, path: &str) -> String {
    let end = format!("OBLETH_CHAT_SCRIPT_{}", uuid::Uuid::new_v4().simple());
    // A presigned URL is percent-encoded, so it holds no quote to escape.
    let url = upload.as_str().replace('\'', "%27");
    format!(
        r#"#!/bin/bash
# Chat runs the script below unchanged, then sends the end of this job's output back
# to Chat, so the chat that submitted it can read it.
chat_script=$(mktemp "${{TMPDIR:-/tmp}}/chat-job.XXXXXX") || exit 1
cat > "$chat_script" <<'{end}'
{script}
{end}
chmod +x "$chat_script"
chat_sent=
chat_send() {{
  [ -n "$chat_sent" ] && return
  chat_sent=1
  chat_tail=$(mktemp "${{TMPDIR:-/tmp}}/chat-out.XXXXXX") || return
  tail -c {RETURNED_OUTPUT} "{path}" > "$chat_tail" 2>/dev/null
  curl -s --connect-timeout 10 -m 60 -T "$chat_tail" '{url}' > /dev/null 2>&1
  rm -f "$chat_tail" "$chat_script"
}}
trap 'chat_send; exit 143' TERM INT
"$chat_script"
chat_status=$?
chat_send
exit $chat_status
"#
    )
}

/// One job as accounting describes it, for Job details once the scheduler forgot it.
fn finished_job(id: i64, j: &Value) -> String {
    let exit = num(&j["exit_code"]["return_code"])
        .map(|c| c.to_string())
        .unwrap_or_else(|| "-".into());
    let file = |expanded: &str, plain: &str| {
        Some(text(&j[expanded]))
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| text(&j[plain]))
    };
    [
        format!(
            "Job {id}: {} [{}], from accounting (the scheduler no longer lists it)",
            text(&j["name"]),
            text(&j["state"]["current"])
        ),
        format!(
            "User {}, account {}, partition {}",
            text(&j["user"]),
            text(&j["account"]),
            text(&j["partition"])
        ),
        format!("Reason: {}", text(&j["state"]["reason"])),
        format!(
            "Nodes {}, ran {} of {}",
            text(&j["nodes"]),
            duration(num(&j["time"]["elapsed"]).unwrap_or(0)),
            minutes(&j["time"]["limit"])
        ),
        format!(
            "Submitted {}, started {}, ended {}",
            time(&j["time"]["submission"]),
            time(&j["time"]["start"]),
            time(&j["time"]["end"])
        ),
        format!("Exit code {exit}"),
        format!("Working directory {}", text(&j["working_directory"])),
        format!(
            "Output {}, errors {}",
            file("stdout_expanded", "stdout"),
            file("stderr_expanded", "stderr")
        ),
    ]
    .join("\n")
}

/// The states the person's job list shows, asked for one at a time.
const ACTIVE: [&str; 2] = ["RUNNING", "PENDING"];

/// One of the person's jobs from the accounting database.
fn own_job(j: &Value) -> Value {
    let state = text(&j["state"]["current"]);
    json!({
        "id": num(&j["job_id"]),
        "name": text(&j["name"]),
        "state": state,
        "partition": text(&j["partition"]),
        "elapsed": if state.contains("PENDING") { "-".into() } else { duration(num(&j["time"]["elapsed"]).unwrap_or(0)) },
        "timeLimit": minutes(&j["time"]["limit"]),
        "nodes": text(&j["nodes"]).replace("None assigned", ""),
        "reason": j["state"]["reason"].as_str().filter(|r| *r != "None").unwrap_or(""),
        "submitted": num(&j["time"]["submission"]),
    })
}

fn not_offered() -> String {
    format!(
        "This server does not offer the Slurm REST API {} to {}. If it runs a newer one, set that version on the cluster.",
        VERSIONS[VERSIONS.len() - 1],
        VERSIONS[0]
    )
}

pub(super) fn job_id(args: &Value) -> Result<i64, String> {
    args["job_id"]
        .as_i64()
        .or_else(|| args["job_id"].as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|id| *id > 0)
        .ok_or_else(|| "job_id must be a positive number".into())
}

fn time(value: &Value) -> String {
    num(value)
        .filter(|t| *t > 0)
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map_or("-".into(), |t| t.format("%Y-%m-%d %H:%M UTC").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slurm_numbers_and_times_read_every_shape() {
        assert_eq!(num(&json!(5)), Some(5));
        assert_eq!(
            num(&json!({"set":true,"infinite":false,"number":30})),
            Some(30)
        );
        assert_eq!(num(&json!({"set":false,"number":0})), None);
        assert_eq!(minutes(&json!({"set":true,"number":90})), "1h30m");
        assert_eq!(
            minutes(&json!({"set":true,"infinite":true,"number":0})),
            "unlimited"
        );
        assert_eq!(
            text(&json!(["RUNNING", "COMPLETING"])),
            "RUNNING,COMPLETING"
        );
        assert_eq!(job_id(&json!({"job_id":"42"})), Ok(42));
        assert!(job_id(&json!({"job_id":-1})).is_err());
        assert_eq!(tools(false).iter().filter(|t| !t.read_only).count(), 2);
        // Job output only reads.
        assert_eq!(tools(true).len(), tools(false).len() + 1);
        assert_eq!(tools(true).iter().filter(|t| !t.read_only).count(), 2);
    }

    /// Runs the job script Chat submits the way Slurm would: its output going to the
    /// output file, with a `curl` that keeps what it was asked to upload.
    fn run_returning(
        script: &str,
        signal_after: Option<std::time::Duration>,
    ) -> (Option<i32>, String) {
        let dir =
            std::env::temp_dir().join(format!("job-output-{}", uuid::Uuid::new_v4().simple()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            bin.join("curl"),
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = -T ] && cp \"$2\" \"$UPLOADED\"; shift; done\n",
        )
        .unwrap();
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(bin.join("curl"))
            .status()
            .unwrap();
        let upload =
            Url::parse("https://store.example/bucket/job-output/x?X-Amz-Signature=abc").unwrap();
        let wrapped = returning_output(script, &upload, &output_path("out-%j.txt").unwrap());
        std::fs::write(dir.join("job.sh"), wrapped).unwrap();
        let out = std::fs::File::create(dir.join("out-8.txt")).unwrap();
        let mut child = std::process::Command::new("setsid")
            .arg("bash")
            .arg("job.sh")
            .current_dir(&dir)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("SLURM_JOB_ID", "8")
            .env("TMPDIR", &dir)
            .env("UPLOADED", dir.join("uploaded"))
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .unwrap();
        if let Some(after) = signal_after {
            std::thread::sleep(after);
            // Like scancel or a time limit: SIGTERM to every process of the job.
            std::process::Command::new("kill")
                .args(["-TERM", "--", &format!("-{}", child.id())])
                .status()
                .unwrap();
        }
        let status = child.wait().unwrap();
        let uploaded = std::fs::read_to_string(dir.join("uploaded")).unwrap_or_default();
        std::fs::remove_dir_all(&dir).ok();
        (status.code(), uploaded)
    }

    #[test]
    fn a_job_sends_the_end_of_its_output_and_keeps_its_exit_code() {
        if std::process::Command::new("setsid")
            .arg("true")
            .status()
            .is_err()
        {
            return;
        }
        let (code, uploaded) = run_returning(
            "#!/bin/bash\necho \"hello from $SLURM_JOB_ID\"\nexit 3",
            None,
        );
        assert_eq!(code, Some(3));
        assert_eq!(uploaded, "hello from 8\n");
        // Another interpreter's script runs as itself.
        let (code, uploaded) = run_returning("#!/bin/sh\nprintf 'sh too\\n'", None);
        assert_eq!((code, uploaded.as_str()), (Some(0), "sh too\n"));
    }

    #[test]
    fn a_cancelled_job_still_sends_its_output() {
        if std::process::Command::new("setsid")
            .arg("true")
            .status()
            .is_err()
        {
            return;
        }
        let (code, uploaded) = run_returning(
            "#!/bin/bash\necho started\nsleep 30\necho never",
            Some(std::time::Duration::from_millis(700)),
        );
        assert_eq!(code, Some(143));
        // What it printed before it was stopped, and the shell's note that it was.
        assert!(uploaded.starts_with("started\n"), "{uploaded}");
        assert!(!uploaded.contains("never"), "{uploaded}");
    }

    #[test]
    fn output_patterns_become_paths_the_job_expands() {
        assert_eq!(
            output_path("slurm-%j.out").as_deref(),
            Some("slurm-${SLURM_JOB_ID}.out")
        );
        assert_eq!(output_path("logs/%x-%A_%a.txt").as_deref(), Some("logs/${SLURM_JOB_NAME}-${SLURM_ARRAY_JOB_ID:-$SLURM_JOB_ID}_${SLURM_ARRAY_TASK_ID}.txt"));
        assert_eq!(output_path("100%%.log").as_deref(), Some("100%.log"));
        for refused in ["%4j.out", "a$(id).out", "a\"b", "x%"] {
            assert_eq!(output_path(refused), None, "{refused}");
        }
    }

    #[test]
    fn api_versions_look_like_slurmrestds() {
        assert!(valid_version("v0.0.45") && valid_version("v0.0.100"));
        for bad in [
            "0.0.45",
            "v0.0.4",
            "v0.0.4x",
            "v0.1.45",
            "v0.0.45/",
            "v0.0.1000",
        ] {
            assert!(!valid_version(bad), "{bad}");
        }
    }
}
