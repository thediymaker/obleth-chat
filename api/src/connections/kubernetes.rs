//! Kubernetes with a person's own token, so the cluster's RBAC decides what they may
//! do. Resource kinds are found through API discovery, and manifests are applied with
//! server-side apply, as JSON or YAML.
use super::mcp::Tool;
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};
use url::Url;

/// The namespace of a service account identity, system:serviceaccount:<ns>:<name>.
pub fn home_namespace(identity: &str) -> Option<String> {
    let mut parts = identity.strip_prefix("system:serviceaccount:")?.split(':');
    let ns = parts.next().filter(|n| !n.is_empty())?;
    parts.next()?;
    Some(ns.to_owned())
}

const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ROWS: usize = 200;
/// The most Chat reads of one Kubernetes API answer.
const MAX_BODY: usize = 32 * 1024 * 1024;
/// Group versions searched first when a kind is given without an apiVersion.
const COMMON: [&str; 8] = [
    "v1",
    "apps/v1",
    "batch/v1",
    "networking.k8s.io/v1",
    "autoscaling/v2",
    "policy/v1",
    "rbac.authorization.k8s.io/v1",
    "storage.k8s.io/v1",
];

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
    let namespace = json!({"type":"string","description":"Defaults to your namespace"});
    let kind = json!({"type":"string","description":"Kind or resource name, such as Pod, deployments or svc"});
    let api_version = json!({"type":"string","description":"apiVersion for custom resources, such as example.com/v1"});
    vec![
        tool("overview", "Overview", "Who you are on the cluster, your namespace's quotas and pods, and what you are allowed to do there.", true, json!({"namespace":namespace}), &[]),
        tool("list", "List", "List resources of one kind with their status.", true, json!({"kind":kind,"namespace":namespace,"all_namespaces":{"type":"boolean"},"label_selector":{"type":"string"},"api_version":api_version}), &["kind"]),
        tool("get", "Get", "One resource as JSON, without managed fields.", true, json!({"kind":kind,"name":{"type":"string"},"namespace":namespace,"api_version":api_version}), &["kind","name"]),
        tool("logs", "Logs", "The latest log lines of a pod's container.", true, json!({"pod":{"type":"string"},"namespace":namespace,"container":{"type":"string"},"tail_lines":{"type":"integer","minimum":1,"maximum":2000},"previous":{"type":"boolean","description":"Logs of the previous, crashed container"}}), &["pod"]),
        tool("apply", "Apply", "Create or update resources with server-side apply. Give one manifest object, a list of them, or YAML text (documents separated by ---).", false, json!({"manifest":{"description":"Manifest object, array of objects, or YAML text"}}), &["manifest"]),
        tool("delete", "Delete", "Delete one resource.", false, json!({"kind":kind,"name":{"type":"string"},"namespace":namespace,"api_version":api_version}), &["kind","name"]),
    ]
}

#[derive(Clone, Debug)]
struct Resource {
    path: String,
    name: String,
    singular: String,
    kind: String,
    short_names: Vec<String>,
    namespaced: bool,
}

pub struct Client {
    http: reqwest::Client,
    base: Url,
    token: String,
    pub namespace: String,
    discovered: HashMap<String, Vec<Resource>>,
    groups: Option<Vec<String>>,
}

enum Body {
    Json(Value),
    Apply(String),
}

impl Client {
    pub fn new(http: reqwest::Client, base: Url, token: String, namespace: String) -> Self {
        let mut base = base;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Self {
            http,
            base,
            token,
            namespace,
            discovered: HashMap::new(),
            groups: None,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Body>,
    ) -> Result<(u16, Vec<u8>), String> {
        let url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| "Invalid Kubernetes path".to_string())?;
        let mut request = self
            .http
            .request(method, url)
            .timeout(TIMEOUT)
            .bearer_auth(&self.token)
            .header("accept", "application/json");
        request = match body {
            Some(Body::Json(value)) => request.json(&value),
            Some(Body::Apply(text)) => request
                .header("content-type", "application/apply-patch+yaml")
                .body(text),
            None => request,
        };
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                "The Kubernetes API did not answer in time".to_string()
            } else if e.is_connect() {
                "Could not connect to the Kubernetes API".to_string()
            } else {
                format!("The Kubernetes request failed: {e}")
            }
        })?;
        let status = response.status().as_u16();
        let bytes = super::read_capped(response, MAX_BODY)
            .await
            .map_err(|e| match e {
                super::Read::Closed => "The Kubernetes API closed the connection".to_string(),
                super::Read::TooLarge => format!(
                    "The Kubernetes API's answer was larger than {} MiB, so Chat did not read it. Narrow it with a namespace or label selector.",
                    MAX_BODY / 1024 / 1024
                ),
            })?;
        if (200..300).contains(&status) {
            return Ok((status, bytes));
        }
        let message = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v["message"].as_str().map(str::to_owned))
            .unwrap_or_default();
        Err(match status {
            401 => "The cluster rejected the token (HTTP 401). It may have expired; add a new one in Settings → Connections.".into(),
            403 if !message.is_empty() => format!("Kubernetes: {message} (HTTP 403). The cluster's RBAC does not allow this for this account. Do not probe other resources to work around it: tell the person what was refused and ask whether to use another namespace."),
            _ if !message.is_empty() => format!("Kubernetes: {message} (HTTP {status})"),
            _ => format!("The Kubernetes API answered HTTP {status}"),
        })
    }

    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Body>,
    ) -> Result<(u16, Value), String> {
        let (status, bytes) = self.send(method, path, body).await?;
        Ok((
            status,
            serde_json::from_slice(&bytes)
                .map_err(|_| "The Kubernetes API returned invalid JSON".to_string())?,
        ))
    }

    /// Whether the API server answers, and its version when anonymous requests may
    /// read it; for a cluster shared before anyone has a token.
    pub async fn reachable(&self) -> Result<Option<String>, String> {
        let url = self
            .base
            .join("version")
            .map_err(|_| "Invalid Kubernetes URL".to_string())?;
        let response = self
            .http
            .get(url)
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    format!("Could not connect to the Kubernetes API: {e}")
                } else {
                    format!("The Kubernetes request failed: {e}")
                }
            })?;
        match response.status().as_u16() {
            200 => Ok(response
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v["gitVersion"].as_str().map(|g| format!("Kubernetes {g}")))),
            401 | 403 => Ok(None),
            status => Err(format!(
                "This does not look like a Kubernetes API server (HTTP {status})"
            )),
        }
    }

    /// The person's user name on the cluster, which also proves the token works.
    pub async fn check(&self) -> Result<String, String> {
        let (_, review) = self
            .json(
                reqwest::Method::POST,
                "apis/authentication.k8s.io/v1/selfsubjectreviews",
                Some(Body::Json(
                    json!({"apiVersion":"authentication.k8s.io/v1","kind":"SelfSubjectReview"}),
                )),
            )
            .await?;
        review["status"]["userInfo"]["username"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "The cluster did not say who the token belongs to".into())
    }

    async fn resources(&mut self, group_version: &str) -> Vec<Resource> {
        if let Some(found) = self.discovered.get(group_version) {
            return found.clone();
        }
        let path = if group_version == "v1" {
            "api/v1".to_string()
        } else {
            format!("apis/{group_version}")
        };
        let list = match self.json(reqwest::Method::GET, &path, None).await {
            Ok((_, doc)) => doc["resources"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let name = r["name"].as_str()?;
                    (!name.contains('/')).then(|| Resource {
                        path: format!("/{path}"),
                        name: name.into(),
                        singular: r["singularName"].as_str().unwrap_or("").into(),
                        kind: r["kind"].as_str().unwrap_or("").into(),
                        short_names: r["shortNames"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect(),
                        namespaced: r["namespaced"].as_bool().unwrap_or(true),
                    })
                })
                .collect(),
            Err(_) => vec![],
        };
        self.discovered.insert(group_version.into(), list.clone());
        list
    }

    async fn resolve(&mut self, kind: &str, api_version: Option<&str>) -> Result<Resource, String> {
        let wanted = kind.trim().to_ascii_lowercase();
        if wanted.is_empty() {
            return Err("Name a kind, such as pods".into());
        }
        let mut candidates: Vec<String> = match api_version {
            Some(v) => vec![v.trim().to_owned()],
            None => COMMON.iter().map(|s| s.to_string()).collect(),
        };
        for pass in 0..2 {
            for gv in &candidates {
                if let Some(found) = self.resources(gv).await.into_iter().find(|r| {
                    r.name == wanted
                        || r.singular == wanted
                        || r.kind.to_ascii_lowercase() == wanted
                        || r.short_names.contains(&wanted)
                }) {
                    return Ok(found);
                }
            }
            if pass == 0 && api_version.is_none() {
                // Then every other API group the cluster serves.
                if self.groups.is_none() {
                    let groups = match self.json(reqwest::Method::GET, "apis", None).await {
                        Ok((_, doc)) => doc["groups"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|g| {
                                g["preferredVersion"]["groupVersion"]
                                    .as_str()
                                    .map(str::to_owned)
                            })
                            .collect(),
                        Err(_) => vec![],
                    };
                    self.groups = Some(groups);
                }
                candidates = self
                    .groups
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|g| !COMMON.contains(&g.as_str()))
                    .collect();
            } else {
                break;
            }
        }
        Err(format!(
            "This cluster has no resource kind {kind}{}",
            api_version.map(|v| format!(" in {v}")).unwrap_or_default()
        ))
    }

    fn namespace_of(&self, args: &Value) -> String {
        args["namespace"]
            .as_str()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&self.namespace)
            .trim()
            .to_owned()
    }

    fn collection(r: &Resource, namespace: Option<&str>) -> String {
        match namespace {
            Some(ns) if r.namespaced => format!("{}/namespaces/{ns}/{}", r.path, r.name),
            _ => format!("{}/{}", r.path, r.name),
        }
    }

    pub async fn call(&mut self, tool: &str, args: &Value) -> Result<(bool, String), String> {
        let text = match tool {
            "overview" => self.overview(args).await?,
            "list" => self.list(args).await?,
            "get" => self.get(args).await?,
            "logs" => self.logs(args).await?,
            "apply" => self.apply(args).await?,
            "delete" => self.delete(args).await?,
            other => return Err(format!("There is no Kubernetes tool named {other}")),
        };
        Ok((true, text))
    }

    async fn overview(&mut self, args: &Value) -> Result<String, String> {
        let ns = self.namespace_of(args);
        let who = self.check().await?;
        let mut out = vec![format!("You are {who} on this cluster. Namespace: {ns}.")];
        if let Ok((_, quotas)) = self
            .json(
                reqwest::Method::GET,
                &format!("api/v1/namespaces/{ns}/resourcequotas"),
                None,
            )
            .await
        {
            for q in quotas["items"].as_array().into_iter().flatten() {
                let hard = q["status"]["hard"].as_object().cloned().unwrap_or_default();
                let used = &q["status"]["used"];
                let parts: Vec<String> = hard
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{k} {}/{}",
                            used[k].as_str().unwrap_or("0"),
                            v.as_str().unwrap_or("?")
                        )
                    })
                    .collect();
                out.push(format!(
                    "Quota {}: {}",
                    q["metadata"]["name"].as_str().unwrap_or(""),
                    parts.join(", ")
                ));
            }
        }
        match self
            .json(
                reqwest::Method::GET,
                &format!("api/v1/namespaces/{ns}/pods"),
                None,
            )
            .await
        {
            Ok((_, pods)) => {
                let mut phases: HashMap<String, usize> = HashMap::new();
                for p in pods["items"].as_array().into_iter().flatten() {
                    *phases
                        .entry(p["status"]["phase"].as_str().unwrap_or("Unknown").into())
                        .or_default() += 1;
                }
                let mut phases: Vec<_> = phases
                    .into_iter()
                    .map(|(k, v)| format!("{v} {k}"))
                    .collect();
                phases.sort();
                out.push(format!(
                    "Pods: {}",
                    if phases.is_empty() {
                        "none".into()
                    } else {
                        phases.join(", ")
                    }
                ));
            }
            Err(e) => out.push(format!("Pods: {e}")),
        }
        if let Ok((_, review)) = self
            .json(reqwest::Method::POST, "apis/authorization.k8s.io/v1/selfsubjectrulesreviews", Some(Body::Json(json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectRulesReview","spec":{"namespace":ns}}))))
            .await
        {
            out.push(format!("Allowed in {ns}:"));
            let rules: Vec<String> = review["status"]["resourceRules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| !r["resources"].as_array().is_some_and(|a| a.iter().all(|x| x.as_str().is_some_and(|s| s.starts_with("selfsubject")))))
                .map(|r| {
                    let list = |key: &str| r[key].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(",");
                    format!("- {}: {}", list("resources"), list("verbs"))
                })
                .take(25)
                .collect();
            if rules.is_empty() {
                out.push(format!("- nothing: this account has no permissions in {ns}."));
                if let Some(home) = home_namespace(&who).filter(|h| *h != ns) {
                    out.push(format!("It is a service account of namespace {home}, where its token usually works. Pass namespace: \"{home}\" to the tools, or ask the person."));
                }
            }
            out.extend(rules);
        }
        Ok(out.join("\n"))
    }

    async fn list(&mut self, args: &Value) -> Result<String, String> {
        let r = self
            .resolve(
                args["kind"].as_str().unwrap_or(""),
                args["api_version"].as_str(),
            )
            .await?;
        let all = args["all_namespaces"].as_bool() == Some(true);
        let ns = self.namespace_of(args);
        let mut path = format!(
            "{}?limit=500",
            Self::collection(&r, (!all).then_some(ns.as_str()))
        );
        if let Some(selector) = args["label_selector"].as_str().filter(|s| !s.is_empty()) {
            path.push_str(&format!("&labelSelector={}", urlencode(selector)));
        }
        let (_, list) = self.json(reqwest::Method::GET, &path, None).await?;
        let items = list["items"].as_array().cloned().unwrap_or_default();
        let now = chrono::Utc::now();
        let mut rows: Vec<String> = items
            .iter()
            .take(MAX_ROWS)
            .map(|item| row(&r.kind, item, now, all || !r.namespaced))
            .collect();
        if items.len() > MAX_ROWS {
            rows.push(format!("… and {} more", items.len() - MAX_ROWS));
        }
        let place = if !r.namespaced {
            String::new()
        } else if all {
            " in all namespaces".into()
        } else {
            format!(" in {ns}")
        };
        Ok(if rows.is_empty() {
            format!("No {}{place}.", r.name)
        } else {
            format!("{} {}{place}:\n{}", items.len(), r.name, rows.join("\n"))
        })
    }

    async fn get(&mut self, args: &Value) -> Result<String, String> {
        let r = self
            .resolve(
                args["kind"].as_str().unwrap_or(""),
                args["api_version"].as_str(),
            )
            .await?;
        let name = args["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or("name is required")?;
        let ns = self.namespace_of(args);
        let (_, mut object) = self
            .json(
                reqwest::Method::GET,
                &format!("{}/{}", Self::collection(&r, Some(&ns)), urlencode(name)),
                None,
            )
            .await?;
        if let Some(meta) = object["metadata"].as_object_mut() {
            meta.remove("managedFields");
            if let Some(notes) = meta.get_mut("annotations").and_then(Value::as_object_mut) {
                notes.remove("kubectl.kubernetes.io/last-applied-configuration");
            }
        }
        Ok(serde_json::to_string_pretty(&object).unwrap_or_default())
    }

    async fn logs(&mut self, args: &Value) -> Result<String, String> {
        let pod = args["pod"]
            .as_str()
            .filter(|p| !p.is_empty())
            .ok_or("pod is required")?;
        let ns = self.namespace_of(args);
        let tail = args["tail_lines"].as_u64().unwrap_or(200).clamp(1, 2000);
        let mut path = format!(
            "api/v1/namespaces/{ns}/pods/{}/log?tailLines={tail}",
            urlencode(pod)
        );
        if let Some(c) = args["container"].as_str().filter(|c| !c.is_empty()) {
            path.push_str(&format!("&container={}", urlencode(c)));
        }
        if args["previous"].as_bool() == Some(true) {
            path.push_str("&previous=true");
        }
        let (_, bytes) = self.send(reqwest::Method::GET, &path, None).await?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Ok(if text.trim().is_empty() {
            "(no log output)".into()
        } else {
            text
        })
    }

    async fn apply(&mut self, args: &Value) -> Result<String, String> {
        let documents = documents(&args["manifest"])?;
        let mut out = vec![];
        for (head, body) in documents {
            let r = self.resolve(&head.kind, Some(&head.api_version)).await?;
            let ns = head
                .namespace
                .clone()
                .unwrap_or_else(|| self.namespace.clone());
            let path = format!(
                "{}/{}?fieldManager=obleth-chat&force=true",
                Self::collection(&r, Some(&ns)),
                urlencode(&head.name)
            );
            let (status, _) = self
                .send(reqwest::Method::PATCH, &path, Some(Body::Apply(body)))
                .await
                .map_err(|e| format!("{} {}: {e}", head.kind, head.name))?;
            out.push(format!(
                "{} {} {}{}",
                head.kind,
                head.name,
                if status == 201 {
                    "created"
                } else {
                    "configured"
                },
                if r.namespaced {
                    format!(" in {ns}")
                } else {
                    String::new()
                }
            ));
        }
        Ok(out.join("\n"))
    }

    async fn delete(&mut self, args: &Value) -> Result<String, String> {
        let r = self
            .resolve(
                args["kind"].as_str().unwrap_or(""),
                args["api_version"].as_str(),
            )
            .await?;
        let name = args["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or("name is required")?;
        let ns = self.namespace_of(args);
        self.send(
            reqwest::Method::DELETE,
            &format!("{}/{}", Self::collection(&r, Some(&ns)), urlencode(name)),
            Some(Body::Json(json!({"propagationPolicy":"Background"}))),
        )
        .await?;
        Ok(format!(
            "Deleting {} {name}{}.",
            r.kind,
            if r.namespaced {
                format!(" in {ns}")
            } else {
                String::new()
            }
        ))
    }
}

impl Client {
    /// Identity, quota use, pods, workloads and (when allowed) nodes, for the Clusters
    /// page. A section the account may not read is {"error": ...}.
    pub async fn status(&self) -> Result<Value, String> {
        let identity = self.check().await?;
        let ns = self.namespace.clone();
        let get = |path: String| async move {
            self.json(reqwest::Method::GET, &path, None)
                .await
                .map(|(_, v)| v)
        };
        let (version, quotas, pods, deployments, jobs, nodes, rules) = futures_util::join!(
            self.reachable(),
            get(format!("api/v1/namespaces/{ns}/resourcequotas")),
            get(format!("api/v1/namespaces/{ns}/pods")),
            get(format!("apis/apps/v1/namespaces/{ns}/deployments")),
            get(format!("apis/batch/v1/namespaces/{ns}/jobs")),
            get("api/v1/nodes".into()),
            self.json(reqwest::Method::POST, "apis/authorization.k8s.io/v1/selfsubjectrulesreviews", Some(Body::Json(json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectRulesReview","spec":{"namespace":ns}})))),
        );
        let now = chrono::Utc::now();
        let items = |v: &Value| v["items"].as_array().cloned().unwrap_or_default();
        let section = |result: Result<Value, String>, map: &dyn Fn(&Value) -> Value| match result {
            Ok(v) => map(&v),
            Err(e) if e.contains("(HTTP 403)") => {
                json!({"error":"Your account may not read this."})
            }
            Err(e) => json!({"error": e}),
        };
        let quota = section(quotas, &|v| {
            let mut rows = vec![];
            for q in items(v) {
                for (resource, hard) in q["status"]["hard"].as_object().into_iter().flatten() {
                    let used = &q["status"]["used"][resource];
                    let ratio = match (quantity(used), quantity(hard)) {
                        (Some(u), Some(h)) if h > 0.0 => Some((u / h).min(1.0)),
                        _ => None,
                    };
                    rows.push(json!({"resource":resource,"used":used.as_str().unwrap_or("0"),"hard":hard,"ratio":ratio}));
                }
            }
            Value::Array(rows)
        });
        let pods = section(pods, &|v| {
            Value::Array(items(v).iter().take(100).map(|p| {
                let statuses = p["status"]["containerStatuses"].as_array().cloned().unwrap_or_default();
                let ready = statuses.iter().filter(|c| c["ready"] == json!(true)).count();
                let waiting = statuses.iter().find_map(|c| c["state"]["waiting"]["reason"].as_str());
                json!({
                    "name": p["metadata"]["name"],
                    "phase": waiting.unwrap_or(p["status"]["phase"].as_str().unwrap_or("Unknown")),
                    "ready": format!("{ready}/{}", statuses.len().max(p["spec"]["containers"].as_array().map_or(0, Vec::len))),
                    "restarts": statuses.iter().map(|c| c["restartCount"].as_i64().unwrap_or(0)).sum::<i64>(),
                    "age": age(p, now),
                })
            }).collect())
        });
        let deployments = section(deployments, &|v| {
            Value::Array(items(v).iter().take(50).map(|d| json!({
                "name": d["metadata"]["name"],
                "ready": format!("{}/{}", d["status"]["readyReplicas"].as_i64().unwrap_or(0), d["spec"]["replicas"].as_i64().unwrap_or(0)),
                "age": age(d, now),
            })).collect())
        });
        let jobs = section(jobs, &|v| {
            Value::Array(
                items(v)
                    .iter()
                    .take(50)
                    .map(|j| {
                        let s = &j["status"];
                        let state = if s["succeeded"].as_i64().unwrap_or(0) > 0
                            && s["active"].as_i64().unwrap_or(0) == 0
                        {
                            "Complete"
                        } else if s["failed"].as_i64().unwrap_or(0) > 0
                            && s["active"].as_i64().unwrap_or(0) == 0
                        {
                            "Failed"
                        } else {
                            "Running"
                        };
                        json!({"name": j["metadata"]["name"], "state": state, "age": age(j, now)})
                    })
                    .collect(),
            )
        });
        let nodes = section(nodes, &|v| {
            Value::Array(items(v).iter().take(100).map(|n| {
                let ready = n["status"]["conditions"].as_array().into_iter().flatten()
                    .any(|c| c["type"] == "Ready" && c["status"] == "True");
                let roles: Vec<String> = n["metadata"]["labels"].as_object().into_iter().flatten()
                    .filter_map(|(k, _)| k.strip_prefix("node-role.kubernetes.io/").map(str::to_owned))
                    .collect();
                json!({
                    "name": n["metadata"]["name"], "ready": ready, "roles": roles.join(","),
                    "cpu": n["status"]["allocatable"]["cpu"], "memory": n["status"]["allocatable"]["memory"],
                    "version": n["status"]["nodeInfo"]["kubeletVersion"],
                })
            }).collect())
        });
        let allowed: Vec<Value> = rules
            .map(|(_, r)| {
                r["status"]["resourceRules"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default()
            .iter()
            .filter(|r| {
                !r["resources"].as_array().is_some_and(|a| {
                    a.iter()
                        .all(|x| x.as_str().is_some_and(|s| s.starts_with("selfsubject")))
                })
            })
            .take(25)
            .map(|r| {
                let list = |key: &str| {
                    r[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                json!({"resources": list("resources"), "verbs": list("verbs")})
            })
            .collect();
        Ok(json!({
            "identity": identity, "namespace": ns,
            "homeNamespace": home_namespace(&identity),
            "version": version.ok().flatten(),
            "quota": quota, "pods": pods, "deployments": deployments, "jobs": jobs, "nodes": nodes,
            "allowed": allowed,
        }))
    }
}

/// A Kubernetes quantity such as 500m, 4Gi or 2 as a number.
fn quantity(value: &Value) -> Option<f64> {
    let text = match value {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => return n.as_f64(),
        _ => return None,
    };
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, suffix) = text.split_at(split);
    let factor = match suffix {
        "" => 1.0,
        "m" => 1e-3,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "Ki" => 1024.0,
        "Mi" => 1024f64.powi(2),
        "Gi" => 1024f64.powi(3),
        "Ti" => 1024f64.powi(4),
        _ => return None,
    };
    number.parse::<f64>().ok().map(|n| n * factor)
}

fn urlencode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn age(item: &Value, now: chrono::DateTime<chrono::Utc>) -> String {
    let Some(created) = item["metadata"]["creationTimestamp"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    else {
        return "-".into();
    };
    let s = (now - created.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    match s {
        s if s >= 86400 => format!("{}d", s / 86400),
        s if s >= 3600 => format!("{}h", s / 3600),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// One line per object, with the status fields that matter for its kind.
fn row(
    kind: &str,
    item: &Value,
    now: chrono::DateTime<chrono::Utc>,
    with_namespace: bool,
) -> String {
    let n = |v: &Value| v.as_i64().unwrap_or(0);
    let s = &item["status"];
    let detail = match kind {
        "Pod" => {
            let statuses = s["containerStatuses"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let ready = statuses
                .iter()
                .filter(|c| c["ready"] == json!(true))
                .count();
            let restarts: i64 = statuses.iter().map(|c| n(&c["restartCount"])).sum();
            let waiting = statuses
                .iter()
                .find_map(|c| c["state"]["waiting"]["reason"].as_str());
            format!(
                "{} {ready}/{} ready, {restarts} restarts, node {}",
                waiting.unwrap_or(s["phase"].as_str().unwrap_or("?")),
                statuses.len(),
                item["spec"]["nodeName"].as_str().unwrap_or("-")
            )
        }
        "Deployment" | "StatefulSet" | "ReplicaSet" => format!(
            "{}/{} ready",
            n(&s["readyReplicas"]),
            n(&item["spec"]["replicas"])
        ),
        "DaemonSet" => format!(
            "{}/{} ready",
            n(&s["numberReady"]),
            n(&s["desiredNumberScheduled"])
        ),
        "Job" => format!(
            "{} succeeded, {} failed, {} active",
            n(&s["succeeded"]),
            n(&s["failed"]),
            n(&s["active"])
        ),
        "CronJob" => format!(
            "schedule {}, last run {}",
            item["spec"]["schedule"].as_str().unwrap_or("?"),
            s["lastScheduleTime"].as_str().unwrap_or("never")
        ),
        "Service" => {
            let ports: Vec<String> = item["spec"]["ports"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| {
                    format!(
                        "{}/{}",
                        n(&p["port"]),
                        p["protocol"].as_str().unwrap_or("TCP")
                    )
                })
                .collect();
            format!(
                "{} {} {}",
                item["spec"]["type"].as_str().unwrap_or(""),
                item["spec"]["clusterIP"].as_str().unwrap_or(""),
                ports.join(",")
            )
        }
        "Event" => format!(
            "{} {} {}/{}: {} (x{})",
            item["type"].as_str().unwrap_or(""),
            item["reason"].as_str().unwrap_or(""),
            item["involvedObject"]["kind"].as_str().unwrap_or(""),
            item["involvedObject"]["name"].as_str().unwrap_or(""),
            item["message"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>(),
            n(&item["count"]).max(1)
        ),
        "Node" => {
            let ready = s["conditions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["type"] == "Ready")
                .map_or("Unknown", |c| {
                    if c["status"] == "True" {
                        "Ready"
                    } else {
                        "NotReady"
                    }
                });
            format!(
                "{ready}, {} CPUs, {} memory",
                s["allocatable"]["cpu"].as_str().unwrap_or("?"),
                s["allocatable"]["memory"].as_str().unwrap_or("?")
            )
        }
        "PersistentVolumeClaim" => format!(
            "{} {}",
            s["phase"].as_str().unwrap_or(""),
            s["capacity"]["storage"].as_str().unwrap_or("")
        ),
        _ => String::new(),
    };
    let name = item["metadata"]["name"].as_str().unwrap_or("?");
    let ns = item["metadata"]["namespace"]
        .as_str()
        .filter(|_| with_namespace)
        .map(|n| format!("{n}/"))
        .unwrap_or_default();
    format!(
        "- {ns}{name} ({}){}",
        age(item, now),
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    )
}

#[derive(Debug, PartialEq)]
struct Head {
    api_version: String,
    kind: String,
    name: String,
    namespace: Option<String>,
}

/// The manifests to apply, each with what is needed to address it and its body.
fn documents(manifest: &Value) -> Result<Vec<(Head, String)>, String> {
    let mut docs = vec![];
    match manifest {
        Value::Array(items) => {
            for item in items {
                docs.extend(documents(item)?);
            }
        }
        Value::Object(_) => docs.push((head_of_json(manifest)?, manifest.to_string())),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(value @ (Value::Object(_) | Value::Array(_))) => docs.extend(documents(&value)?),
            _ => {
                for doc in text
                    .split("\n---")
                    .map(|d| d.trim_start_matches("---").trim())
                {
                    if doc
                        .lines()
                        .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                    {
                        docs.push((head_of_yaml(doc)?, doc.to_owned()));
                    }
                }
            }
        },
        _ => return Err("manifest must be an object, a list of objects or YAML text".into()),
    }
    if docs.is_empty() || docs.len() > 20 {
        return Err("Apply between 1 and 20 manifests at a time".into());
    }
    Ok(docs)
}

fn head_of_json(object: &Value) -> Result<Head, String> {
    let field = |v: &Value, what: &str| {
        v.as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or(format!("The manifest needs {what}"))
    };
    Ok(Head {
        api_version: field(&object["apiVersion"], "apiVersion")?,
        kind: field(&object["kind"], "kind")?,
        name: field(&object["metadata"]["name"], "metadata.name")?,
        namespace: object["metadata"]["namespace"].as_str().map(str::to_owned),
    })
}

/// apiVersion, kind and metadata name/namespace from YAML text; the API server
/// parses the rest.
fn head_of_yaml(doc: &str) -> Result<Head, String> {
    let value = |line: &str| {
        line.split_once(':').map(|(_, v)| {
            v.split(" #")
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(['"', '\''])
                .to_owned()
        })
    };
    let (mut api_version, mut kind, mut name, mut namespace) = (None, None, None, None);
    let mut in_metadata = false;
    let mut child: Option<usize> = None;
    for line in doc.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            in_metadata = line.starts_with("metadata:");
            if line.starts_with("apiVersion:") {
                api_version = value(line);
            } else if line.starts_with("kind:") {
                kind = value(line);
            }
            continue;
        }
        if in_metadata {
            let level = *child.get_or_insert(indent);
            if indent == level {
                let key = line.trim_start();
                if key.starts_with("name:") {
                    name = value(key);
                } else if key.starts_with("namespace:") {
                    namespace = value(key);
                }
            }
        }
    }
    let need = |v: Option<String>, what: &str| {
        v.filter(|s| !s.is_empty())
            .ok_or(format!("A YAML document is missing {what}"))
    };
    Ok(Head {
        api_version: need(api_version, "apiVersion")?,
        kind: need(kind, "kind")?,
        name: need(name, "metadata.name")?,
        namespace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_read_every_unit() {
        assert_eq!(quantity(&json!("500m")), Some(0.5));
        assert_eq!(quantity(&json!("2")), Some(2.0));
        assert_eq!(quantity(&json!("4Gi")), Some(4.0 * 1024f64.powi(3)));
        assert_eq!(quantity(&json!("1.5k")), Some(1500.0));
        assert_eq!(quantity(&json!("lots")), None);
    }

    #[test]
    fn service_accounts_have_a_home_namespace() {
        assert_eq!(
            home_namespace("system:serviceaccount:chat-playground:demo").as_deref(),
            Some("chat-playground")
        );
        assert_eq!(home_namespace("alice"), None);
        assert_eq!(home_namespace("system:serviceaccount:team"), None);
    }

    #[test]
    fn yaml_heads_are_read_without_a_yaml_parser() {
        let docs = documents(&json!("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web # the site\n  labels:\n    name: not-this\n  namespace: \"demo\"\nspec:\n  replicas: 1\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: web\n")).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(
            docs[0].0,
            Head {
                api_version: "apps/v1".into(),
                kind: "Deployment".into(),
                name: "web".into(),
                namespace: Some("demo".into())
            }
        );
        assert_eq!(docs[1].0.namespace, None);
        assert!(docs[1].1.starts_with("apiVersion: v1"));
        let docs =
            documents(&json!([{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"a"}}]))
                .unwrap();
        assert_eq!(docs[0].0.kind, "ConfigMap");
        assert!(documents(&json!("kind: Pod\nmetadata:\n  name: x\n")).is_err());
        assert!(documents(&json!(42)).is_err());
    }

    #[test]
    fn rows_summarize_status() {
        let now = chrono::Utc::now();
        let pod = json!({"metadata":{"name":"web-1","namespace":"demo","creationTimestamp":(now - chrono::Duration::minutes(5)).to_rfc3339()},"spec":{"nodeName":"node-2"},"status":{"phase":"Running","containerStatuses":[{"ready":true,"restartCount":2,"state":{}}]}});
        assert_eq!(
            row("Pod", &pod, now, false),
            "- web-1 (5m): Running 1/1 ready, 2 restarts, node node-2"
        );
        let crash = json!({"metadata":{"name":"bad"},"status":{"phase":"Running","containerStatuses":[{"ready":false,"restartCount":7,"state":{"waiting":{"reason":"CrashLoopBackOff"}}}]}});
        assert!(row("Pod", &crash, now, true).contains("CrashLoopBackOff 0/1 ready, 7 restarts"));
        assert_eq!(
            tools()
                .iter()
                .filter(|t| !t.read_only)
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["apply", "delete"]
        );
    }
}
