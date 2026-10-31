//! Chat's browser: one headless Chromium per person, in a pod and profile volume in
//! the workspace namespace, driven over the DevTools protocol. Each conversation with
//! the browser on has its own tab. The person watches it live beside the chat and can
//! take over, for example to sign in; while they have control the model's browser
//! tools refuse to run. The model's tools are in tools.rs, the live view in live.rs.
pub mod cdp;
pub mod live;
pub mod tools;

use crate::{
    auth::Identity,
    error::ApiError,
    sandbox::{
        create_pod, kube::Kube, pod_state, quantity_bytes, release_image, waiting_reason, PodState,
        SandboxConfig, SLOTS_FULL,
    },
    AppState,
};
use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use cdp::Cdp;
use chrono::{DateTime, Utc};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use std::{env, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Where Chromium serves DevTools for Chat inside the pod.
const PORT: u16 = 9222;
/// The page size the model's tab gets, in CSS pixels.
pub const VIEWPORT: (u32, u32) = (1280, 800);

#[derive(Clone)]
pub struct BrowserConfig {
    pub image: String,
    pub cpu_limit: String,
    pub memory_limit: String,
    pub storage: String,
    pub idle: Duration,
}

impl BrowserConfig {
    /// The browser comes with workspaces unless OBLETH_CHAT_BROWSER=off. Its image
    /// defaults to this release's.
    pub fn from_env() -> Result<Option<Self>> {
        let var = |key: &str| env::var(key).ok().filter(|v| !v.trim().is_empty());
        match var("OBLETH_CHAT_BROWSER").as_deref().map(str::trim) {
            None | Some("on") => {}
            Some("off") => return Ok(None),
            Some(_) => bail!("OBLETH_CHAT_BROWSER must be on or off"),
        }
        let image = var("OBLETH_CHAT_BROWSER_IMAGE").unwrap_or_else(|| release_image("browser"));
        let storage = var("OBLETH_CHAT_BROWSER_STORAGE").unwrap_or_else(|| "1Gi".into());
        quantity_bytes(&storage)
            .context("OBLETH_CHAT_BROWSER_STORAGE must be a size such as 1Gi")?;
        let minutes = match var("OBLETH_CHAT_BROWSER_IDLE_MINUTES") {
            Some(value) => value
                .parse::<u64>()
                .ok()
                .filter(|m| (5..=24 * 60).contains(m))
                .context("OBLETH_CHAT_BROWSER_IDLE_MINUTES must be between 5 and 1440")?,
            None => 15,
        };
        Ok(Some(Self {
            image,
            cpu_limit: var("OBLETH_CHAT_BROWSER_CPU").unwrap_or_else(|| "1".into()),
            memory_limit: var("OBLETH_CHAT_BROWSER_MEMORY").unwrap_or_else(|| "1Gi".into()),
            storage,
            idle: Duration::from_secs(minutes * 60),
        }))
    }
}

/// A person's browser: its pod, profile volume and settings.
#[derive(Clone, Debug, FromRow)]
pub struct Browser {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub slug: String,
    pub status: String,
    pub pod_ip: Option<String>,
    pub error_message: Option<String>,
    pub search_engine: String,
    pub last_used_at: DateTime<Utc>,
}

/// A conversation's tab in its owner's browser.
#[derive(Clone, Debug, FromRow)]
pub struct Tab {
    pub conversation_id: Uuid,
    pub browser_id: Uuid,
    pub target_id: Option<String>,
    pub controller: String,
    pub url: String,
    pub title: String,
}

/// A tab ready for commands: a DevTools connection and a session on its page.
pub struct Page {
    pub cdp: Cdp,
    pub session: String,
    pub target: String,
    pub browser: Browser,
    pub tab: Tab,
}

fn pod_name(slug: &str) -> String {
    format!("brw-{slug}")
}

const BROWSER: &str =
    "SELECT id,owner_id,slug,status,pod_ip,error_message,search_engine,last_used_at FROM browsers";
const TAB: &str =
    "SELECT conversation_id,browser_id,target_id,controller,url,title FROM browser_tabs";

#[derive(Clone)]
pub struct Browsers {
    inner: Arc<Inner>,
}
struct Inner {
    config: BrowserConfig,
    sandbox: SandboxConfig,
    kube: Kube,
    pool: PgPool,
    http: reqwest::Client,
    /// host:port of a browser to use instead of pods, for tests.
    fixed: Option<String>,
}

impl Browsers {
    pub fn new(config: BrowserConfig, sandbox: SandboxConfig, pool: PgPool) -> Result<Self> {
        let kube = Kube::from_env(sandbox.namespace.clone())?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        let fixed = env::var("OBLETH_CHAT_BROWSER_ADDRESS")
            .ok()
            .filter(|v| !v.is_empty());
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                sandbox,
                kube,
                pool,
                http,
                fixed,
            }),
        })
    }
    pub fn config(&self) -> &BrowserConfig {
        &self.inner.config
    }
    pub fn chat_origin(&self) -> &str {
        &self.inner.sandbox.chat_origin
    }
    pub fn allowed(&self, user: &Identity) -> bool {
        self.inner.sandbox.everyone || user.is_admin()
    }
    pub fn pool(&self) -> &PgPool {
        &self.inner.pool
    }

    /// Whether the conversation has the browser turned on.
    pub async fn is_on(&self, owner: Uuid, conversation: Uuid) -> bool {
        sqlx::query_scalar("SELECT browser FROM conversations WHERE id=$1 AND owner_id=$2")
            .bind(conversation)
            .bind(owner)
            .fetch_optional(&self.inner.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }
    pub async fn set_on(&self, owner: Uuid, conversation: Uuid, on: bool) -> Result<(), ApiError> {
        let updated =
            sqlx::query("UPDATE conversations SET browser=$3 WHERE id=$1 AND owner_id=$2")
                .bind(conversation)
                .bind(owner)
                .bind(on)
                .execute(&self.inner.pool)
                .await?;
        if updated.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        Ok(())
    }

    async fn record(&self, owner: Uuid) -> Result<Browser, ApiError> {
        let pool = &self.inner.pool;
        sqlx::query("INSERT INTO browsers(id,owner_id,slug) VALUES($1,$2,$3) ON CONFLICT (owner_id) DO NOTHING")
            .bind(Uuid::new_v4())
            .bind(owner)
            .bind(new_slug())
            .execute(pool)
            .await?;
        Ok(
            sqlx::query_as::<_, Browser>(&format!("{BROWSER} WHERE owner_id=$1"))
                .bind(owner)
                .fetch_one(pool)
                .await?,
        )
    }
    pub async fn get(&self, owner: Uuid) -> Result<Option<Browser>, ApiError> {
        Ok(
            sqlx::query_as::<_, Browser>(&format!("{BROWSER} WHERE owner_id=$1"))
                .bind(owner)
                .fetch_optional(&self.inner.pool)
                .await?,
        )
    }
    pub async fn get_tab(&self, owner: Uuid, conversation: Uuid) -> Result<Option<Tab>, ApiError> {
        Ok(sqlx::query_as::<_, Tab>(&format!(
            "{TAB} WHERE conversation_id=$1 AND browser_id=(SELECT id FROM browsers WHERE owner_id=$2)"
        ))
        .bind(conversation)
        .bind(owner)
        .fetch_optional(&self.inner.pool)
        .await?)
    }
    /// The owner's browser record and the conversation's tab record, created as needed.
    pub async fn tab(&self, owner: Uuid, conversation: Uuid) -> Result<(Browser, Tab), ApiError> {
        let browser = self.record(owner).await?;
        sqlx::query("INSERT INTO browser_tabs(conversation_id,browser_id) SELECT $1,$2 WHERE EXISTS (SELECT 1 FROM conversations WHERE id=$1 AND owner_id=$3) ON CONFLICT (conversation_id) DO NOTHING")
            .bind(conversation)
            .bind(browser.id)
            .bind(owner)
            .execute(&self.inner.pool)
            .await?;
        let tab = self
            .get_tab(owner, conversation)
            .await?
            .ok_or_else(ApiError::not_found)?;
        Ok((browser, tab))
    }

    pub async fn set_controller(
        &self,
        owner: Uuid,
        conversation: Uuid,
        controller: &str,
    ) -> Result<(), ApiError> {
        let (_, tab) = self.tab(owner, conversation).await?;
        sqlx::query(
            "UPDATE browser_tabs SET controller=$2,updated_at=NOW() WHERE conversation_id=$1",
        )
        .bind(tab.conversation_id)
        .bind(controller)
        .execute(&self.inner.pool)
        .await?;
        Ok(())
    }
    pub async fn save_location(&self, conversation: Uuid, url: &str, title: &str) {
        let url: String = url.chars().take(2000).collect();
        let title: String = title.chars().take(300).collect();
        let _ = sqlx::query(
            "UPDATE browser_tabs SET url=$2,title=$3,updated_at=NOW() WHERE conversation_id=$1",
        )
        .bind(conversation)
        .bind(url)
        .bind(title)
        .execute(&self.inner.pool)
        .await;
    }
    pub async fn touch(&self, browser: Uuid) {
        let _ = sqlx::query("UPDATE browsers SET last_used_at=NOW() WHERE id=$1 AND last_used_at<NOW()-INTERVAL '20 seconds'")
            .bind(browser)
            .execute(&self.inner.pool)
            .await;
    }

    /// The browser's DevTools address, starting its pod when needed.
    pub async fn ensure_running(&self, browser: &Browser) -> Result<String, String> {
        let pool = &self.inner.pool;
        if let Some(fixed) = &self.inner.fixed {
            let _ =
                sqlx::query("UPDATE browsers SET status='running',last_used_at=NOW() WHERE id=$1")
                    .bind(browser.id)
                    .execute(pool)
                    .await;
            return Ok(fixed.clone());
        }
        match self.start(browser).await {
            Ok(ip) => {
                let _ = sqlx::query("UPDATE browsers SET status='running',pod_ip=$2,error_message=NULL,last_used_at=NOW() WHERE id=$1")
                    .bind(browser.id)
                    .bind(&ip)
                    .execute(pool)
                    .await;
                Ok(format!("{ip}:{PORT}"))
            }
            Err(error) => {
                let message: String = format!("{error:#}").chars().take(500).collect();
                tracing::warn!(browser=%browser.id, "browser did not start: {message}");
                let message = if message.contains("exceeded quota") || message.contains(SLOTS_FULL)
                {
                    "All browser and workspace slots are in use right now. Try again in a few minutes.".into()
                } else {
                    message
                };
                let _ =
                    sqlx::query("UPDATE browsers SET status='error',error_message=$2 WHERE id=$1")
                        .bind(browser.id)
                        .bind(&message)
                        .execute(pool)
                        .await;
                Err(format!("The browser could not start: {message}"))
            }
        }
    }

    async fn start(&self, browser: &Browser) -> Result<String> {
        let kube = &self.inner.kube;
        let name = pod_name(&browser.slug);
        if let Some(pod) = kube.get("pods", &name).await? {
            match pod_state(&pod) {
                PodState::Ready(ip) => return Ok(ip),
                PodState::Finished | PodState::Deleting => {
                    kube.delete("pods", &name).await?;
                    for _ in 0..60 {
                        if kube.get("pods", &name).await?.is_none() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                PodState::Pending => {}
            }
        }
        // The idle sweep stops 'starting' browsers not used for a while; a browser
        // last used hours ago would otherwise be stopped while its pod starts.
        sqlx::query("UPDATE browsers SET status='starting',error_message=NULL,last_used_at=NOW() WHERE id=$1")
            .bind(browser.id)
            .execute(&self.inner.pool)
            .await?;
        kube.create(
            "persistentvolumeclaims",
            &self.volume_manifest(&browser.slug),
        )
        .await?;
        create_pod(
            &self.inner.pool,
            kube,
            self.inner.sandbox.slots,
            &self.pod_manifest(&browser.slug),
        )
        .await?;
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(pod) = kube.get("pods", &name).await? else {
                bail!("the browser pod disappeared while starting");
            };
            match pod_state(&pod) {
                PodState::Ready(ip) => return Ok(ip),
                PodState::Finished => bail!("the browser exited: {}", waiting_reason(&pod)),
                _ => {
                    let reason = waiting_reason(&pod);
                    if matches!(
                        reason.as_str(),
                        "ErrImagePull" | "ImagePullBackOff" | "InvalidImageName"
                    ) {
                        bail!("the browser image could not start ({reason})");
                    }
                }
            }
        }
        bail!("the browser was not ready after two minutes")
    }

    /// Workspace labels, so the namespace's network policies apply (only Chat may
    /// connect; public HTTP(S) only out), plus obleth.io/browser, which keeps the
    /// workspace reconciler's hands off.
    fn labels(slug: &str) -> Value {
        json!({"app.kubernetes.io/managed-by":"obleth-chat","app.kubernetes.io/name":"obleth-sandbox","obleth.io/browser":slug})
    }

    fn volume_manifest(&self, slug: &str) -> Value {
        let mut spec = json!({"accessModes":["ReadWriteOnce"],"resources":{"requests":{"storage":self.inner.config.storage}}});
        if let Some(class) = &self.inner.sandbox.storage_class {
            spec["storageClassName"] = json!(class);
        }
        json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":pod_name(slug),"labels":Self::labels(slug)},"spec":spec})
    }

    /// The restricted Pod Security profile, as for workspaces.
    fn pod_manifest(&self, slug: &str) -> Value {
        let c = &self.inner.config;
        let mut spec = json!({
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            "hostname": "browser",
            "restartPolicy": "Always",
            "terminationGracePeriodSeconds": 5,
            "securityContext": {"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,"fsGroup":1000,"seccompProfile":{"type":"RuntimeDefault"}},
            "containers": [{
                "name": "browser",
                "image": c.image,
                "imagePullPolicy": "IfNotPresent",
                "ports": [{"containerPort": PORT}],
                "readinessProbe": {"exec":{"command":["curl","-fsS","-o","/dev/null","http://127.0.0.1:9223/json/version"]},"periodSeconds":2,"timeoutSeconds":3},
                "resources": {
                    "requests": {"cpu":"100m","memory":"256Mi","ephemeral-storage":"256Mi"},
                    "limits": {"cpu":c.cpu_limit,"memory":c.memory_limit,"ephemeral-storage":"2Gi"}
                },
                "securityContext": {"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]},"runAsNonRoot":true},
                "volumeMounts": [{"name":"profile","mountPath":"/profile"},{"name":"tmp","mountPath":"/tmp"}]
            }],
            "volumes": [
                {"name":"profile","persistentVolumeClaim":{"claimName":pod_name(slug)}},
                {"name":"tmp","emptyDir":{"sizeLimit":"1Gi"}}
            ]
        });
        if let Some(class) = &self.inner.sandbox.runtime_class {
            spec["runtimeClassName"] = json!(class);
        }
        json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":pod_name(slug),"labels":Self::labels(slug)},"spec":spec})
    }

    /// Opens the conversation's tab, starting the browser and creating the tab as
    /// needed. A tab whose page is gone (the pod restarted) is replaced.
    pub async fn open(&self, owner: Uuid, conversation: Uuid) -> Result<Page, String> {
        let (browser, mut tab) = self.tab(owner, conversation).await.map_err(|e| e.1)?;
        let address = self.ensure_running(&browser).await?;
        let mut connected = None;
        for attempt in 0..5 {
            match Cdp::connect(&self.inner.http, &address).await {
                Ok(cdp) => {
                    connected = Some(cdp);
                    break;
                }
                Err(error) if attempt == 4 => {
                    tracing::warn!("could not reach the browser: {error:#}");
                }
                Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        }
        let cdp = connected.ok_or("The browser is not answering. Try again in a moment.")?;
        let fail = |error: anyhow::Error| format!("The browser failed: {error:#}");
        // Downloads are not kept yet.
        let _ = cdp
            .call(
                "Browser.setDownloadBehavior",
                json!({"behavior":"deny"}),
                None,
            )
            .await;
        let targets = cdp
            .call("Target.getTargets", json!({}), None)
            .await
            .map_err(fail)?;
        let exists = |id: &str| {
            targets["targetInfos"].as_array().is_some_and(|all| {
                all.iter()
                    .any(|t| t["targetId"] == id && t["type"] == "page")
            })
        };
        let target = match tab.target_id.as_deref().filter(|id| exists(id)) {
            Some(id) => id.to_owned(),
            None => {
                let created = cdp
                    .call("Target.createTarget", json!({"url":"about:blank"}), None)
                    .await
                    .map_err(fail)?;
                let id = created["targetId"]
                    .as_str()
                    .ok_or("The browser did not open a tab")?
                    .to_owned();
                // The live view and a tool can open the tab at the same moment: the
                // first to record its tab wins and the other closes its own.
                let claimed = sqlx::query("UPDATE browser_tabs SET target_id=$2,url='',title='',updated_at=NOW() WHERE conversation_id=$1 AND target_id IS NOT DISTINCT FROM $3")
                    .bind(conversation)
                    .bind(&id)
                    .bind(&tab.target_id)
                    .execute(&self.inner.pool)
                    .await
                    .map_err(|e| format!("The browser failed: {e}"))?
                    .rows_affected()
                    == 1;
                if claimed {
                    tab.url.clear();
                    tab.title.clear();
                    id
                } else {
                    let _ = cdp
                        .call("Target.closeTarget", json!({"targetId":id}), None)
                        .await;
                    let winner = self
                        .get_tab(owner, conversation)
                        .await
                        .map_err(|e| e.1)?
                        .and_then(|t| t.target_id)
                        .ok_or("The browser tab could not open. Try again.")?;
                    tab.url.clear();
                    tab.title.clear();
                    winner
                }
            }
        };
        tab.target_id = Some(target.clone());
        let session = cdp.attach(&target).await.map_err(fail)?;
        // Pages in background tabs keep running as if focused.
        let _ = cdp
            .call(
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
                Some(&session),
            )
            .await;
        self.touch(browser.id).await;
        Ok(Page {
            cdp,
            session,
            target,
            browser,
            tab,
        })
    }

    /// Stops the person's browser pod; the profile volume stays.
    pub async fn sleep(&self, browser: &Browser) -> Result<()> {
        self.inner
            .kube
            .delete("pods", &pod_name(&browser.slug))
            .await?;
        sqlx::query("UPDATE browsers SET status='stopped',pod_ip=NULL WHERE id=$1")
            .bind(browser.id)
            .execute(&self.inner.pool)
            .await?;
        Ok(())
    }

    /// Deletes the person's browser: pod, profile (cookies, sign-ins, history), tabs
    /// and site permissions.
    pub async fn forget(&self, browser: &Browser) -> Result<()> {
        let name = pod_name(&browser.slug);
        self.inner.kube.delete("pods", &name).await?;
        self.inner
            .kube
            .delete("persistentvolumeclaims", &name)
            .await?;
        sqlx::query("DELETE FROM browser_site_grants WHERE owner_id=$1")
            .bind(browser.owner_id)
            .execute(&self.inner.pool)
            .await?;
        sqlx::query("DELETE FROM browsers WHERE id=$1")
            .bind(browser.id)
            .execute(&self.inner.pool)
            .await?;
        Ok(())
    }

    /// Whether the person allowed form submissions on this site for good.
    pub async fn site_granted(&self, owner: Uuid, host: &str) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM browser_site_grants WHERE owner_id=$1 AND host=$2)",
        )
        .bind(owner)
        .bind(host)
        .fetch_one(&self.inner.pool)
        .await
        .unwrap_or(false)
    }

    /// One replica at a time: stop idle browsers, delete profiles unused past the
    /// workspace retention, and remove pods and volumes nobody owns.
    async fn reconcile(&self) -> Result<()> {
        let pool = &self.inner.pool;
        let mut tx = pool.begin().await?;
        let locked: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_xact_lock(hashtext('obleth-chat-browser-reconcile'))",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !locked {
            return Ok(());
        }
        let kube = &self.inner.kube;
        let expired = sqlx::query_as::<_, Browser>(&format!(
            "{BROWSER} WHERE last_used_at<NOW()-make_interval(secs=>$1)"
        ))
        .bind(self.inner.sandbox.retention.as_secs() as f64)
        .fetch_all(&mut *tx)
        .await?;
        for browser in expired {
            self.forget(&browser).await?;
        }
        let idle = sqlx::query_as::<_, Browser>(&format!(
            "{BROWSER} WHERE status IN ('running','error','starting') AND last_used_at<NOW()-make_interval(secs=>$1)"
        ))
        .bind(self.inner.config.idle.as_secs() as f64)
        .fetch_all(&mut *tx)
        .await?;
        for browser in idle {
            self.sleep(&browser).await?;
        }
        let known: Vec<String> = sqlx::query_scalar("SELECT slug FROM browsers")
            .fetch_all(&mut *tx)
            .await?;
        for kind in ["pods", "persistentvolumeclaims"] {
            for object in kube
                .list(
                    kind,
                    "app.kubernetes.io/managed-by=obleth-chat,obleth.io/browser",
                )
                .await?
            {
                let slug = object
                    .pointer("/metadata/labels/obleth.io~1browser")
                    .and_then(Value::as_str);
                let name = object.pointer("/metadata/name").and_then(Value::as_str);
                if let (Some(slug), Some(name)) = (slug, name) {
                    if !known.iter().any(|k| k == slug) {
                        kube.delete(kind, name).await?;
                    }
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn worker(self, shutdown: CancellationToken) {
        if self.inner.fixed.is_some() {
            return;
        }
        loop {
            if let Err(error) = self.reconcile().await {
                tracing::warn!("browser reconciliation failed: {error:#}");
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                _ = shutdown.cancelled() => break,
            }
        }
    }
}

fn new_slug() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .take(16)
        .map(char::from)
        .collect()
}

async fn manager(
    s: &AppState,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<(Browsers, Identity), ApiError> {
    let user = s.auth.require(headers, mutation).await?;
    let browsers = s
        .browsers
        .clone()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "The browser is not enabled".into()))?;
    if !browsers.allowed(&user) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "The browser is available to administrators".into(),
        ));
    }
    Ok((browsers, user))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/browser",
            get(account).put(settings).delete(forget_all),
        )
        .route("/api/browser/sleep", post(sleep_now))
        .route("/api/browser/sites/{host}", delete(forget_site))
        .route("/api/conversations/{id}/browser", get(status).put(toggle))
        .route("/api/conversations/{id}/browser/control", post(control))
        .route("/api/conversations/{id}/browser/live", get(live::open))
        .with_state(state)
}

fn tab_json(browser: Option<&Browser>, tab: Option<&Tab>, on: bool) -> Value {
    json!({
        "on": on,
        "status": browser.map_or("stopped", |b| b.status.as_str()),
        "errorMessage": browser.and_then(|b| b.error_message.as_deref()),
        "controller": tab.map_or("model", |t| t.controller.as_str()),
        "url": tab.map_or("", |t| t.url.as_str()),
        "title": tab.map_or("", |t| t.title.as_str()),
    })
}

async fn status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (browsers, user) = manager(&s, &headers, false).await?;
    let browser = browsers.get(user.id).await?;
    let tab = browsers.get_tab(user.id, id).await?;
    let on = browsers.is_on(user.id, id).await;
    Ok(Json(tab_json(browser.as_ref(), tab.as_ref(), on)))
}

#[derive(Deserialize)]
struct Toggle {
    on: bool,
}

async fn toggle(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Toggle>,
) -> Result<Json<Value>, ApiError> {
    let (browsers, user) = manager(&s, &headers, true).await?;
    browsers.set_on(user.id, id, body.on).await?;
    let browser = browsers.get(user.id).await?;
    let tab = browsers.get_tab(user.id, id).await?;
    Ok(Json(tab_json(browser.as_ref(), tab.as_ref(), body.on)))
}

#[derive(Deserialize)]
struct Control {
    controller: String,
}

/// Take over (the person drives the tab) or hand back (the model does).
async fn control(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Control>,
) -> Result<StatusCode, ApiError> {
    let (browsers, user) = manager(&s, &headers, true).await?;
    if !matches!(body.controller.as_str(), "user" | "model") {
        return Err(ApiError::bad("Choose user or model"));
    }
    if !browsers.is_on(user.id, id).await {
        return Err(ApiError::bad("The browser is off in this chat"));
    }
    browsers
        .set_controller(user.id, id, &body.controller)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn account(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let (browsers, user) = manager(&s, &headers, false).await?;
    let browser = browsers.get(user.id).await?;
    let tabs: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT t.conversation_id,c.title,t.url,t.controller FROM browser_tabs t JOIN browsers b ON b.id=t.browser_id JOIN conversations c ON c.id=t.conversation_id WHERE b.owner_id=$1 AND c.browser ORDER BY t.updated_at DESC LIMIT 50",
    )
    .bind(user.id)
    .fetch_all(browsers.pool())
    .await?;
    let sites: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT host,created_at FROM browser_site_grants WHERE owner_id=$1 ORDER BY host",
    )
    .bind(user.id)
    .fetch_all(browsers.pool())
    .await?;
    let config = browsers.config();
    Ok(Json(json!({
        "status": browser.as_ref().map_or("stopped", |b| b.status.as_str()),
        "errorMessage": browser.as_ref().and_then(|b| b.error_message.as_deref()),
        "searchEngine": browser.as_ref().map_or("duckduckgo", |b| b.search_engine.as_str()),
        "lastUsedAt": browser.as_ref().map(|b| b.last_used_at),
        "idleMinutes": config.idle.as_secs() / 60,
        "memory": config.memory_limit,
        "tabs": tabs.into_iter().map(|(id, title, url, controller)| json!({"conversationId":id,"title":title,"url":url,"controller":controller})).collect::<Vec<_>>(),
        "sites": sites.into_iter().map(|(host, at)| json!({"host":host,"createdAt":at})).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    search_engine: String,
}

async fn settings(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Settings>,
) -> Result<StatusCode, ApiError> {
    let (browsers, user) = manager(&s, &headers, true).await?;
    if tools::search_url(&body.search_engine, "").is_none() {
        return Err(ApiError::bad("Choose duckduckgo, google or bing"));
    }
    let browser = browsers.record(user.id).await?;
    sqlx::query("UPDATE browsers SET search_engine=$2 WHERE id=$1")
        .bind(browser.id)
        .bind(&body.search_engine)
        .execute(browsers.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sleep_now(State(s): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let (browsers, user) = manager(&s, &headers, true).await?;
    if let Some(browser) = browsers.get(user.id).await? {
        browsers.sleep(&browser).await.map_err(|error| {
            tracing::warn!("could not stop a browser: {error:#}");
            ApiError(StatusCode::BAD_GATEWAY, "The browser could not stop".into())
        })?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn forget_all(State(s): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let (browsers, user) = manager(&s, &headers, true).await?;
    if let Some(browser) = browsers.get(user.id).await? {
        browsers.forget(&browser).await.map_err(|error| {
            tracing::warn!("could not delete a browser: {error:#}");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "The browser data could not be deleted".into(),
            )
        })?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn forget_site(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(host): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (browsers, user) = manager(&s, &headers, true).await?;
    sqlx::query("DELETE FROM browser_site_grants WHERE owner_id=$1 AND host=$2")
        .bind(user.id)
        .bind(host)
        .execute(browsers.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Records "Always on this site" when the person allows a browser submission with
/// remember; called from the approval handler.
pub async fn remember_site(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: Uuid,
    tool: &str,
) -> Result<(), sqlx::Error> {
    if let Some(host) = tool
        .strip_prefix(tools::GRANT_PREFIX)
        .filter(|host| !host.is_empty())
    {
        sqlx::query(
            "INSERT INTO browser_site_grants(owner_id,host) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(owner)
        .bind(host)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Records that a browser click or submission waits for the person's approval.
pub async fn ask(
    pool: &PgPool,
    owner: Uuid,
    conversation: Uuid,
    generation: Uuid,
    call: &str,
    host: &str,
) -> Result<(), ApiError> {
    // A site name too long to keep can be allowed once, not remembered.
    let grant = if host.is_empty() || host.len() > 110 {
        tools::GRANT_PREFIX.to_owned()
    } else {
        format!("{}{host}", tools::GRANT_PREFIX)
    };
    sqlx::query("INSERT INTO tool_approvals(generation_id,call_id,conversation_id,owner_id,tool_name) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
        .bind(generation)
        .bind(call)
        .bind(conversation)
        .bind(owner)
        .bind(grant)
        .execute(pool)
        .await?;
    Ok(())
}

/// Step pictures are kept on the newest browser steps only, to keep replies small.
pub fn keep_recent_thumbnails(activity: &mut [Value]) {
    const KEEP: usize = 12;
    let mut seen = 0;
    for entry in activity.iter_mut().rev() {
        if let Some(fields) = entry.as_object_mut().filter(|e| e.contains_key("thumb")) {
            seen += 1;
            if seen > KEEP {
                fields.remove("thumb");
            }
        }
    }
}
