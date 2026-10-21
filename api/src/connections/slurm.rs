//! Slurm through slurmrestd with a person's own JWT, so jobs run as their account.
//! Chat finds the newest data parser the server offers (v0.0.42 to v0.0.40).
use super::mcp::Tool;
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

const VERSIONS: [&str; 3] = ["v0.0.42", "v0.0.41", "v0.0.40"];
const TIMEOUT: Duration = Duration::from_secs(60);

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
        tool("jobs", "Jobs", "List Slurm jobs, by default only your own.", true, json!({"all_users":{"type":"boolean","description":"Include other users' jobs"},"state":{"type":"string","description":"Only jobs in this state, such as RUNNING or PENDING"}}), &[]),
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
    version: Option<&'static str>,
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
    pub fn new(http: reqwest::Client, base: Url, user: String, token: String) -> Self {
        let mut base = base;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Self {
            http,
            base,
            user,
            token,
            version: None,
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
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "slurmrestd closed the connection".to_string())?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }

    async fn version(&mut self) -> Result<&'static str, String> {
        if let Some(v) = self.version {
            return Ok(v);
        }
        for v in VERSIONS {
            if let Ok((200, _)) = self
                .send(reqwest::Method::GET, &format!("slurm/{v}/ping"), None)
                .await
            {
                self.version = Some(v);
                return Ok(v);
            }
        }
        Err("This server does not offer the Slurm REST API v0.0.40 or newer".into())
    }

    async fn call_api(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let v = self.version().await?;
        let (status, value) = self
            .send(method, &format!("slurm/{v}/{path}"), body)
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
            _ => Err(format!("slurmrestd answered HTTP {status}")),
        }
    }

    /// Whether slurmrestd answers at all, for a cluster shared before anyone has a token.
    pub async fn reachable(&self) -> Result<(), String> {
        for v in VERSIONS {
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
        Err("No Slurm REST API v0.0.40 or newer at this URL".into())
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

    async fn jobs(&mut self, args: &Value) -> Result<String, String> {
        let all = args["all_users"].as_bool() == Some(true);
        let state = args["state"].as_str().map(str::to_ascii_uppercase);
        let data = self.call_api(reqwest::Method::GET, "jobs", None).await?;
        let now = chrono::Utc::now().timestamp();
        let lines: Vec<String> = data["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|j| all || j["user_name"].as_str() == Some(&self.user))
            .filter(|j| {
                state
                    .as_ref()
                    .is_none_or(|s| text(&j["job_state"]).contains(s.as_str()))
            })
            .map(|j| {
                let ran = ran(j, now);
                let state = text(&j["job_state"]);
                format!(
                    "- {} {} [{}] user {}, partition {}, ran {} of {}, nodes {}{}",
                    num(&j["job_id"]).unwrap_or(0),
                    text(&j["name"]),
                    state,
                    text(&j["user_name"]),
                    text(&j["partition"]),
                    duration(ran),
                    minutes(&j["time_limit"]),
                    if text(&j["nodes"]).is_empty() {
                        "-".into()
                    } else {
                        text(&j["nodes"])
                    },
                    match j["state_reason"].as_str() {
                        Some(r) if r != "None" && !r.is_empty() => format!(", reason {r}"),
                        _ => String::new(),
                    }
                )
            })
            .collect();
        Ok(if lines.is_empty() {
            format!(
                "No {}jobs{}.",
                if all { "" } else { "of your " },
                state.map(|s| format!(" in state {s}")).unwrap_or_default()
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
        let jobs = self.call_api(reqwest::Method::GET, "jobs", None).await?;
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
        let all = list(&jobs, "jobs");
        let in_state = |state: &str| {
            all.iter()
                .filter(|j| text(&j["job_state"]).contains(state))
                .count()
        };
        let now = chrono::Utc::now().timestamp();
        let mut mine: Vec<&Value> = all
            .iter()
            .filter(|j| j["user_name"].as_str() == Some(&self.user))
            .collect();
        mine.sort_by_key(|j| std::cmp::Reverse(num(&j["job_id"]).unwrap_or(0)));
        let mine: Vec<Value> = mine
            .into_iter()
            .take(25)
            .map(|j| {
                let ran = ran(j, now);
                json!({
                    "id": num(&j["job_id"]),
                    "name": text(&j["name"]),
                    "state": text(&j["job_state"]),
                    "partition": text(&j["partition"]),
                    "elapsed": if text(&j["job_state"]).contains("PENDING") { "-".into() } else { duration(ran) },
                    "timeLimit": minutes(&j["time_limit"]),
                    "nodes": text(&j["nodes"]),
                    "reason": j["state_reason"].as_str().filter(|r| *r != "None").unwrap_or(""),
                    "submitted": num(&j["submit_time"]),
                })
            })
            .collect();
        Ok(json!({
            "user": self.user,
            "partitions": partitions,
            "totals": {
                "nodes": nodes.len(),
                "cpus": sum("cpus"), "allocCpus": sum("allocCpus"),
                "memoryMb": sum("memoryMb"), "allocMemoryMb": sum("allocMemoryMb"),
            },
            "nodes": nodes,
            "queue": {"running": in_state("RUNNING"), "pending": in_state("PENDING"), "total": all.len()},
            "jobs": mine,
        }))
    }
}

/// How long a job has run. For a running job slurmrestd reports end_time as the
/// expected end, so only a finished job's end time counts.
fn ran(job: &Value, now: i64) -> i64 {
    let start = num(&job["start_time"]).unwrap_or(0);
    let end = num(&job["end_time"]).unwrap_or(0);
    if start <= 0 || start > now {
        0
    } else if text(&job["job_state"]).contains("RUNNING") || end < start || end > now {
        now - start
    } else {
        end - start
    }
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
    fn running_jobs_count_time_until_now_not_their_expected_end() {
        let now = 10_000;
        let running = json!({"job_state":["RUNNING"],"start_time":9_940,"end_time":11_740});
        let finished = json!({"job_state":["COMPLETED"],"start_time":9_000,"end_time":9_120});
        let pending = json!({"job_state":["PENDING"],"start_time":11_000,"end_time":12_800});
        assert_eq!(
            (ran(&running, now), ran(&finished, now), ran(&pending, now)),
            (60, 120, 0)
        );
    }

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
}
