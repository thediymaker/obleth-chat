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
    pub gateway_issuer: String,
    pub gateway_audience: String,
    pub max_streams: usize,
    pub db_connections: u32,
    pub backup_status_dir: Option<PathBuf>,
    /// Lets people's own connections reach private addresses. For development.
    pub connections_allow_private: bool,
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
        if gateway_url.is_some() != signing_key_file.is_some() {
            bail!("Configure both OBLETH_CHAT_GATEWAY_URL and OBLETH_CHAT_GATEWAY_SIGNING_KEY_FILE, or neither");
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
            gateway_issuer,
            gateway_audience,
            max_streams: number("OBLETH_CHAT_MAX_STREAMS", 128, 1, 4096)? as usize,
            db_connections: number("OBLETH_CHAT_DB_CONNECTIONS", 20, 2, 200)?,
            backup_status_dir: env::var("OBLETH_CHAT_BACKUP_STATUS_DIR")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            connections_allow_private: env::var("OBLETH_CHAT_CONNECTIONS_ALLOW_PRIVATE")
                .is_ok_and(|v| v == "true"),
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
