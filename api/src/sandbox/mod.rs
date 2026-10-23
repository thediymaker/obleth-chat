//! Kubernetes workspaces attached to conversations. Each user gets one pod and one
//! volume in a dedicated namespace; each conversation's workspace is a project
//! folder in it with its own dev server port and preview origin. Agent tools run
//! through exec.
pub mod kube;
pub mod preview;
pub mod terminal;
pub mod tools;

use crate::{auth::Identity, error::ApiError, AppState};
use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use std::{env, sync::Arc, time::Duration};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use kube::Kube;

#[derive(Clone)]
pub struct SandboxConfig {
    pub namespace: String,
    pub image: String,
    pub runtime_class: Option<String>,
    pub storage_class: Option<String>,
    pub storage: String,
    /// `storage` in bytes: the most a user's workspace may hold.
    pub disk_limit: u64,
    pub cpu_limit: String,
    pub memory_limit: String,
    pub everyone: bool,
    pub preview_domain: String,
    pub preview_scheme: String,
    pub chat_origin: String,
    pub idle: Duration,
    pub retention: Duration,
}

impl SandboxConfig {
    /// Workspaces are enabled by OBLETH_CHAT_SANDBOX_NAMESPACE; the image and the
    /// preview domain are then required.
    pub fn from_env() -> Result<Option<Self>> {
        let var = |key: &str| env::var(key).ok().filter(|v| !v.trim().is_empty());
        let Some(namespace) = var("OBLETH_CHAT_SANDBOX_NAMESPACE") else {
            return Ok(None);
        };
        let image = var("OBLETH_CHAT_SANDBOX_IMAGE").context("Set OBLETH_CHAT_SANDBOX_IMAGE")?;
        let preview_domain = var("OBLETH_CHAT_PREVIEW_DOMAIN")
            .context("Set OBLETH_CHAT_PREVIEW_DOMAIN to the wildcard preview domain")?
            .to_ascii_lowercase();
        if !preview_domain.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }) || !preview_domain.contains('.')
        {
            bail!("OBLETH_CHAT_PREVIEW_DOMAIN must be a domain such as sbx.example.com");
        }
        let chat_origin =
            var("OBLETH_CHAT_ORIGIN").unwrap_or_else(|| "http://localhost:8085".into());
        let origin = Url::parse(&chat_origin).context("Invalid OBLETH_CHAT_ORIGIN")?;
        if origin.host_str().is_some_and(|host| {
            host == preview_domain || host.ends_with(&format!(".{preview_domain}"))
        }) {
            bail!("The preview domain must not contain the Chat origin");
        }
        let minutes = |key: &str, default: u64| -> Result<Duration> {
            let value = var(key)
                .map(|v| v.parse::<u64>())
                .transpose()
                .with_context(|| format!("Invalid {key}"))?
                .unwrap_or(default);
            if !(5..=60 * 24 * 90).contains(&value) {
                bail!("{key} must be between 5 and 129600 minutes");
            }
            Ok(Duration::from_secs(value * 60))
        };
        let everyone = match var("OBLETH_CHAT_SANDBOX_ACCESS").as_deref() {
            None | Some("admins") => false,
            Some("everyone") => true,
            Some(_) => bail!("OBLETH_CHAT_SANDBOX_ACCESS must be admins or everyone"),
        };
        let storage = var("OBLETH_CHAT_SANDBOX_STORAGE").unwrap_or_else(|| "10Gi".into());
        let disk_limit = quantity_bytes(&storage)
            .context("OBLETH_CHAT_SANDBOX_STORAGE must be a size such as 10Gi")?;
        Ok(Some(Self {
            namespace,
            image,
            runtime_class: var("OBLETH_CHAT_SANDBOX_RUNTIME_CLASS"),
            storage_class: var("OBLETH_CHAT_SANDBOX_STORAGE_CLASS"),
            storage,
            disk_limit,
            cpu_limit: var("OBLETH_CHAT_SANDBOX_CPU").unwrap_or_else(|| "2".into()),
            memory_limit: var("OBLETH_CHAT_SANDBOX_MEMORY").unwrap_or_else(|| "4Gi".into()),
            everyone,
            preview_scheme: if origin.scheme() == "https" {
                "https"
            } else {
                "http"
            }
            .into(),
            chat_origin: origin.origin().ascii_serialization(),
            preview_domain,
            idle: minutes("OBLETH_CHAT_SANDBOX_IDLE_MINUTES", 60)?,
            retention: minutes("OBLETH_CHAT_SANDBOX_RETENTION_MINUTES", 60 * 24 * 7)?,
        }))
    }
}

/// A conversation's workspace: a project folder in its owner's pod. Status, address
/// and errors belong to the pod.
#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Sandbox {
    pub id: Uuid,
    pub conversation_id: Uuid,
    #[serde(skip)]
    pub owner_id: Uuid,
    /// Project folder name and preview host label.
    #[serde(skip)]
    pub slug: String,
    #[serde(skip)]
    pub pod_id: Uuid,
    #[serde(skip)]
    pub pod_slug: String,
    pub status: String,
    #[serde(skip)]
    pub pod_ip: Option<String>,
    /// The port Chat gives this project's dev server.
    #[serde(skip)]
    pub port: i32,
    pub preview_port: Option<i32>,
    pub error_message: Option<String>,
    /// Size of the owner's whole workspace volume when last measured.
    pub disk_bytes: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
}

impl Sandbox {
    pub fn pod_name(&self) -> String {
        pod_name(&self.pod_slug)
    }
    pub fn dir(&self) -> String {
        format!("/workspace/{}", self.slug)
    }
}

fn pod_name(slug: &str) -> String {
    format!("sbx-{slug}")
}

fn new_slug() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .take(16)
        .map(char::from)
        .collect()
}

const SELECT: &str = "SELECT s.id,s.conversation_id,s.owner_id,s.slug,s.pod_id,p.slug AS pod_slug,p.status,p.pod_ip,s.port,s.preview_port,p.error_message,p.disk_bytes,s.created_at,s.last_used_at FROM sandboxes s JOIN sandbox_pods p ON p.id=s.pod_id";
/// How often a running workspace's disk use is measured.
const DISK_CHECK: Duration = Duration::from_secs(5 * 60);
/// Dev servers of projects unused this long are stopped; the pod keeps running.
const SERVER_IDLE: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct Sandboxes {
    inner: Arc<Inner>,
}
struct Inner {
    config: SandboxConfig,
    kube: Kube,
    pool: PgPool,
    http: reqwest::Client,
    key: OnceCell<Vec<u8>>,
}

impl Sandboxes {
    pub fn new(config: SandboxConfig, pool: PgPool) -> Result<Self> {
        let kube = Kube::from_env(config.namespace.clone())?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                kube,
                pool,
                http,
                key: OnceCell::new(),
            }),
        })
    }
    pub fn config(&self) -> &SandboxConfig {
        &self.inner.config
    }
    /// Whether the owner's workspace holds more than its storage size.
    pub fn full(&self, sandbox: &Sandbox) -> bool {
        sandbox
            .disk_bytes
            .is_some_and(|used| used as u64 > self.inner.config.disk_limit)
    }

    /// Measures the owner's whole workspace volume and stores the result.
    pub async fn measure(&self, sandbox: &Sandbox) -> Option<i64> {
        let script = "du -sk /workspace 2>/dev/null | cut -f1";
        let output = tokio::time::timeout(
            Duration::from_secs(120),
            self.exec(sandbox, &["sh", "-c", script], None),
        )
        .await
        .ok()?
        .ok()?;
        let bytes = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<i64>()
            .ok()?
            .saturating_mul(1024);
        let _ =
            sqlx::query("UPDATE sandbox_pods SET disk_bytes=$2,disk_checked_at=NOW() WHERE id=$1")
                .bind(sandbox.pod_id)
                .bind(bytes)
                .execute(&self.inner.pool)
                .await;
        Some(bytes)
    }

    pub fn allowed(&self, user: &Identity) -> bool {
        self.inner.config.everyone || user.is_admin()
    }
    pub fn preview_origin(&self, sandbox: &Sandbox) -> String {
        let c = &self.inner.config;
        format!(
            "{}://{}.{}",
            c.preview_scheme, sandbox.slug, c.preview_domain
        )
    }

    pub async fn get(&self, owner: Uuid, conversation: Uuid) -> Result<Option<Sandbox>, ApiError> {
        Ok(sqlx::query_as::<_, Sandbox>(&format!(
            "{SELECT} WHERE s.conversation_id=$1 AND s.owner_id=$2"
        ))
        .bind(conversation)
        .bind(owner)
        .fetch_optional(&self.inner.pool)
        .await?)
    }
    /// A person's projects with their conversation titles, most recently used first.
    pub async fn list(&self, owner: Uuid) -> Result<Vec<(Sandbox, String)>, ApiError> {
        let projects = sqlx::query_as::<_, Sandbox>(&format!(
            "{SELECT} WHERE s.owner_id=$1 ORDER BY s.last_used_at DESC"
        ))
        .bind(owner)
        .fetch_all(&self.inner.pool)
        .await?;
        let ids: Vec<Uuid> = projects.iter().map(|p| p.conversation_id).collect();
        let titles: std::collections::HashMap<Uuid, String> =
            sqlx::query_as("SELECT id,title FROM conversations WHERE owner_id=$1 AND id=ANY($2)")
                .bind(owner)
                .bind(&ids)
                .fetch_all(&self.inner.pool)
                .await?
                .into_iter()
                .collect();
        Ok(projects
            .into_iter()
            .map(|p| {
                let title = titles.get(&p.conversation_id).cloned().unwrap_or_default();
                (p, title)
            })
            .collect())
    }
    async fn by_id(&self, id: Uuid) -> Result<Option<Sandbox>, ApiError> {
        Ok(
            sqlx::query_as::<_, Sandbox>(&format!("{SELECT} WHERE s.id=$1"))
                .bind(id)
                .fetch_optional(&self.inner.pool)
                .await?,
        )
    }
    pub async fn by_slug(&self, slug: &str) -> Result<Option<Sandbox>, ApiError> {
        Ok(
            sqlx::query_as::<_, Sandbox>(&format!("{SELECT} WHERE s.slug=$1"))
                .bind(slug)
                .fetch_optional(&self.inner.pool)
                .await?,
        )
    }

    /// Creates the workspace for an owned conversation (idempotent): the owner's pod
    /// record if needed and a project with a free port, then starts the pod and the
    /// project's dev server in the background.
    pub async fn create(&self, owner: Uuid, conversation: Uuid) -> Result<Sandbox, ApiError> {
        let owned: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM conversations WHERE id=$1 AND owner_id=$2")
                .bind(conversation)
                .bind(owner)
                .fetch_optional(&self.inner.pool)
                .await?;
        if owned.is_none() {
            return Err(ApiError::not_found());
        }
        let pool = &self.inner.pool;
        sqlx::query("INSERT INTO sandbox_pods(id,owner_id,slug) VALUES($1,$2,$3) ON CONFLICT (owner_id) DO NOTHING")
            .bind(Uuid::new_v4())
            .bind(owner)
            .bind(new_slug())
            .execute(pool)
            .await?;
        let pod: Uuid = sqlx::query_scalar("SELECT id FROM sandbox_pods WHERE owner_id=$1")
            .bind(owner)
            .fetch_one(pool)
            .await?;
        // A concurrent create can take the same port; the unique constraint makes it retry.
        let mut sandbox = self.get(owner, conversation).await?;
        for _ in 0..5 {
            if sandbox.is_some() {
                break;
            }
            let port: Option<i32> = sqlx::query_scalar("SELECT p FROM generate_series(3000,3999) p WHERE NOT EXISTS (SELECT 1 FROM sandboxes WHERE pod_id=$1 AND port=p) ORDER BY p LIMIT 1")
                .bind(pod)
                .fetch_optional(pool)
                .await?;
            let Some(port) = port else {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "You have too many workspaces. Delete some to create more.".into(),
                ));
            };
            sqlx::query("INSERT INTO sandboxes(id,conversation_id,owner_id,slug,pod_id,port) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
                .bind(Uuid::new_v4())
                .bind(conversation)
                .bind(owner)
                .bind(new_slug())
                .bind(pod)
                .bind(port)
                .execute(pool)
                .await?;
            sandbox = self.get(owner, conversation).await?;
        }
        let sandbox = sandbox.ok_or_else(|| {
            ApiError(
                StatusCode::CONFLICT,
                "The workspace could not be created. Try again.".into(),
            )
        })?;
        let this = self.clone();
        let starting = sandbox.clone();
        tokio::spawn(async move {
            if let Ok(running) = this.ensure_running(&starting).await {
                if let Err(error) = tools::ensure_dev_server(&this, &running).await {
                    tracing::warn!("could not restart the workspace server: {error}");
                }
            }
        });
        Ok(sandbox)
    }

    /// Returns a running sandbox with a current pod address, creating or replacing
    /// the owner's pod as needed. The volume keeps /workspace across pod restarts.
    pub async fn ensure_running(&self, sandbox: &Sandbox) -> Result<Sandbox, String> {
        let result = self.start(sandbox).await;
        let pool = &self.inner.pool;
        match result {
            Ok((ip, created)) => {
                let _ = sqlx::query("UPDATE sandbox_pods SET status='running',pod_ip=$2,error_message=NULL,last_used_at=NOW() WHERE id=$1")
                    .bind(sandbox.pod_id).bind(&ip).execute(pool).await;
                let current = self
                    .by_id(sandbox.id)
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| "The workspace was deleted".to_string())?;
                if created {
                    self.prune(&current).await;
                }
                Ok(current)
            }
            Err(error) => {
                let message: String = format!("{error:#}").chars().take(500).collect();
                tracing::warn!(sandbox=%sandbox.id, "workspace did not start: {message}");
                let message = if message.contains("exceeded quota") {
                    "All workspace slots are in use right now. Try again in a few minutes.".into()
                } else {
                    message
                };
                let _ = sqlx::query(
                    "UPDATE sandbox_pods SET status='error',error_message=$2 WHERE id=$1",
                )
                .bind(sandbox.pod_id)
                .bind(&message)
                .execute(pool)
                .await;
                Err(format!("The workspace could not start: {message}"))
            }
        }
    }

    /// Removes project folders whose conversation was deleted while the pod was away.
    async fn prune(&self, sandbox: &Sandbox) {
        let known: Vec<String> =
            match sqlx::query_scalar("SELECT slug FROM sandboxes WHERE pod_id=$1")
                .bind(sandbox.pod_id)
                .fetch_all(&self.inner.pool)
                .await
            {
                Ok(known) => known,
                Err(_) => return,
            };
        let script = "cd /workspace || exit 0; for d in *; do \
            case \"$d\" in *[!a-z0-9]*) continue;; esac; [ ${#d} -eq 16 ] && [ -d \"$d\" ] || continue; \
            case \" $1 \" in *\" $d \"*) ;; *) rm -rf -- \"$d\";; esac; done; \
            for r in .obleth-checkpoints/*.git; do [ -d \"$r\" ] || continue; d=${r#.obleth-checkpoints/}; d=${d%.git}; \
            case \" $1 \" in *\" $d \"*) ;; *) rm -rf -- \"$r\";; esac; done";
        let known = known.join(" ");
        if let Err(error) = self
            .exec(sandbox, &["sh", "-c", script, "sh", &known], None)
            .await
        {
            tracing::warn!("could not prune workspace folders: {error:#}");
        }
    }

    /// Returns the pod address, and whether this call created the pod.
    async fn start(&self, sandbox: &Sandbox) -> Result<(String, bool)> {
        let kube = &self.inner.kube;
        let name = sandbox.pod_name();
        if let Some(pod) = kube.get("pods", &name).await? {
            match pod_state(&pod) {
                PodState::Ready(ip) => return Ok((ip, false)),
                PodState::Finished | PodState::Deleting => {
                    kube.delete("pods", &name).await?;
                    self.wait_gone(&name).await?;
                }
                PodState::Pending => {}
            }
        }
        sqlx::query("UPDATE sandbox_pods SET status='starting',error_message=NULL WHERE id=$1 AND status<>'starting'")
            .bind(sandbox.pod_id)
            .execute(&self.inner.pool)
            .await?;
        kube.create(
            "persistentvolumeclaims",
            &self.volume_manifest(&sandbox.pod_slug),
        )
        .await?;
        kube.create("pods", &self.pod_manifest(&sandbox.pod_slug))
            .await?;
        for _ in 0..180 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(pod) = kube.get("pods", &name).await? else {
                bail!("the workspace pod disappeared while starting");
            };
            match pod_state(&pod) {
                PodState::Ready(ip) => return Ok((ip, true)),
                PodState::Finished => bail!("the workspace pod exited: {}", waiting_reason(&pod)),
                _ => {
                    let reason = waiting_reason(&pod);
                    if matches!(
                        reason.as_str(),
                        "ErrImagePull"
                            | "ImagePullBackOff"
                            | "InvalidImageName"
                            | "CreateContainerConfigError"
                    ) {
                        bail!("the workspace image could not start ({reason})");
                    }
                }
            }
        }
        bail!("the workspace pod was not ready after three minutes")
    }

    async fn wait_gone(&self, name: &str) -> Result<()> {
        for _ in 0..60 {
            if self.inner.kube.get("pods", name).await?.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!("the previous workspace pod did not stop")
    }

    /// obleth.io/pod names the owner's pod record. Objects that carry the earlier
    /// per-conversation obleth.io/sandbox label instead are retired by the reconciler.
    fn labels(slug: &str) -> Value {
        json!({"app.kubernetes.io/managed-by":"obleth-chat","app.kubernetes.io/name":"obleth-sandbox","obleth.io/pod":slug})
    }

    fn volume_manifest(&self, slug: &str) -> Value {
        let c = &self.inner.config;
        let mut spec =
            json!({"accessModes":["ReadWriteOnce"],"resources":{"requests":{"storage":c.storage}}});
        if let Some(class) = &c.storage_class {
            spec["storageClassName"] = json!(class);
        }
        json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":pod_name(slug),"labels":Self::labels(slug)},"spec":spec})
    }

    /// Restricted Pod Security profile: non-root, no privilege escalation, no
    /// capabilities, default seccomp, and no service account token.
    fn pod_manifest(&self, slug: &str) -> Value {
        let c = &self.inner.config;
        let mut spec = json!({
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            "hostname": "workspace",
            "restartPolicy": "Always",
            // The pause container becomes PID 1 and reaps exited dev servers.
            "shareProcessNamespace": true,
            "terminationGracePeriodSeconds": 5,
            "securityContext": {"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,"fsGroup":1000,"seccompProfile":{"type":"RuntimeDefault"}},
            "containers": [{
                "name": "sandbox",
                "image": c.image,
                "imagePullPolicy": "IfNotPresent",
                "command": ["sleep", "infinity"],
                "workingDir": "/workspace",
                "env": [
                    {"name":"HOME","value":"/home/node"},
                    {"name":"NEXT_TELEMETRY_DISABLED","value":"1"}
                ],
                "resources": {
                    "requests": {"cpu":"250m","memory":"512Mi","ephemeral-storage":"1Gi"},
                    "limits": {"cpu":c.cpu_limit,"memory":c.memory_limit,"ephemeral-storage":"4Gi"}
                },
                "securityContext": {"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]},"runAsNonRoot":true},
                "volumeMounts": [{"name":"workspace","mountPath":"/workspace"}]
            }],
            "volumes": [{"name":"workspace","persistentVolumeClaim":{"claimName":pod_name(slug)}}]
        });
        if let Some(class) = &c.runtime_class {
            spec["runtimeClassName"] = json!(class);
        }
        json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":pod_name(slug),"labels":Self::labels(slug)},"spec":spec})
    }

    pub async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[&str],
        stdin: Option<&[u8]>,
    ) -> Result<kube::ExecOutput> {
        self.inner.kube.exec(&sandbox.pod_name(), argv, stdin).await
    }

    pub async fn exec_socket(&self, sandbox: &Sandbox, argv: &[&str]) -> Result<kube::ExecSocket> {
        self.inner
            .kube
            .exec_socket(&sandbox.pod_name(), argv, true, true)
            .await
    }

    pub async fn exec_limited(
        &self,
        sandbox: &Sandbox,
        argv: &[&str],
        limit: usize,
    ) -> Result<kube::ExecOutput> {
        self.inner
            .kube
            .exec_limited(&sandbox.pod_name(), argv, None, limit)
            .await
    }

    /// Runs `argv` with optional input, keeping up to `limit` bytes per stream.
    pub async fn exec_with(
        &self,
        sandbox: &Sandbox,
        argv: &[&str],
        stdin: Option<&[u8]>,
        limit: usize,
    ) -> Result<kube::ExecOutput> {
        self.inner
            .kube
            .exec_limited(&sandbox.pod_name(), argv, stdin, limit)
            .await
    }

    pub async fn touch(&self, sandbox: &Sandbox) {
        let _ = sqlx::query("WITH s AS (UPDATE sandboxes SET last_used_at=NOW() WHERE id=$1 AND last_used_at<NOW()-INTERVAL '30 seconds' RETURNING pod_id) UPDATE sandbox_pods SET last_used_at=NOW() WHERE id IN (SELECT pod_id FROM s)")
            .bind(sandbox.id)
            .execute(&self.inner.pool)
            .await;
    }

    pub async fn set_preview(&self, sandbox: &Sandbox, port: Option<u16>) -> Result<(), ApiError> {
        sqlx::query("UPDATE sandboxes SET preview_port=$2 WHERE id=$1")
            .bind(sandbox.id)
            .bind(port.map(i32::from))
            .execute(&self.inner.pool)
            .await?;
        Ok(())
    }

    /// Deletes a conversation's project, and the owner's pod and volume with it when
    /// it was the last one.
    pub async fn destroy(&self, sandbox: &Sandbox) -> Result<()> {
        if sandbox.status == "running" {
            tools::stop_server(self, sandbox).await;
            let script = "cd /workspace && rm -rf -- \"$1\" \".obleth-checkpoints/$1.git\"";
            self.exec(sandbox, &["sh", "-c", script, "sh", &sandbox.slug], None)
                .await?;
        }
        let pool = &self.inner.pool;
        sqlx::query("DELETE FROM sandboxes WHERE id=$1")
            .bind(sandbox.id)
            .execute(pool)
            .await?;
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sandboxes WHERE pod_id=$1")
            .bind(sandbox.pod_id)
            .fetch_one(pool)
            .await?;
        if remaining == 0 {
            self.destroy_pod(sandbox.pod_id, &sandbox.pod_slug).await?;
        }
        Ok(())
    }

    async fn destroy_pod(&self, id: Uuid, slug: &str) -> Result<()> {
        let name = pod_name(slug);
        self.inner.kube.delete("pods", &name).await?;
        self.inner
            .kube
            .delete("persistentvolumeclaims", &name)
            .await?;
        sqlx::query("DELETE FROM sandbox_pods WHERE id=$1")
            .bind(id)
            .execute(&self.inner.pool)
            .await?;
        Ok(())
    }

    /// One replica at a time: stop idle dev servers and idle pods (keeping their
    /// volumes), remove volumes past retention, and delete cluster objects whose
    /// pod record no longer exists.
    async fn reconcile(&self) -> Result<()> {
        let pool = &self.inner.pool;
        let mut tx = pool.begin().await?;
        let locked: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_xact_lock(hashtext('obleth-chat-sandbox-reconcile'))",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !locked {
            return Ok(());
        }
        let kube = &self.inner.kube;
        let idle = self.inner.config.idle.as_secs() as f64;
        let retention = self.inner.config.retention.as_secs() as f64;
        let expired: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id,slug FROM sandbox_pods WHERE last_used_at<NOW()-make_interval(secs=>$1)",
        )
        .bind(retention)
        .fetch_all(&mut *tx)
        .await?;
        for (id, slug) in expired {
            self.destroy_pod(id, &slug).await?;
        }
        let idle_pods: Vec<(Uuid, String)> = sqlx::query_as("SELECT id,slug FROM sandbox_pods WHERE status IN ('running','error') AND last_used_at<NOW()-make_interval(secs=>$1)")
            .bind(idle).fetch_all(&mut *tx).await?;
        for (id, slug) in idle_pods {
            kube.delete("pods", &pod_name(&slug)).await?;
            sqlx::query("UPDATE sandbox_pods SET status='stopped',pod_ip=NULL WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE sandboxes SET preview_port=NULL WHERE pod_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let idle_servers = sqlx::query_as::<_, Sandbox>(&format!("{SELECT} WHERE p.status='running' AND s.preview_port IS NOT NULL AND s.last_used_at<NOW()-make_interval(secs=>$1)"))
            .bind(SERVER_IDLE.as_secs() as f64).fetch_all(&mut *tx).await?;
        for sandbox in idle_servers {
            tools::stop_server(self, &sandbox).await;
            sqlx::query("UPDATE sandboxes SET preview_port=NULL WHERE id=$1")
                .bind(sandbox.id)
                .execute(&mut *tx)
                .await?;
        }
        // One project per running pod stands in for the pod when measuring the volume.
        let unmeasured = sqlx::query_as::<_, Sandbox>(&format!("{SELECT} WHERE s.id IN (SELECT DISTINCT ON (s2.pod_id) s2.id FROM sandboxes s2 JOIN sandbox_pods p2 ON p2.id=s2.pod_id WHERE p2.status='running' AND (p2.disk_checked_at IS NULL OR p2.disk_checked_at<NOW()-make_interval(secs=>$1)) ORDER BY s2.pod_id, s2.last_used_at DESC)"))
            .bind(DISK_CHECK.as_secs() as f64).fetch_all(&mut *tx).await?;
        for sandbox in unmeasured {
            self.measure(&sandbox).await;
        }
        let known: Vec<String> = sqlx::query_scalar("SELECT slug FROM sandbox_pods")
            .fetch_all(&mut *tx)
            .await?;
        for kind in ["pods", "persistentvolumeclaims"] {
            for object in kube
                // Browser pods share the labels for network policy but are not workspaces.
                .list(
                    kind,
                    "app.kubernetes.io/managed-by=obleth-chat,!obleth.io/browser",
                )
                .await?
            {
                let labels = object.pointer("/metadata/labels");
                let slug = labels
                    .and_then(|l| l.get("obleth.io/pod"))
                    .and_then(Value::as_str);
                let name = object.pointer("/metadata/name").and_then(Value::as_str);
                let orphan = slug.is_none_or(|slug| !known.iter().any(|k| k == slug));
                if let (true, Some(name)) = (orphan, name) {
                    kube.delete(kind, name).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn worker(self, shutdown: CancellationToken) {
        loop {
            if let Err(error) = self.reconcile().await {
                tracing::warn!("workspace reconciliation failed: {error:#}");
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                _ = shutdown.cancelled() => break,
            }
        }
    }

    /// HMAC key shared by all replicas, created on first use.
    async fn key(&self) -> Result<&[u8], ApiError> {
        self.inner
            .key
            .get_or_try_init(|| async {
                let mut fresh = vec![0u8; 32];
                rand::thread_rng().fill(&mut fresh[..]);
                sqlx::query("INSERT INTO app_secrets(name,value) VALUES('preview-hmac',$1) ON CONFLICT (name) DO NOTHING")
                    .bind(&fresh)
                    .execute(&self.inner.pool)
                    .await?;
                let key: Vec<u8> = sqlx::query_scalar("SELECT value FROM app_secrets WHERE name='preview-hmac'")
                    .fetch_one(&self.inner.pool)
                    .await?;
                Ok::<_, ApiError>(key)
            })
            .await
            .map(Vec::as_slice)
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }
}

pub(crate) enum PodState {
    Ready(String),
    Pending,
    Finished,
    Deleting,
}

pub(crate) fn pod_state(pod: &Value) -> PodState {
    if pod.pointer("/metadata/deletionTimestamp").is_some() {
        return PodState::Deleting;
    }
    match pod.pointer("/status/phase").and_then(Value::as_str) {
        Some("Succeeded" | "Failed") => PodState::Finished,
        Some("Running") => {
            let ready = pod
                .pointer("/status/containerStatuses/0/ready")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match pod.pointer("/status/podIP").and_then(Value::as_str) {
                Some(ip) if ready => PodState::Ready(ip.to_owned()),
                _ => PodState::Pending,
            }
        }
        _ => PodState::Pending,
    }
}

pub(crate) fn waiting_reason(pod: &Value) -> String {
    pod.pointer("/status/containerStatuses/0/state/waiting/reason")
        .or_else(|| pod.pointer("/status/containerStatuses/0/state/terminated/reason"))
        .or_else(|| pod.pointer("/status/reason"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

fn unavailable() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "Workspaces are not enabled".into())
}

async fn manager(
    s: &AppState,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<(Sandboxes, Identity), ApiError> {
    let user = s.auth.require(headers, mutation).await?;
    let sandboxes = s.sandboxes.clone().ok_or_else(unavailable)?;
    if !sandboxes.allowed(&user) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Workspaces are available to administrators".into(),
        ));
    }
    Ok((sandboxes, user))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/workspaces", get(list))
        .route(
            "/api/conversations/{id}/workspace",
            get(status).post(create).delete(remove),
        )
        .route(
            "/api/conversations/{id}/workspace/preview",
            post(preview_link),
        )
        .route("/api/conversations/{id}/workspace/files", get(files))
        .route("/api/conversations/{id}/workspace/file", get(file))
        .route("/api/conversations/{id}/workspace/download", get(download))
        .route("/api/conversations/{id}/workspace/raw", get(raw))
        .route(
            "/api/conversations/{id}/workspace/checkpoints/{sha}/restore",
            post(restore_checkpoint),
        )
        .route(
            "/api/conversations/{id}/workspace/terminal",
            get(terminal::open),
        )
        .with_state(state)
}

async fn status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, false).await?;
    Ok(Json(json!({
        "workspace": sandboxes.get(user.id, id).await?,
        "diskLimit": sandboxes.config().disk_limit,
    })))
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, false).await?;
    let projects: Vec<Value> = sandboxes
        .list(user.id)
        .await?
        .into_iter()
        .map(|(project, title)| {
            let mut value = json!(project);
            value["title"] = json!(title);
            value
        })
        .collect();
    Ok(Json(json!({
        "workspaces": projects,
        "diskLimit": sandboxes.config().disk_limit,
    })))
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, true).await?;
    let existed = sandboxes.get(user.id, id).await?.is_some();
    let mut sandbox = sandboxes.create(user.id, id).await?;
    // A project chat's new workspace starts with the project's code.
    if !existed {
        match crate::code::seed(&s.store.pool, &sandboxes, &user, id, &sandbox).await {
            Ok(Some(running)) => {
                let started = running.clone();
                let sb = sandboxes.clone();
                tokio::spawn(async move {
                    if let Err(error) = tools::ensure_dev_server(&sb, &started).await {
                        tracing::warn!("could not start the workspace server: {error}");
                    }
                });
                sandbox = running;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(chat=%id, "could not put the project's code in the workspace: {}", error.1)
            }
        }
    }
    Ok(Json(json!({"workspace":sandbox})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, true).await?;
    let sandbox = sandboxes
        .get(user.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    sandboxes.destroy(&sandbox).await.map_err(|error| {
        tracing::warn!("could not delete workspace: {error:#}");
        ApiError(
            StatusCode::BAD_GATEWAY,
            "The workspace could not be deleted. Try again.".into(),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct PreviewQuery {
    /// A page to open instead of the home page, such as /about.html.
    #[serde(default)]
    path: String,
}

async fn preview_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<PreviewQuery>,
) -> Result<Json<Value>, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, true).await?;
    let sandbox = sandboxes
        .get(user.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    // Bring the dev server back if it stopped, for example after the pod slept.
    let mut serving = sandbox.preview_port.is_some();
    if sandbox.status == "running" {
        match tools::ensure_dev_server(&sandboxes, &sandbox).await {
            Ok(server) => serving |= server.is_some(),
            Err(error) => tracing::warn!("could not restart the workspace server: {error}"),
        }
    }
    let ticket = preview::sign(
        sandboxes.key().await?,
        preview::ENTER,
        &sandbox.slug,
        user.id,
        60,
    );
    let mut url = format!(
        "{}/__obleth/enter?ticket={ticket}",
        sandboxes.preview_origin(&sandbox)
    );
    if let Some(page) = preview::landing(&q.path) {
        url.push_str("&next=");
        url.extend(url::form_urlencoded::byte_serialize(page.as_bytes()));
    }
    Ok(Json(json!({
        "url": url,
        "origin": sandboxes.preview_origin(&sandbox),
        "running": sandbox.status == "running" && serving,
    })))
}

#[derive(Deserialize)]
struct FileQuery {
    #[serde(default)]
    path: String,
}

async fn running(sandboxes: &Sandboxes, user: &Identity, id: Uuid) -> Result<Sandbox, ApiError> {
    let sandbox = sandboxes
        .get(user.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if sandbox.status != "running" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The workspace is not running".into(),
        ));
    }
    Ok(sandbox)
}

async fn files(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> Result<Json<Value>, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, false).await?;
    let sandbox = running(&sandboxes, &user, id).await?;
    let entries = tools::list_entries(&sandboxes, &sandbox, &q.path)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(json!({"entries":entries})))
}

async fn file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> Result<Json<Value>, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, false).await?;
    let sandbox = running(&sandboxes, &user, id).await?;
    let (content, truncated) = tools::read_text(&sandboxes, &sandbox, &q.path, 200_000)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(
        json!({"path":q.path,"content":content,"truncated":truncated}),
    ))
}

/// The project as a zip, without dependencies or build output. An asleep
/// workspace is woken first.
async fn download(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, false).await?;
    let sandbox = sandboxes
        .get(user.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let sandbox = sandboxes
        .ensure_running(&sandbox)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    let title: String = sqlx::query_scalar("SELECT title FROM conversations WHERE id=$1")
        .bind(id)
        .fetch_one(&sandboxes.inner.pool)
        .await?;
    let archive = tools::archive(&sandboxes, &sandbox)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    let disposition = format!("attachment; filename=\"{}.zip\"", file_stem(&title));
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        archive,
    )
        .into_response())
}

/// Puts the project back as it was at one of its checkpoints. Not while a reply works
/// in it.
async fn restore_checkpoint(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, sha)): Path<(Uuid, String)>,
) -> Result<Json<Value>, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, true).await?;
    let sandbox = sandboxes
        .get(user.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let busy: bool =
        sqlx::query_scalar("SELECT active_generation IS NOT NULL FROM conversations WHERE id=$1")
            .bind(id)
            .fetch_one(&sandboxes.inner.pool)
            .await?;
    if busy {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Wait for the reply to finish, or stop it, before restoring a checkpoint".into(),
        ));
    }
    let sandbox = sandboxes
        .ensure_running(&sandbox)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    let before = tools::restore(&sandboxes, &sandbox, &sha)
        .await
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(json!({"before": before})))
}

/// Largest file served by `raw`.
const RAW_LIMIT: usize = 10 * 1024 * 1024;

/// A workspace file's bytes for the file viewer. Images are shown inline; anything
/// else downloads. The sandbox policy keeps workspace content inert on Chat's origin.
async fn raw(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<FileQuery>,
) -> Result<Response, ApiError> {
    let (sandboxes, user) = manager(&s, &headers, false).await?;
    let sandbox = running(&sandboxes, &user, id).await?;
    let (bytes, truncated) = tools::read_bytes(&sandboxes, &sandbox, &q.path, RAW_LIMIT)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    if truncated {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Files over 10 MB open from the project download".into(),
        ));
    }
    let extension = q
        .path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let image = match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    };
    let name = q.path.rsplit('/').next().unwrap_or("file");
    let disposition = match image {
        Some(_) => "inline".to_owned(),
        None => format!("attachment; filename=\"{}\"", file_stem(name)),
    };
    Ok((
        [
            (
                header::CONTENT_TYPE,
                image.unwrap_or("application/octet-stream").to_owned(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:".to_owned(),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// A Kubernetes quantity such as 10Gi, 500M or 1073741824 in bytes.
pub(crate) fn quantity_bytes(quantity: &str) -> Option<u64> {
    let quantity = quantity.trim();
    let split = quantity
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(quantity.len());
    let (number, unit) = quantity.split_at(split);
    let scale: u64 = match unit {
        "" => 1,
        "Ki" => 1 << 10,
        "Mi" => 1 << 20,
        "Gi" => 1 << 30,
        "Ti" => 1 << 40,
        "K" | "k" => 1_000,
        "M" => 1_000_000,
        "G" => 1_000_000_000,
        "T" => 1_000_000_000_000,
        _ => return None,
    };
    number.parse::<u64>().ok()?.checked_mul(scale)
}

/// A safe download name from a conversation title.
fn file_stem(title: &str) -> String {
    let mut stem = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            stem.push(c.to_ascii_lowercase());
        } else if !stem.is_empty() && !stem.ends_with('-') {
            stem.push('-');
        }
        if stem.len() >= 60 {
            break;
        }
    }
    let stem = stem.trim_end_matches('-');
    if stem.is_empty() {
        "workspace".into()
    } else {
        stem.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_sizes_parse_as_bytes() {
        assert_eq!(quantity_bytes("10Gi"), Some(10 << 30));
        assert_eq!(quantity_bytes("500M"), Some(500_000_000));
        assert_eq!(quantity_bytes("2048"), Some(2048));
        assert_eq!(quantity_bytes("1.5Gi"), None);
        assert_eq!(quantity_bytes("10GB"), None);
    }

    #[test]
    fn download_names_are_safe() {
        assert_eq!(
            file_stem("Build me a site for my lab's reading group"),
            "build-me-a-site-for-my-lab-s-reading-group"
        );
        assert_eq!(file_stem("\"; rm -rf /"), "rm-rf");
        assert_eq!(file_stem("…"), "workspace");
    }
}
