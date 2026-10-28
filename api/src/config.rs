use anyhow::{bail, Context, Result};
use std::{env, net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub listen: SocketAddr,
    pub static_dir: PathBuf,
    pub gateway_url: Option<Url>,
    pub admin_gateway_url: Option<String>,
    pub signing_key_file: Option<PathBuf>,
    /// Whose token gateway calls carry: Chat's signed one, or the person's own.
    pub gateway_auth: GatewayAuth,
    pub gateway_issuer: String,
    pub gateway_audience: String,
    pub max_streams: usize,
    pub db_connections: u32,
    pub backup_status_dir: Option<PathBuf>,
    /// When set, `/metrics` needs `Authorization: Bearer` with this token.
    pub metrics_token: Option<String>,
    /// Lets people's own connections reach private addresses. For development.
    pub connections_allow_private: bool,
    /// Which connections people can add: none, MCP servers only, or also clusters.
    pub connections: ConnectionsMode,
    /// Whether a chat may switch to Auto mode, where tools run without asking.
    pub auto_mode: bool,
    /// OBLETH_CHAT_MAX_FILE_MB: the largest file or document one upload may be.
    pub max_file_mb: u32,
    /// OBLETH_CHAT_FILE_QUOTA_MB: one person's attached files in all. Unset, it
    /// follows where files are kept (see `attachments::Limits`).
    pub file_quota_mb: Option<u32>,
    /// OBLETH_CHAT_MAX_FILES: how many files one person may keep.
    pub max_files: u32,
}
/// OBLETH_CHAT_GATEWAY_AUTH: `sign` (the default) sends a token Chat signs with
/// its gateway signing key; `forward` sends the access token the identity provider
/// issued to the signed-in person.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GatewayAuth {
    Sign,
    Forward,
}
impl GatewayAuth {
    pub fn from_env() -> Result<Self> {
        match env::var("OBLETH_CHAT_GATEWAY_AUTH")
            .ok()
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            None | Some("sign") => Ok(Self::Sign),
            Some("forward") => Ok(Self::Forward),
            Some(_) => bail!("OBLETH_CHAT_GATEWAY_AUTH must be sign or forward"),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectionsMode {
    Off,
    Mcp,
    All,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let origin =
            env::var("OBLETH_CHAT_ORIGIN").unwrap_or_else(|_| "http://localhost:8085".into());
        let gateway_url = env::var("OBLETH_CHAT_GATEWAY_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| Url::parse(&s))
            .transpose()
            .context("Invalid gateway URL")?;
        if let Some(u) = &gateway_url {
            if !["http", "https"].contains(&u.scheme())
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
                || !["", "/"].contains(&u.path())
            {
                bail!("OBLETH_CHAT_GATEWAY_URL must be an http(s) service origin without credentials, path, query or fragment");
            }
        }
        let admin_gateway_url = env::var("OBLETH_CHAT_ADMIN_GATEWAY_URL")
            .ok()
            .filter(|v| !v.is_empty());
        if let Some(value) = &admin_gateway_url {
            let url = Url::parse(value).context("Invalid admin gateway URL")?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                bail!("Admin gateway URL must be an http(s) URL without credentials");
            }
        }
        let signing_key_file = env::var("OBLETH_CHAT_GATEWAY_SIGNING_KEY_FILE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let gateway_auth = GatewayAuth::from_env()?;
        // Forwarding people's own tokens needs no signing key; one that is set still
        // signs for local accounts.
        match (gateway_url.is_some(), signing_key_file.is_some(), gateway_auth) {
            (true, true, _) | (false, false, GatewayAuth::Sign) | (true, false, GatewayAuth::Forward) => {}
            (false, _, GatewayAuth::Forward) => {
                bail!("OBLETH_CHAT_GATEWAY_AUTH=forward needs OBLETH_CHAT_GATEWAY_URL")
            }
            _ => bail!("Configure both OBLETH_CHAT_GATEWAY_URL and OBLETH_CHAT_GATEWAY_SIGNING_KEY_FILE, or neither"),
        }
        let gateway_issuer = env::var("OBLETH_CHAT_GATEWAY_ISSUER").unwrap_or(origin);
        let issuer_url = Url::parse(&gateway_issuer).context("Invalid gateway issuer URL")?;
        if !["https", "http"].contains(&issuer_url.scheme())
            || issuer_url.query().is_some()
            || issuer_url.fragment().is_some()
            || !issuer_url.username().is_empty()
            || issuer_url.password().is_some()
        {
            bail!("Gateway issuer must be a credential-free http(s) URL");
        }
        let gateway_audience =
            env::var("OBLETH_CHAT_GATEWAY_AUDIENCE").unwrap_or_else(|_| "obleth".into());
        if gateway_audience.trim().is_empty() {
            bail!("Gateway audience must not be empty");
        }
        Ok(Self {
            database_url: env::var("OBLETH_CHAT_DATABASE_URL")
                .context("Set OBLETH_CHAT_DATABASE_URL to a Postgres database owned by Chat")?,
            listen: env::var("OBLETH_CHAT_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:8085".into())
                .parse()
                .context("Invalid OBLETH_CHAT_LISTEN")?,
            static_dir: env::var("OBLETH_CHAT_STATIC_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("web")),
            gateway_url,
            admin_gateway_url,
            signing_key_file,
            gateway_auth,
            gateway_issuer,
            gateway_audience,
            max_streams: number("OBLETH_CHAT_MAX_STREAMS", 128, 1, 4096)? as usize,
            db_connections: number("OBLETH_CHAT_DB_CONNECTIONS", 20, 2, 200)?,
            backup_status_dir: env::var("OBLETH_CHAT_BACKUP_STATUS_DIR")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            metrics_token: env::var("OBLETH_CHAT_METRICS_TOKEN")
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
            connections_allow_private: env::var("OBLETH_CHAT_CONNECTIONS_ALLOW_PRIVATE")
                .is_ok_and(|v| v == "true"),
            connections: match env::var("OBLETH_CHAT_CONNECTIONS")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .as_deref()
                .map(str::trim)
            {
                None | Some("all") => ConnectionsMode::All,
                Some("mcp") => ConnectionsMode::Mcp,
                Some("off") => ConnectionsMode::Off,
                Some(_) => bail!("OBLETH_CHAT_CONNECTIONS must be all, mcp or off"),
            },
            auto_mode: match env::var("OBLETH_CHAT_AUTO_MODE")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .as_deref()
                .map(str::trim)
            {
                None | Some("allowed") => true,
                Some("off") => false,
                Some(_) => bail!("OBLETH_CHAT_AUTO_MODE must be allowed or off"),
            },
            max_file_mb: number("OBLETH_CHAT_MAX_FILE_MB", 50, 1, 2000)?,
            file_quota_mb: env::var("OBLETH_CHAT_FILE_QUOTA_MB")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|_| number("OBLETH_CHAT_FILE_QUOTA_MB", 0, 1, 10_000_000))
                .transpose()?,
            max_files: number("OBLETH_CHAT_MAX_FILES", 500, 1, 1_000_000)?,
        })
    }
}
fn number(key: &str, default: u32, min: u32, max: u32) -> Result<u32> {
    let n = env::var(key)
        .ok()
        .map(|s| s.parse::<u32>())
        .transpose()
        .with_context(|| format!("Invalid {key}"))?
        .unwrap_or(default);
    if !(min..=max).contains(&n) {
        bail!("{key} must be between {min} and {max}");
    }
    Ok(n)
}
