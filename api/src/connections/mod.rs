//! Connections: MCP servers, Slurm clusters (slurmrestd) and Kubernetes clusters.
//! People add their own, and administrators can share one with everyone; for a
//! shared Slurm or Kubernetes cluster, and a shared MCP server set to personal
//! tokens, each person adds their own token, so calls run as them. Tools join every chat's tool loop, and a tool that may change something
//! waits for the person's Allow or Deny.
pub mod kubernetes;
pub mod mcp;
pub mod slurm;

use crate::{
    auth::Identity,
    error::ApiError,
    secrets::{self, SecretKey},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const MAX_PER_OWNER: i64 = 20;
/// Tools offered to the model in one request, across all connections.
const MAX_OFFERED: usize = 128;
const MODEL_OUTPUT: usize = 48 * 1024;
const DETAIL_OUTPUT: usize = 4 * 1024;
/// How long a tool call waits for Allow or Deny.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct Connections {
    pool: PgPool,
    /// The key derived from the gateway signing key, which sealed tokens before the
    /// installation had its own secret key. Format 1.
    legacy: Option<[u8; 32]>,
    /// The installation's secret key. Format 2; new tokens use it when it is set.
    secret: Option<SecretKey>,
    clients: mcp::Clients,
    /// Lets people's own connections reach private addresses (development only).
    private_allowed: bool,
    /// Whether Slurm and Kubernetes clusters are offered (OBLETH_CHAT_CONNECTIONS=all).
    clusters: bool,
    /// Whether a chat may run its tools without asking (OBLETH_CHAT_AUTO_MODE).
    auto_mode: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Mcp,
    Slurm,
    Kubernetes,
}
impl Kind {
    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "mcp" => Some(Self::Mcp),
            "slurm" => Some(Self::Slurm),
            "kubernetes" => Some(Self::Kubernetes),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Slurm => "slurm",
            Self::Kubernetes => "kubernetes",
        }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    owner_id: Option<Uuid>,
    name: String,
    url: String,
    token: Option<Vec<u8>>,
    enabled: bool,
    confirm_actions: bool,
    tools: Value,
    server_name: Option<String>,
    checked_at: Option<DateTime<Utc>>,
    opted_out: bool,
    kind: String,
    options: Value,
    credential: Option<Vec<u8>>,
    credential_options: Option<Value>,
}
impl Row {
    fn shared(&self) -> bool {
        self.owner_id.is_none()
    }
    fn can_manage(&self, user: &Identity) -> bool {
        self.owner_id == Some(user.id) || (self.shared() && user.is_admin())
    }
    fn kind(&self) -> Kind {
        Kind::parse(&self.kind).unwrap_or(Kind::Mcp)
    }
    /// A shared cluster, or a shared MCP server set to personal tokens, runs with
    /// each person's own token rather than one for everyone.
    fn personal_tokens(&self) -> bool {
        self.shared() && (self.kind() != Kind::Mcp || self.options["personalTokens"] == true)
    }
    /// Such a connection needs the person's own credentials before it can be used.
    fn usable(&self) -> bool {
        !self.personal_tokens() || self.credential.is_some()
    }
    /// The Slurm user, namespace and identity in effect for the viewer.
    fn settings(&self) -> &Value {
        match (&self.credential_options, self.shared()) {
            (Some(options), true) => options,
            _ => &self.options,
        }
    }
    fn tools(&self) -> Vec<mcp::Tool> {
        match self.kind() {
            Kind::Mcp => self
                .tools
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(mcp::Tool::from_json)
                .collect(),
            Kind::Slurm => slurm::tools(),
            Kind::Kubernetes => kubernetes::tools(),
        }
    }
}

const SELECT: &str = "SELECT c.id,c.owner_id,c.name,c.url,c.token,c.enabled,c.confirm_actions,c.tools,c.server_name,c.checked_at,c.kind,c.options,cr.token AS credential,cr.options AS credential_options,EXISTS(SELECT 1 FROM connection_opt_outs o WHERE o.user_id=$1 AND o.connection_id=c.id) AS opted_out FROM connections c LEFT JOIN connection_credentials cr ON cr.connection_id=c.id AND cr.user_id=$1 WHERE (c.owner_id=$1 OR c.owner_id IS NULL)";

/// What a Slurm or Kubernetes client is built from.
struct Access<'a> {
    kind: Kind,
    url: &'a Url,
    shared: bool,
    ca: Option<&'a str>,
    /// The Slurm REST API version set on the cluster.
    api_version: Option<&'a str>,
    token: String,
    user: String,
    namespace: String,
}

enum Native {
    Slurm(slurm::Client),
    Kubernetes(kubernetes::Client),
}

/// Found when checking a connection: the server's name, its tools and who the
/// token belongs to.
struct Checked {
    server_name: Option<String>,
    tools: Vec<mcp::Tool>,
    identity: Option<String>,
}

impl Connections {
    /// The token key is derived from `key_material` (the gateway signing key), so
    /// no further secret needs provisioning. Replacing that key makes saved tokens
    /// unreadable; people then re-enter them.
    pub fn new(pool: PgPool, key_material: &[u8], private_allowed: bool) -> anyhow::Result<Self> {
        Self::with_keys(pool, Some(key_material), None, private_allowed)
    }

    /// Seals tokens with the installation's secret key when it is set, and still
    /// opens tokens sealed with the key derived from the gateway signing key while
    /// that key is configured. `reseal` moves those to the secret key.
    pub fn with_keys(
        pool: PgPool,
        key_material: Option<&[u8]>,
        secret: Option<SecretKey>,
        private_allowed: bool,
    ) -> anyhow::Result<Self> {
        let legacy = match key_material {
            Some(material) => {
                let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(material)?;
                mac.update(b"obleth-chat connection tokens v1");
                Some(mac.finalize().into_bytes().into())
            }
            None => None,
        };
        if legacy.is_none() && secret.is_none() {
            anyhow::bail!("Connections need a secret key or the gateway signing key");
        }
        Ok(Self {
            pool,
            legacy,
            secret,
            clients: mcp::Clients::new(private_allowed)?,
            private_allowed,
            clusters: true,
            auto_mode: true,
        })
    }

    /// Re-encrypts tokens still sealed with the key derived from the gateway signing
    /// key under the secret key, so the signing key can be removed. Each row changes
    /// only if it still holds what was read, so replicas starting together, or a
    /// second run, change nothing twice. Returns how many tokens moved.
    pub async fn reseal(&self) -> anyhow::Result<u64> {
        if self.secret.is_none() {
            return Ok(0);
        }
        let mut moved = 0;
        let rows: Vec<(Uuid, Vec<u8>)> = sqlx::query_as(
            "SELECT id, token FROM connections WHERE token IS NOT NULL AND get_byte(token, 0) = 1",
        )
        .fetch_all(&self.pool)
        .await?;
        let credentials: Vec<(Uuid, Uuid, Vec<u8>)> = sqlx::query_as(
            "SELECT connection_id, user_id, token FROM connection_credentials WHERE get_byte(token, 0) = 1",
        )
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() && credentials.is_empty() {
            return Ok(0);
        }
        if self.legacy.is_none() {
            tracing::warn!(
                count = rows.len() + credentials.len(),
                "saved connection tokens were sealed with the gateway signing key, which is not configured; people need to enter them again"
            );
            return Ok(0);
        }
        for (id, sealed) in rows {
            let Some(token) = self.unseal(id.as_bytes(), &sealed) else {
                continue;
            };
            moved += sqlx::query("UPDATE connections SET token=$2 WHERE id=$1 AND token=$3")
                .bind(id)
                .bind(self.seal(id.as_bytes(), &token))
                .bind(&sealed)
                .execute(&self.pool)
                .await?
                .rows_affected();
        }
        for (connection, user, sealed) in credentials {
            let aad = credential_aad(connection, user);
            let Some(token) = self.unseal(&aad, &sealed) else {
                continue;
            };
            moved += sqlx::query("UPDATE connection_credentials SET token=$3 WHERE connection_id=$1 AND user_id=$2 AND token=$4")
                .bind(connection)
                .bind(user)
                .bind(self.seal(&aad, &token))
                .bind(&sealed)
                .execute(&self.pool)
                .await?
                .rows_affected();
        }
        Ok(moved)
    }

    /// While both keys are set, moves tokens to the secret key every few minutes.
    /// During a rolling update, replicas still running a release without the secret
    /// key keep sealing with the signing key, so one pass at startup is not enough.
    pub async fn reseal_worker(self, shutdown: tokio_util::sync::CancellationToken) {
        if self.secret.is_none() || self.legacy.is_none() {
            return;
        }
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(300)) => {}
            }
            match self.reseal().await {
                Ok(0) => {}
                Ok(moved) => tracing::info!(moved, "connection tokens moved to the secret key"),
                Err(error) => {
                    tracing::warn!("could not move connection tokens to the secret key: {error:#}")
                }
            }
        }
    }

    /// Lets people switch a chat to Auto mode (the default) or not.
    pub fn auto_mode(mut self, allowed: bool) -> Self {
        self.auto_mode = allowed;
        self
    }

    /// Whether this chat's tool calls run without asking: it is in Auto mode and
    /// the installation allows that.
    pub async fn auto(&self, conversation: Uuid) -> bool {
        self.auto_mode
            && sqlx::query_scalar("SELECT auto_approve FROM conversations WHERE id=$1")
                .bind(conversation)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten()
                .unwrap_or(false)
    }

    /// Offers Slurm and Kubernetes clusters (the default) or MCP servers only.
    pub fn clusters(mut self, on: bool) -> Self {
        self.clusters = on;
        self
    }

    pub fn clusters_enabled(&self) -> bool {
        self.clusters
    }

    pub fn auto_mode_allowed(&self) -> bool {
        self.auto_mode
    }

    /// Encrypts a token bound to `aad`: the connection id, plus the user id for a
    /// person's credentials on a shared cluster.
    fn seal(&self, aad: &[u8], token: &str) -> Vec<u8> {
        match (&self.secret, &self.legacy) {
            (Some(key), _) => key.seal(2, aad, token.as_bytes()),
            (None, Some(key)) => secrets::seal_with(key, 1, aad, token.as_bytes()),
            (None, None) => unreachable!("a connection key is always configured"),
        }
    }

    fn unseal(&self, aad: &[u8], sealed: &[u8]) -> Option<String> {
        let plain = match sealed.first()? {
            1 => secrets::open_with(self.legacy.as_ref()?, 1, aad, sealed)?,
            2 => self.secret.as_ref()?.open(2, aad, sealed)?,
            _ => return None,
        };
        String::from_utf8(plain).ok()
    }

    fn client(&self, shared: bool) -> reqwest::Client {
        if shared {
            self.clients.any.clone()
        } else {
            self.clients.public.clone()
        }
    }

    fn native(&self, access: Access) -> Result<Native, String> {
        Ok(match access.kind {
            Kind::Slurm => Native::Slurm(slurm::Client::new(
                self.client(access.shared),
                access.url.clone(),
                access.user,
                access.token,
                access.api_version.map(str::to_owned),
            )),
            Kind::Kubernetes => {
                let http = mcp::client(access.shared || self.private_allowed, access.ca)
                    .map_err(|_| "The CA certificate could not be read".to_string())?;
                Native::Kubernetes(kubernetes::Client::new(
                    http,
                    access.url.clone(),
                    access.token,
                    access.namespace,
                ))
            }
            Kind::Mcp => unreachable!("MCP connections have no native client"),
        })
    }

    /// A Slurm or Kubernetes client acting with the viewer's token and settings.
    fn cluster_client(&self, row: &Row, url: &Url, token: String) -> Result<Native, String> {
        let settings = row.settings();
        self.native(Access {
            kind: row.kind(),
            url,
            shared: row.shared(),
            ca: row.options["ca"].as_str(),
            api_version: row.options["apiVersion"].as_str(),
            token,
            user: settings["user"].as_str().unwrap_or("").into(),
            namespace: settings["namespace"].as_str().unwrap_or("default").into(),
        })
    }

    /// Connects once: lists an MCP server's tools, checks a cluster token, or for a
    /// shared cluster without a token, checks that the server answers.
    async fn check(
        &self,
        kind: Kind,
        url: &Url,
        token: Option<String>,
        options: &Value,
        shared: bool,
    ) -> Result<Checked, String> {
        if kind == Kind::Mcp {
            let mut session = mcp::Session::open(self.client(shared), url.clone(), token).await?;
            let tools = session.list_tools().await;
            let server_name = session.server_name.clone();
            session.close().await;
            let tools = tools?;
            if tools.is_empty() {
                return Err("The server lists no tools".into());
            }
            return Ok(Checked {
                server_name,
                tools,
                identity: None,
            });
        }
        let native = self.native(Access {
            kind,
            url,
            shared,
            ca: options["ca"].as_str(),
            api_version: options["apiVersion"].as_str(),
            token: token.clone().unwrap_or_default(),
            user: options["user"].as_str().unwrap_or("").into(),
            namespace: options["namespace"].as_str().unwrap_or("default").into(),
        })?;
        let (server_name, identity) = match (native, token.is_some()) {
            (Native::Slurm(client), false) => (client.reachable().await.map(|_| None)?, None),
            (Native::Slurm(mut client), true) => (None, Some(client.check().await?)),
            (Native::Kubernetes(client), false) => (client.reachable().await?, None),
            (Native::Kubernetes(client), true) => (
                client.reachable().await.unwrap_or(None),
                Some(client.check().await?),
            ),
        };
        let server_name = server_name.or_else(|| {
            Some(match kind {
                Kind::Slurm => "Slurm".into(),
                _ => "Kubernetes".into(),
            })
        });
        Ok(Checked {
            server_name,
            tools: vec![],
            identity,
        })
    }

    async fn visible(&self, user: &Identity) -> Result<Vec<Row>, ApiError> {
        let mut rows: Vec<Row> = sqlx::query_as(&format!(
            "{SELECT} ORDER BY c.owner_id IS NULL, LOWER(c.name)"
        ))
        .bind(user.id)
        .fetch_all(&self.pool)
        .await?;
        // A shared connection an administrator switched off is hidden from everyone else,
        // and clusters are hidden while they are turned off for the installation.
        rows.retain(|r| {
            (r.enabled || !r.shared() || user.is_admin())
                && (self.clusters || r.kind() == Kind::Mcp)
        });
        Ok(rows)
    }

    async fn find(&self, user: &Identity, id: Uuid) -> Result<Row, ApiError> {
        self.visible(user)
            .await?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(ApiError::not_found)
    }

    /// The token a row uses for `user`: its own, or the person's credential where
    /// each person brings one. None when there is none or it cannot be decrypted.
    fn token_for(&self, row: &Row, user: Uuid) -> Option<String> {
        match (&row.credential, row.personal_tokens()) {
            (Some(sealed), true) => self.unseal(&credential_aad(row.id, user), sealed),
            _ => row
                .token
                .as_deref()
                .and_then(|t| self.unseal(row.id.as_bytes(), t)),
        }
    }

    /// The tools this person can use in this conversation now.
    pub async fn toolset(&self, user: Uuid, conversation: Uuid) -> Toolset {
        let rows: Vec<Row> = match sqlx::query_as(&format!(
            "{SELECT} AND c.enabled AND (c.kind<>'mcp' OR jsonb_array_length(c.tools)>0) AND NOT EXISTS(SELECT 1 FROM conversation_connections_off f WHERE f.conversation_id=$2 AND f.connection_id=c.id) ORDER BY c.owner_id IS NULL, c.created_at"
        ))
        .bind(user)
        .bind(conversation)
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!("could not load connections: {error}");
                vec![]
            }
        };
        let mut set = Toolset {
            servers: vec![],
            tools: vec![],
        };
        let clusters = self.clusters;
        for row in rows
            .into_iter()
            .filter(|r| !r.opted_out && r.usable() && (clusters || r.kind() == Kind::Mcp))
        {
            let Ok(url) = Url::parse(&row.url) else {
                continue;
            };
            let token = self.token_for(&row, user);
            if token.is_none() && (row.token.is_some() || row.credential.is_some()) {
                tracing::warn!(connection=%row.id, "connection token could not be decrypted");
                continue;
            }
            let settings = row.settings().clone();
            let (backend, about) = match row.kind() {
                Kind::Mcp => (
                    Backend::Mcp {
                        url,
                        token,
                        client: self.client(row.shared()),
                        session: None,
                    },
                    String::new(),
                ),
                kind => {
                    let about = match kind {
                        Kind::Slurm => format!(
                            "a Slurm cluster where you act as user {}",
                            settings["user"].as_str().unwrap_or("")
                        ),
                        _ => format!(
                            "a Kubernetes cluster where you act as {}, default namespace {}",
                            settings["identity"].as_str().unwrap_or("yourself"),
                            settings["namespace"].as_str().unwrap_or("default")
                        ),
                    };
                    match self.cluster_client(&row, &url, token.unwrap_or_default()) {
                        Ok(Native::Slurm(c)) => (Backend::Slurm(c), about),
                        Ok(Native::Kubernetes(c)) => (Backend::Kubernetes(c), about),
                        Err(error) => {
                            tracing::warn!(connection=%row.id, "connection unusable: {error}");
                            continue;
                        }
                    }
                }
            };
            let server = set.servers.len();
            let prefix = prefix(&row.name);
            for tool in row.tools() {
                let exposed: String = format!("{prefix}__{}", tool.name.replace(['.', '/'], "_"))
                    .chars()
                    .take(64)
                    .collect();
                if set.tools.len() >= MAX_OFFERED || set.tools.iter().any(|t| t.exposed == exposed)
                {
                    continue;
                }
                set.tools.push(Offered {
                    exposed,
                    server,
                    tool,
                });
            }
            set.servers.push(Server {
                id: row.id,
                name: row.name,
                about,
                confirm: row.confirm_actions,
                backend,
            });
        }
        set
    }
}

/// What a person's credential on a shared cluster is bound to when encrypted.
fn credential_aad(connection: Uuid, user: Uuid) -> Vec<u8> {
    [connection.as_bytes().as_slice(), user.as_bytes().as_slice()].concat()
}

/// A connection name as a tool-name prefix, such as "docs".
fn prefix(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out: String = out.trim_end_matches('_').chars().take(24).collect();
    if out.is_empty() {
        "mcp".into()
    } else {
        out.trim_end_matches('_').into()
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
    format!("{}\n… ({} more bytes)", &text[..end], text.len() - end)
}

struct Server {
    id: Uuid,
    name: String,
    /// What the model is told about a cluster connection.
    about: String,
    confirm: bool,
    backend: Backend,
}
enum Backend {
    Mcp {
        url: Url,
        token: Option<String>,
        client: reqwest::Client,
        session: Option<mcp::Session>,
    },
    Slurm(slurm::Client),
    Kubernetes(kubernetes::Client),
}
struct Offered {
    exposed: String,
    server: usize,
    tool: mcp::Tool,
}
pub struct Outcome {
    pub ok: bool,
    pub output: String,
    pub detail: String,
}

/// One turn's connection tools. Sessions open on first use and close with the turn.
pub struct Toolset {
    servers: Vec<Server>,
    tools: Vec<Offered>,
}
impl Toolset {
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
    pub fn definitions(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                let server = &self.servers[t.server].name;
                let about = match (&t.tool.title, t.tool.description.is_empty()) {
                    (Some(title), false) => format!("{title}. {}", t.tool.description),
                    (Some(title), true) => title.clone(),
                    (None, _) => t.tool.description.clone(),
                };
                json!({"type":"function","function":{"name":t.exposed,"description":clip(&format!("[{server}] {about}"), 1024),"parameters":t.tool.input_schema}})
            })
            .collect()
    }
    pub fn prompt(&self) -> String {
        let names: Vec<String> = self
            .servers
            .iter()
            .filter(|s| {
                self.tools
                    .iter()
                    .any(|t| self.servers[t.server].name == s.name)
            })
            .map(|s| {
                let about = if s.about.is_empty() {
                    String::new()
                } else {
                    format!(", {}", s.about)
                };
                format!("{}{about} (tools named {}__…)", s.name, prefix(&s.name))
            })
            .collect();
        format!(
            "You can use tools from these connected services: {}. Use them when they help with the request. Some tools need the person's approval before they run; if the person declines a call, do not retry it unless they ask.",
            names.join(", ")
        )
    }
    pub fn find(&self, name: &str) -> Option<usize> {
        self.tools.iter().position(|t| t.exposed == name)
    }
    pub fn connection(&self, index: usize) -> &str {
        &self.servers[self.tools[index].server].name
    }
    pub fn label(&self, index: usize) -> String {
        let tool = &self.tools[index].tool;
        format!(
            "{} · {}",
            self.connection(index),
            tool.title.as_deref().unwrap_or(&tool.name)
        )
    }
    /// What the reply shows of a call: the kind of service, the tool's own name and
    /// whether it only reads.
    pub fn describe(&self, index: usize) -> Value {
        let offered = &self.tools[index];
        let service = match self.servers[offered.server].backend {
            Backend::Mcp { .. } => Kind::Mcp,
            Backend::Slurm(_) => Kind::Slurm,
            Backend::Kubernetes(_) => Kind::Kubernetes,
        };
        json!({"service":service.as_str(),"action":offered.tool.name,"readOnly":offered.tool.read_only})
    }
    /// The connection and tool name an "Allow for this chat" grant covers.
    pub fn grant(&self, index: usize) -> (Uuid, &str) {
        let offered = &self.tools[index];
        (self.servers[offered.server].id, &offered.tool.name)
    }
    pub fn needs_approval(&self, index: usize) -> bool {
        let offered = &self.tools[index];
        self.servers[offered.server].confirm && !offered.tool.read_only
    }

    pub async fn call(&mut self, index: usize, arguments: &str) -> Outcome {
        let arguments: Value = match arguments.trim() {
            "" => json!({}),
            text => match serde_json::from_str(text) {
                Ok(value @ Value::Object(_)) => value,
                _ => {
                    return Outcome {
                        ok: false,
                        output: "The tool arguments must be a JSON object.".into(),
                        detail: "Invalid arguments".into(),
                    }
                }
            },
        };
        let offered = &self.tools[index];
        let name = offered.tool.name.clone();
        let server = &mut self.servers[offered.server];
        let result = match &mut server.backend {
            Backend::Mcp {
                url,
                token,
                client,
                session,
            } => {
                let mut result = Err(String::new());
                // A server may forget a session (it then answers 404); reconnect once.
                for attempt in 0..2 {
                    if session.is_none() {
                        match mcp::Session::open(client.clone(), url.clone(), token.clone()).await {
                            Ok(opened) => *session = Some(opened),
                            Err(error) => {
                                result = Err(error);
                                break;
                            }
                        }
                    }
                    result = session
                        .as_mut()
                        .expect("opened above")
                        .call_tool(&name, arguments.clone())
                        .await;
                    match &result {
                        Err(error) if attempt == 0 && error.contains("HTTP 404") => *session = None,
                        _ => break,
                    }
                }
                result.map(|value| mcp::result_text(&value))
            }
            Backend::Slurm(client) => client.call(&name, &arguments).await,
            Backend::Kubernetes(client) => client.call(&name, &arguments).await,
        };
        match result {
            Ok((ok, text)) => Outcome {
                ok,
                detail: clip(&text, DETAIL_OUTPUT),
                output: clip(&text, MODEL_OUTPUT),
            },
            Err(error) => Outcome {
                ok: false,
                output: format!("The {} connection failed: {error}", server.name),
                detail: error,
            },
        }
    }

    /// Ends the turn's server sessions in the background.
    pub fn close(self) {
        let sessions: Vec<_> = self
            .servers
            .into_iter()
            .filter_map(|s| match s.backend {
                Backend::Mcp { session, .. } => session,
                _ => None,
            })
            .collect();
        if !sessions.is_empty() {
            tokio::spawn(async move {
                futures_util::future::join_all(sessions.into_iter().map(mcp::Session::close)).await;
            });
        }
    }
}

/// Shown on the approval card: the arguments the model chose.
pub fn arguments_preview(arguments: &str) -> String {
    let preview = match serde_json::from_str::<Value>(arguments) {
        // One line per argument, so a manifest or script reads as itself
        // instead of a JSON string full of \n escapes.
        Ok(Value::Object(map)) if !map.is_empty() => map
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) if text.contains('\n') => {
                    format!("{key}:\n{}", text.trim_end())
                }
                Value::String(text) => format!("{key}: {text}"),
                other => format!("{key}: {other}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| arguments.to_owned()),
        Err(_) => arguments.to_owned(),
    };
    clip(&preview, 2048)
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    Allow,
    Deny,
    Expired,
}

/// Records that a tool call waits for the person.
pub async fn ask(
    pool: &PgPool,
    owner: Uuid,
    conversation: Uuid,
    generation: Uuid,
    call: &str,
    (connection, tool): (Uuid, &str),
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM tool_approvals WHERE created_at < NOW() - INTERVAL '1 day'")
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO tool_approvals(generation_id,call_id,conversation_id,owner_id,connection_id,tool_name) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
        .bind(generation).bind(call).bind(conversation).bind(owner).bind(connection).bind(tool)
        .execute(pool).await?;
    Ok(())
}

/// Whether the person chose "Allow for this chat" for this tool earlier.
pub async fn granted(pool: &PgPool, conversation: Uuid, (connection, tool): (Uuid, &str)) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tool_grants WHERE conversation_id=$1 AND connection_id=$2 AND tool_name=$3)")
        .bind(conversation).bind(connection).bind(tool)
        .fetch_one(pool).await.unwrap_or(false)
}

/// Polls for the person's decision. Stopping the response drops this future.
pub async fn wait(pool: &PgPool, generation: Uuid, call: &str) -> Decision {
    let deadline = tokio::time::Instant::now() + APPROVAL_WAIT;
    while tokio::time::Instant::now() < deadline {
        let decision: Result<Option<Option<String>>, _> = sqlx::query_scalar(
            "SELECT decision FROM tool_approvals WHERE generation_id=$1 AND call_id=$2",
        )
        .bind(generation)
        .bind(call)
        .fetch_optional(pool)
        .await;
        match decision {
            Ok(Some(Some(d))) if d == "allow" => return Decision::Allow,
            Ok(Some(Some(_))) => return Decision::Deny,
            Ok(None) => return Decision::Deny,
            _ => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Decision::Expired
}

pub async fn finish(pool: &PgPool, generation: Uuid) {
    let _ = sqlx::query("DELETE FROM tool_approvals WHERE generation_id=$1")
        .bind(generation)
        .execute(pool)
        .await;
}

fn view(row: &Row, user: &Identity) -> Value {
    let tools: Vec<Value> = row
        .tools()
        .into_iter()
        .map(|t| json!({"name":t.name,"title":t.title,"description":t.description.chars().take(240).collect::<String>(),"readOnly":t.read_only}))
        .collect();
    let settings = row.settings();
    let credential = (row.personal_tokens() && row.credential.is_some())
        .then(|| json!({"user":settings["user"],"namespace":settings["namespace"],"identity":settings["identity"]}));
    json!({
        "id": row.id, "name": row.name, "url": row.url, "kind": row.kind, "shared": row.shared(),
        "canManage": row.can_manage(user), "enabled": row.enabled, "optedOut": row.opted_out,
        "active": row.enabled && !row.opted_out && row.usable(), "usable": row.usable(),
        "needsCredential": !row.usable(), "credential": credential,
        "personalTokens": row.personal_tokens(),
        "confirmActions": row.confirm_actions, "hasToken": row.token.is_some(),
        "hasCa": row.options["ca"].is_string(), "apiVersion": row.options["apiVersion"],
        "user": settings["user"], "namespace": settings["namespace"], "identity": settings["identity"],
        "serverName": row.server_name, "checkedAt": row.checked_at, "tools": tools,
    })
}

fn unavailable() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Connections are not configured".into(),
    )
}

async fn manager(
    s: &AppState,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<(Connections, Identity), ApiError> {
    let user = s.auth.require(headers, mutation).await?;
    Ok((s.connections.clone().ok_or_else(unavailable)?, user))
}

fn clean_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err(ApiError::bad("Name the connection in 1–40 characters"));
    }
    Ok(name.to_owned())
}

/// A pasted token, without a leading "Bearer"; None when empty.
fn clean_token(token: &str) -> Result<Option<String>, ApiError> {
    let token = token.trim();
    let token = token
        .strip_prefix("Bearer ")
        .or_else(|| token.strip_prefix("bearer "))
        .unwrap_or(token)
        .trim();
    if token.is_empty() {
        return Ok(None);
    }
    if token.len() > 4096 || !token.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(ApiError::bad(
            "The token must be one line of printable characters",
        ));
    }
    Ok(Some(token.to_owned()))
}

fn unreachable_server(error: String) -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        format!("Could not connect: {error}"),
    )
}

fn duplicate(error: sqlx::Error) -> ApiError {
    match &error {
        sqlx::Error::Database(e) if e.code().as_deref() == Some("23505") => ApiError(
            StatusCode::CONFLICT,
            "A connection with this name already exists".into(),
        ),
        _ => error.into(),
    }
}

async fn audit(pool: &PgPool, actor: Uuid, action: &str, row_id: Uuid, name: &str, url: &str) {
    let _ = sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,target_id,metadata) VALUES($1,$2,$3,'connection',$4,$5)")
        .bind(Uuid::new_v4()).bind(actor).bind(action).bind(row_id.to_string()).bind(json!({"name":name,"url":url}))
        .execute(pool).await;
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/connections", get(list).post(create))
        .route("/api/connections/{id}", patch(update).delete(remove))
        .route("/api/connections/{id}/refresh", post(refresh))
        .route("/api/connections/{id}/status", get(status))
        .route(
            "/api/connections/{id}/credential",
            put(save_credential).delete(remove_credential),
        )
        .route("/api/conversations/{id}/approvals", post(decide))
        .route(
            "/api/conversations/{id}/connections",
            get(chat_connections).patch(switch_chat_connection),
        )
        .with_state(state)
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, false).await?;
    let rows = connections.visible(&user).await?;
    Ok(Json(
        json!({"connections": rows.iter().map(|r| view(r, &user)).collect::<Vec<_>>()}),
    ))
}

fn namespace_given(options: &Value) -> bool {
    options["namespace"]
        .as_str()
        .is_some_and(|n| !n.trim().is_empty())
}

/// A namespace left empty becomes a service account's own namespace, since its token
/// usually works only there.
fn settle_namespace(options: &mut Value, given: bool, identity: Option<&str>) {
    if let Some(ns) = identity
        .filter(|_| !given)
        .and_then(kubernetes::home_namespace)
    {
        options["namespace"] = json!(ns);
    }
}

/// A Kubernetes namespace or Slurm user name.
fn clean_label(value: &str, what: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 63
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(ApiError::bad(format!("Enter a valid {what}")));
    }
    Ok(value.to_owned())
}

/// Validated options for a connection kind: a CA for Kubernetes, the REST API version
/// for Slurm, and the Slurm user or namespace a person's own cluster connection uses.
fn clean_options(kind: Kind, options: &Value, personal: bool) -> Result<Value, ApiError> {
    let mut clean = json!({});
    match kind {
        Kind::Mcp => {
            if !personal && options["personalTokens"] == true {
                clean["personalTokens"] = json!(true);
            }
        }
        Kind::Kubernetes => {
            if let Some(ca) = options["ca"]
                .as_str()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                if ca.len() > 16 * 1024
                    || !ca.contains("-----BEGIN CERTIFICATE-----")
                    || reqwest::Certificate::from_pem_bundle(ca.as_bytes()).is_err()
                {
                    return Err(ApiError::bad("The CA must be PEM certificates"));
                }
                clean["ca"] = json!(ca);
            }
            if personal {
                clean["namespace"] = json!(clean_label(
                    options["namespace"]
                        .as_str()
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or("default"),
                    "namespace"
                )?);
            }
        }
        Kind::Slurm => {
            if let Some(version) = options["apiVersion"]
                .as_str()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                if !slurm::valid_version(version) {
                    return Err(ApiError::bad(
                        "The Slurm REST API version looks like v0.0.45",
                    ));
                }
                clean["apiVersion"] = json!(version);
            }
            if personal {
                clean["user"] = json!(clean_label(
                    options["user"].as_str().unwrap_or(""),
                    "Slurm user name"
                )?);
            }
        }
    }
    Ok(clean)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Create {
    name: String,
    url: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    shared: bool,
    confirm_actions: Option<bool>,
    kind: Option<String>,
    #[serde(default)]
    options: Value,
}

async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Create>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    if body.shared && !user.is_admin() {
        return Err(ApiError::forbidden());
    }
    let kind = Kind::parse(body.kind.as_deref().unwrap_or("mcp")).ok_or_else(|| {
        ApiError::bad("Choose an MCP server, a Slurm cluster or a Kubernetes cluster")
    })?;
    if kind != Kind::Mcp && !connections.clusters {
        return Err(ApiError::bad(
            "Clusters are turned off on this installation",
        ));
    }
    let name = clean_name(&body.name)?;
    let token = clean_token(&body.token)?;
    let native = kind != Kind::Mcp;
    if native && body.shared && token.is_some() {
        return Err(ApiError::bad(
            "A shared cluster takes no token: each person adds their own",
        ));
    }
    if native && !body.shared && token.is_none() {
        return Err(ApiError::bad("Enter your token for this cluster"));
    }
    let mut options = clean_options(kind, &body.options, !body.shared)?;
    let url = mcp::check_url(&body.url, body.shared || connections.private_allowed)
        .map_err(ApiError::bad)?;
    let owner = (!body.shared).then_some(user.id);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM connections WHERE owner_id IS NOT DISTINCT FROM $1",
    )
    .bind(owner)
    .fetch_one(&connections.pool)
    .await?;
    if count >= MAX_PER_OWNER {
        return Err(ApiError::bad(format!(
            "Up to {MAX_PER_OWNER} connections are allowed"
        )));
    }
    let checked = connections
        .check(kind, &url, token.clone(), &options, body.shared)
        .await
        .map_err(unreachable_server)?;
    if kind == Kind::Kubernetes && !body.shared {
        settle_namespace(
            &mut options,
            namespace_given(&body.options),
            checked.identity.as_deref(),
        );
    }
    if let Some(identity) = &checked.identity {
        options["identity"] = json!(identity);
    }
    let id = Uuid::new_v4();
    let sealed = token.as_deref().map(|t| connections.seal(id.as_bytes(), t));
    sqlx::query("INSERT INTO connections(id,owner_id,name,url,token,confirm_actions,tools,server_name,checked_at,kind,options) VALUES($1,$2,$3,$4,$5,$6,$7,$8,NOW(),$9,$10)")
        .bind(id).bind(owner).bind(&name).bind(url.as_str()).bind(sealed)
        .bind(body.confirm_actions.unwrap_or(true))
        .bind(Value::Array(checked.tools.iter().map(mcp::Tool::to_json).collect()))
        .bind(checked.server_name)
        .bind(kind.as_str())
        .bind(&options)
        .execute(&connections.pool).await.map_err(duplicate)?;
    if body.shared {
        audit(
            &connections.pool,
            user.id,
            "connection.create",
            id,
            &name,
            url.as_str(),
        )
        .await;
    }
    Ok(Json(view(&connections.find(&user, id).await?, &user)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Update {
    name: Option<String>,
    url: Option<String>,
    /// Replaces the token; an empty string removes it.
    token: Option<String>,
    enabled: Option<bool>,
    confirm_actions: Option<bool>,
    /// For a shared connection: switch it off for your own chats.
    opted_out: Option<bool>,
    /// Replaces the CA, Slurm REST API version, Slurm user or namespace.
    options: Option<Value>,
}

async fn update(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Update>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    let row = connections.find(&user, id).await?;
    let pool = &connections.pool;
    if let Some(opted_out) = body.opted_out {
        if !row.shared() {
            return Err(ApiError::bad("Switch your own connections with enabled"));
        }
        if opted_out {
            sqlx::query("INSERT INTO connection_opt_outs(user_id,connection_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(user.id).bind(id).execute(pool).await?;
        } else {
            sqlx::query("DELETE FROM connection_opt_outs WHERE user_id=$1 AND connection_id=$2")
                .bind(user.id)
                .bind(id)
                .execute(pool)
                .await?;
        }
    }
    let edits = body.name.is_some()
        || body.url.is_some()
        || body.token.is_some()
        || body.enabled.is_some()
        || body.confirm_actions.is_some()
        || body.options.is_some();
    if edits {
        if !row.can_manage(&user) {
            return Err(ApiError::forbidden());
        }
        let kind = row.kind();
        let name = body
            .name
            .as_deref()
            .map(clean_name)
            .transpose()?
            .unwrap_or(row.name.clone());
        let url = match &body.url {
            Some(raw) => mcp::check_url(raw, row.shared() || connections.private_allowed)
                .map_err(ApiError::bad)?,
            None => Url::parse(&row.url).map_err(|_| ApiError::internal())?,
        };
        let token = match &body.token {
            Some(raw) => clean_token(raw)?,
            None => row
                .token
                .as_deref()
                .and_then(|t| connections.unseal(id.as_bytes(), t)),
        };
        if kind != Kind::Mcp && row.shared() && token.is_some() {
            return Err(ApiError::bad(
                "A shared cluster takes no token: each person adds their own",
            ));
        }
        if kind != Kind::Mcp && !row.shared() && token.is_none() {
            return Err(ApiError::bad("Enter your token for this cluster"));
        }
        let mut options = match &body.options {
            Some(options) => {
                // Keep the saved CA and API version unless the edit names new ones
                // ("" removes them).
                let mut options = options.clone();
                for key in ["ca", "apiVersion"] {
                    if options.get(key).is_none() {
                        options[key] = row.options[key].clone();
                    }
                }
                clean_options(kind, &options, !row.shared())?
            }
            None => row.options.clone(),
        };
        let mut tools = row.tools.clone();
        let mut server_name = row.server_name.clone();
        let probed = url.as_str() != row.url || body.token.is_some() || body.options.is_some();
        if probed {
            let checked = connections
                .check(kind, &url, token.clone(), &options, row.shared())
                .await
                .map_err(unreachable_server)?;
            tools = Value::Array(checked.tools.iter().map(mcp::Tool::to_json).collect());
            server_name = checked.server_name;
            if let (Kind::Kubernetes, false, Some(given)) = (kind, row.shared(), &body.options) {
                settle_namespace(
                    &mut options,
                    namespace_given(given),
                    checked.identity.as_deref(),
                );
            }
            if let Some(identity) = checked.identity {
                options["identity"] = json!(identity);
            }
        }
        let sealed = match (&body.token, &token) {
            (None, _) => row.token.clone(),
            (Some(_), Some(token)) => Some(connections.seal(id.as_bytes(), token)),
            (Some(_), None) => None,
        };
        sqlx::query("UPDATE connections SET name=$2,url=$3,token=$4,enabled=$5,confirm_actions=$6,tools=$7,server_name=$8,checked_at=CASE WHEN $9 THEN NOW() ELSE checked_at END,options=$10,updated_at=NOW() WHERE id=$1")
            .bind(id).bind(&name).bind(url.as_str()).bind(sealed)
            .bind(body.enabled.unwrap_or(row.enabled))
            .bind(body.confirm_actions.unwrap_or(row.confirm_actions))
            .bind(tools).bind(server_name)
            .bind(probed)
            .bind(&options)
            .execute(pool).await.map_err(duplicate)?;
        if row.shared() {
            audit(pool, user.id, "connection.update", id, &name, url.as_str()).await;
        }
    }
    Ok(Json(view(&connections.find(&user, id).await?, &user)))
}

async fn refresh(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    let row = connections.find(&user, id).await?;
    if !row.can_manage(&user) {
        return Err(ApiError::forbidden());
    }
    let url = Url::parse(&row.url).map_err(|_| ApiError::internal())?;
    let token = match &row.token {
        Some(sealed) => Some(connections.unseal(id.as_bytes(), sealed).ok_or_else(|| {
            ApiError::bad("The saved token can no longer be read. Enter it again.")
        })?),
        None => None,
    };
    let checked = connections
        .check(row.kind(), &url, token, &row.options, row.shared())
        .await
        .map_err(unreachable_server)?;
    sqlx::query("UPDATE connections SET tools=$2,server_name=$3,checked_at=NOW() WHERE id=$1")
        .bind(id)
        .bind(Value::Array(
            checked.tools.iter().map(mcp::Tool::to_json).collect(),
        ))
        .bind(checked.server_name)
        .execute(&connections.pool)
        .await?;
    Ok(Json(view(&connections.find(&user, id).await?, &user)))
}

/// The live state of a Slurm or Kubernetes cluster, as the viewer sees it.
async fn status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, false).await?;
    let row = connections.find(&user, id).await?;
    if row.kind() == Kind::Mcp {
        return Err(ApiError::bad("Only clusters report a status"));
    }
    if !row.usable() {
        return Ok(Json(json!({"state":"needsCredential"})));
    }
    let url = Url::parse(&row.url).map_err(|_| ApiError::internal())?;
    let Some(token) = connections.token_for(&row, user.id) else {
        return Ok(Json(
            json!({"state":"error","error":"The saved token can no longer be read. Enter it again."}),
        ));
    };
    let result = match connections.cluster_client(&row, &url, token) {
        Ok(Native::Slurm(mut client)) => client.status().await,
        Ok(Native::Kubernetes(client)) => client.status().await,
        Err(error) => Err(error),
    };
    Ok(Json(match result {
        Ok(status) => {
            json!({"state":"connected","status":status,"checkedAt":chrono::Utc::now()})
        }
        Err(error) => json!({"state":"error","error":error}),
    }))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    let row = connections.find(&user, id).await?;
    if !row.can_manage(&user) {
        return Err(ApiError::forbidden());
    }
    sqlx::query("DELETE FROM connections WHERE id=$1")
        .bind(id)
        .execute(&connections.pool)
        .await?;
    if row.shared() {
        audit(
            &connections.pool,
            user.id,
            "connection.delete",
            id,
            &row.name,
            &row.url,
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Credential {
    token: String,
    /// The Slurm user name the token belongs to.
    user: Option<String>,
    /// The Kubernetes namespace tools use when none is named. Empty: a service
    /// account's own namespace, or "default".
    namespace: Option<String>,
}

/// Saves a person's token for a shared Slurm or Kubernetes cluster, or a shared MCP
/// server set to personal tokens, after checking it.
async fn save_credential(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Credential>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    let row = connections.find(&user, id).await?;
    let kind = row.kind();
    if !row.personal_tokens() || !row.enabled {
        return Err(ApiError::bad(
            "Only shared clusters and shared servers set to personal tokens take your own token",
        ));
    }
    let token = clean_token(&body.token)?.ok_or_else(|| ApiError::bad("Enter your token"))?;
    let mut options = match kind {
        Kind::Mcp => json!({}),
        Kind::Slurm => {
            json!({"user": clean_label(body.user.as_deref().unwrap_or(""), "Slurm user name")?})
        }
        _ => {
            let namespace = body
                .namespace
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty());
            json!({"namespace": clean_label(namespace.unwrap_or("default"), "namespace")?})
        }
    };
    let url = Url::parse(&row.url).map_err(|_| ApiError::internal())?;
    let mut check_options = options.clone();
    check_options["ca"] = row.options["ca"].clone();
    check_options["apiVersion"] = row.options["apiVersion"].clone();
    let checked = connections
        .check(kind, &url, Some(token.clone()), &check_options, true)
        .await
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    if kind == Kind::Kubernetes {
        let given = body
            .namespace
            .as_deref()
            .is_some_and(|n| !n.trim().is_empty());
        settle_namespace(&mut options, given, checked.identity.as_deref());
    }
    if kind != Kind::Mcp {
        options["identity"] = json!(checked.identity);
    }
    let sealed = connections.seal(&credential_aad(id, user.id), &token);
    sqlx::query("INSERT INTO connection_credentials(connection_id,user_id,token,options) VALUES($1,$2,$3,$4) ON CONFLICT (connection_id,user_id) DO UPDATE SET token=EXCLUDED.token,options=EXCLUDED.options,updated_at=NOW()")
        .bind(id).bind(user.id).bind(sealed).bind(&options)
        .execute(&connections.pool).await?;
    Ok(Json(view(&connections.find(&user, id).await?, &user)))
}

async fn remove_credential(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    connections.find(&user, id).await?;
    sqlx::query("DELETE FROM connection_credentials WHERE connection_id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.id)
        .execute(&connections.pool)
        .await?;
    Ok(Json(view(&connections.find(&user, id).await?, &user)))
}

/// The person's active connections and whether each is on in this conversation.
async fn chat_connections(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(conversation): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let (connections, user) = manager(&s, &headers, false).await?;
    owned(&connections.pool, user.id, conversation).await?;
    let off: Vec<Uuid> = sqlx::query_scalar(
        "SELECT connection_id FROM conversation_connections_off WHERE conversation_id=$1",
    )
    .bind(conversation)
    .fetch_all(&connections.pool)
    .await?;
    let list: Vec<Value> = connections
        .visible(&user)
        .await?
        .iter()
        .filter(|r| r.enabled && !r.opted_out && r.usable())
        .map(|r| json!({"id":r.id,"name":r.name,"kind":r.kind,"shared":r.shared(),"tools":r.tools().len(),"on":!off.contains(&r.id)}))
        .collect();
    let auto = connections.auto(conversation).await;
    Ok(Json(
        json!({"connections": list, "auto": auto, "autoAllowed": connections.auto_mode}),
    ))
}

/// Switches one connection on or off in a chat, or the chat's Auto mode.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Switch {
    connection_id: Option<Uuid>,
    on: Option<bool>,
    auto: Option<bool>,
}

async fn switch_chat_connection(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(conversation): Path<Uuid>,
    Json(body): Json<Switch>,
) -> Result<StatusCode, ApiError> {
    let (connections, user) = manager(&s, &headers, true).await?;
    owned(&connections.pool, user.id, conversation).await?;
    if let Some(auto) = body.auto {
        if auto && !connections.auto_mode {
            return Err(ApiError::bad(
                "Auto mode is turned off on this installation",
            ));
        }
        sqlx::query("UPDATE conversations SET auto_approve=$2 WHERE id=$1")
            .bind(conversation)
            .bind(auto)
            .execute(&connections.pool)
            .await?;
    }
    let (Some(connection), Some(on)) = (body.connection_id, body.on) else {
        return if body.auto.is_some() {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(ApiError::bad("Name a connection and whether it is on"))
        };
    };
    connections.find(&user, connection).await?;
    let query = if on {
        "DELETE FROM conversation_connections_off WHERE conversation_id=$1 AND connection_id=$2"
    } else {
        "INSERT INTO conversation_connections_off(conversation_id,connection_id) VALUES($1,$2) ON CONFLICT DO NOTHING"
    };
    sqlx::query(query)
        .bind(conversation)
        .bind(connection)
        .execute(&connections.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn owned(pool: &PgPool, user: Uuid, conversation: Uuid) -> Result<(), ApiError> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1 AND owner_id=$2)",
    )
    .bind(conversation)
    .bind(user)
    .fetch_one(pool)
    .await?;
    if found {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Approval {
    call_id: String,
    decision: String,
    /// With allow: run this tool without asking for the rest of the conversation.
    #[serde(default)]
    remember: bool,
}

async fn decide(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(conversation): Path<Uuid>,
    Json(body): Json<Approval>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    if !matches!(body.decision.as_str(), "allow" | "deny") {
        return Err(ApiError::bad("Choose allow or deny"));
    }
    let mut tx = s.store.pool.begin().await?;
    let decided: Option<(Option<Uuid>, Option<String>)> = sqlx::query_as("UPDATE tool_approvals SET decision=$4 WHERE conversation_id=$1 AND call_id=$2 AND owner_id=$3 AND decision IS NULL RETURNING connection_id,tool_name")
        .bind(conversation).bind(&body.call_id).bind(user.id).bind(&body.decision)
        .fetch_optional(&mut *tx).await?;
    let Some(tool) = decided else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This step is no longer waiting for an answer".into(),
        ));
    };
    match (body.remember, body.decision.as_str(), tool) {
        (true, "allow", (Some(connection), Some(name))) => {
            sqlx::query("INSERT INTO tool_grants(conversation_id,connection_id,tool_name) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(conversation).bind(connection).bind(name)
                .execute(&mut *tx).await?;
        }
        // A browser submission: "Always on this site".
        (true, "allow", (None, Some(name))) => {
            crate::browser::remember_site(&mut tx, user.id, &name).await?;
        }
        _ => {}
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_preview_shows_multiline_text_as_is() {
        let preview = arguments_preview(
            r#"{"namespace":"lab","manifest":"kind: Deployment\nmetadata:\n  name: hello\n","replicas":1}"#,
        );
        assert_eq!(
            preview,
            "manifest:\nkind: Deployment\nmetadata:\n  name: hello\nnamespace: lab\nreplicas: 1"
        );
        assert_eq!(arguments_preview("[1,2]"), "[\n  1,\n  2\n]");
        assert_eq!(arguments_preview("not json"), "not json");
    }

    #[test]
    fn prefixes_are_tool_name_safe() {
        assert_eq!(prefix("Docs"), "docs");
        assert_eq!(prefix("Lab HPC · Slurm!"), "lab_hpc_slurm");
        assert_eq!(prefix("  ***  "), "mcp");
        assert_eq!(
            prefix("a-very-long-connection-name-indeed"),
            "a_very_long_connection_n"
        );
    }

    #[test]
    fn tokens_are_cleaned() {
        assert_eq!(
            clean_token("  Bearer abc123 ").unwrap().as_deref(),
            Some("abc123")
        );
        assert_eq!(clean_token("").unwrap(), None);
        assert!(clean_token("two words").is_err());
    }

    #[test]
    fn clipping_keeps_utf8_whole() {
        assert_eq!(clip("short", 10), "short");
        let clipped = clip("ééééé", 3);
        assert!(clipped.starts_with('é') && clipped.contains("more bytes"));
    }
}
