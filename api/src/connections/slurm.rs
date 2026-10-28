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

pub fn tools() -> Vec<Tool> {
    vec![
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
    ]
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
            "submit" => self.submit(args).await.map(|t| (true, t)),
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
        let data = self
            .call_api(reqwest::Method::GET, &format!("job/{id}"), None)
            .await?;
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

    async fn submit(&mut self, args: &Value) -> Result<String, String> {
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
        Ok(format!(
            "Submitted job {id} as {} in {}.{}",
            self.user,
            job["current_working_directory"].as_str().unwrap_or(""),
            if warnings.is_empty() {
                String::new()
            } else {
                format!(" Warnings: {}", warnings.join("; "))
            }
        ))
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

fn job_id(args: &Value) -> Result<i64, String> {
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
        assert_eq!(tools().iter().filter(|t| !t.read_only).count(), 2);
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
