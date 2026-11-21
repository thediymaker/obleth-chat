use anyhow::{bail, Context, Result};
use std::{net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub listen: SocketAddr,
    /// OBLETH_CHAT_ORIGIN, the address people open Chat at.
    pub origin: Url,
    /// The site's branding (`site.config.js` and `assets/`), which installs replace.
    pub branding_dir: PathBuf,
    /// OBLETH_CHAT_UI_URL: the web interface server Chat passes pages to.
    pub ui_url: Option<Url>,
    pub gateway_url: Option<Url>,
    pub admin_gateway_url: Option<String>,
    pub signing_key_file: Option<PathBuf>,
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
    /// OBLETH_CHAT_KNOWLEDGE_USER_MB: one person's knowledge documents in all.
    pub knowledge_quota_mb: u32,
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
        Ok(env::choice(
            "OBLETH_CHAT_GATEWAY_AUTH",
            &[("sign", Self::Sign), ("forward", Self::Forward)],
        )?
        .unwrap_or(Self::Sign))
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectionsMode {
    Off,
    Mcp,
    All,
}
/// Where Chat runs when OBLETH_CHAT_ORIGIN is unset.
const DEFAULT_ORIGIN: &str = "http://localhost:8085";
impl Config {
    pub fn from_env() -> Result<Self> {
        let raw_origin = env::text("OBLETH_CHAT_ORIGIN").unwrap_or_else(|| DEFAULT_ORIGIN.into());
        let origin = Url::parse(raw_origin.trim()).context("OBLETH_CHAT_ORIGIN must be a URL")?;
        let gateway_url = env::text("OBLETH_CHAT_GATEWAY_URL")
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
        let admin_gateway_url = env::text("OBLETH_CHAT_ADMIN_GATEWAY_URL");
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
        let ui_url = env::text("OBLETH_CHAT_UI_URL")
            .map(|raw| Url::parse(raw.trim()))
            .transpose()
            .context("OBLETH_CHAT_UI_URL must be a URL")?;
        if ui_url
            .as_ref()
            .is_some_and(|url| !matches!(url.scheme(), "http" | "https"))
        {
            bail!("OBLETH_CHAT_UI_URL must be an http or https address");
        }
        let signing_key_file = env::text("OBLETH_CHAT_GATEWAY_SIGNING_KEY_FILE").map(PathBuf::from);
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
        // The issuer is the origin as written, which gateways may already trust.
        let gateway_issuer = env::text("OBLETH_CHAT_GATEWAY_ISSUER").unwrap_or(raw_origin);
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
            env::text("OBLETH_CHAT_GATEWAY_AUDIENCE").unwrap_or_else(|| "obleth".into());
        Ok(Self {
            database_url: env::text("OBLETH_CHAT_DATABASE_URL")
                .context("Set OBLETH_CHAT_DATABASE_URL to a Postgres database owned by Chat")?,
            listen: env::text("OBLETH_CHAT_LISTEN")
                .unwrap_or_else(|| "127.0.0.1:8085".into())
                .trim()
                .parse()
                .context("Invalid OBLETH_CHAT_LISTEN")?,
            origin,
            branding_dir: env::text("OBLETH_CHAT_BRANDING_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("branding")),
            ui_url,
            gateway_url,
            admin_gateway_url,
            signing_key_file,
            gateway_issuer,
            gateway_audience,
            max_streams: env::number("OBLETH_CHAT_MAX_STREAMS", 1..=4096)?.unwrap_or(128),
            db_connections: env::number("OBLETH_CHAT_DB_CONNECTIONS", 2..=200)?.unwrap_or(20),
            backup_status_dir: env::text("OBLETH_CHAT_BACKUP_STATUS_DIR").map(PathBuf::from),
            metrics_token: env::text("OBLETH_CHAT_METRICS_TOKEN").map(|v| v.trim().to_owned()),
            connections_allow_private: env::flag("OBLETH_CHAT_CONNECTIONS_ALLOW_PRIVATE")?
                .unwrap_or(false),
            connections: env::choice(
                "OBLETH_CHAT_CONNECTIONS",
                &[
                    ("all", ConnectionsMode::All),
                    ("mcp", ConnectionsMode::Mcp),
                    ("off", ConnectionsMode::Off),
                ],
            )?
            .unwrap_or(ConnectionsMode::All),
            auto_mode: env::choice(
                "OBLETH_CHAT_AUTO_MODE",
                &[("allowed", true), ("off", false)],
            )?
            .unwrap_or(true),
            max_file_mb: env::number("OBLETH_CHAT_MAX_FILE_MB", 1..=2000)?.unwrap_or(50),
            file_quota_mb: env::number("OBLETH_CHAT_FILE_QUOTA_MB", 1..=10_000_000)?,
            max_files: env::number("OBLETH_CHAT_MAX_FILES", 1..=1_000_000)?.unwrap_or(500),
            knowledge_quota_mb: env::number("OBLETH_CHAT_KNOWLEDGE_USER_MB", 1..=10_000_000)?
                .unwrap_or(2048),
        })
    }
}

/// Settings from the environment, read one way everywhere. Blank is the same as
/// unset, so a deployment can pass every setting through (Compose's `${KEY:-}`) and
/// leave most empty. A value that is set but invalid is an error naming the
/// setting, which stops startup: it is never quietly read as the default.
pub mod env {
    use anyhow::{bail, Result};
    use std::{fmt::Display, ops::RangeInclusive, str::FromStr};

    /// A text setting, as written and untrimmed: a prefix may end in a space.
    pub fn text(key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.trim().is_empty())
    }

    /// An on/off setting: `true` or `1`, `false` or `0`.
    pub fn flag(key: &str) -> Result<Option<bool>> {
        match text(key).as_deref().map(str::trim) {
            None => Ok(None),
            Some("true" | "1") => Ok(Some(true)),
            Some("false" | "0") => Ok(Some(false)),
            Some(_) => bail!("{key} must be true or false"),
        }
    }

    /// A whole number in `range`.
    pub fn number<T>(key: &str, range: RangeInclusive<T>) -> Result<Option<T>>
    where
        T: FromStr + PartialOrd + Display,
    {
        let Some(value) = text(key) else {
            return Ok(None);
        };
        match value.trim().parse::<T>() {
            Ok(n) if range.contains(&n) => Ok(Some(n)),
            _ => bail!(
                "{key} must be a whole number between {} and {}",
                range.start(),
                range.end()
            ),
        }
    }

    /// One of `options`, each a value as written and what it means.
    pub fn choice<T: Copy>(key: &str, options: &[(&str, T)]) -> Result<Option<T>> {
        let Some(value) = text(key) else {
            return Ok(None);
        };
        match options.iter().find(|(name, _)| *name == value.trim()) {
            Some((_, meaning)) => Ok(Some(*meaning)),
            None => {
                let names: Vec<&str> = options.iter().map(|(name, _)| *name).collect();
                let (last, rest) = names.split_last().expect("a choice has options");
                match rest {
                    [] => bail!("{key} must be {last}"),
                    _ => bail!("{key} must be {} or {last}", rest.join(", ")),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::env::{choice, flag, number, text};
    use std::env::set_var;

    // Each test owns its keys: the environment is shared by every test thread.
    #[test]
    fn a_blank_setting_is_unset() {
        set_var("OBLETH_CHAT_TEST_VAR_EMPTY", "");
        set_var("OBLETH_CHAT_TEST_VAR_SPACES", "  ");
        set_var("OBLETH_CHAT_TEST_VAR_SET", " query: ");
        assert_eq!(text("OBLETH_CHAT_TEST_VAR_EMPTY"), None);
        assert_eq!(text("OBLETH_CHAT_TEST_VAR_SPACES"), None);
        assert_eq!(text("OBLETH_CHAT_TEST_VAR_UNSET"), None);
        // A value is kept as written: a prefix may end in a space.
        assert_eq!(
            text("OBLETH_CHAT_TEST_VAR_SET").as_deref(),
            Some(" query: ")
        );
    }

    #[test]
    fn numbers_are_unset_when_blank_and_refuse_bad_values() {
        set_var("OBLETH_CHAT_TEST_NUMBER_EMPTY", "");
        set_var("OBLETH_CHAT_TEST_NUMBER_SET", " 40 ");
        set_var("OBLETH_CHAT_TEST_NUMBER_HIGH", "500");
        set_var("OBLETH_CHAT_TEST_NUMBER_WORD", "lots");
        let read = |key| number::<u32>(key, 2..=200);
        assert_eq!(read("OBLETH_CHAT_TEST_NUMBER_EMPTY").unwrap(), None);
        assert_eq!(read("OBLETH_CHAT_TEST_NUMBER_UNSET").unwrap(), None);
        assert_eq!(read("OBLETH_CHAT_TEST_NUMBER_SET").unwrap(), Some(40));
        let high = read("OBLETH_CHAT_TEST_NUMBER_HIGH")
            .unwrap_err()
            .to_string();
        assert_eq!(
            high,
            "OBLETH_CHAT_TEST_NUMBER_HIGH must be a whole number between 2 and 200"
        );
        let word = read("OBLETH_CHAT_TEST_NUMBER_WORD")
            .unwrap_err()
            .to_string();
        assert!(word.starts_with("OBLETH_CHAT_TEST_NUMBER_WORD "), "{word}");
    }

    #[test]
    fn flags_take_true_false_one_and_zero_only() {
        for (value, meaning) in [
            ("true", Some(true)),
            (" false ", Some(false)),
            ("1", Some(true)),
            ("0", Some(false)),
            ("", None),
        ] {
            set_var("OBLETH_CHAT_TEST_FLAG", value);
            assert_eq!(flag("OBLETH_CHAT_TEST_FLAG").unwrap(), meaning, "{value:?}");
        }
        for value in ["yes", "on", "TRUE", "2"] {
            set_var("OBLETH_CHAT_TEST_FLAG", value);
            assert_eq!(
                flag("OBLETH_CHAT_TEST_FLAG").unwrap_err().to_string(),
                "OBLETH_CHAT_TEST_FLAG must be true or false",
                "{value:?}"
            );
        }
        assert_eq!(flag("OBLETH_CHAT_TEST_FLAG_UNSET").unwrap(), None);
    }

    #[test]
    fn choices_name_what_they_accept() {
        let options = [("all", 2), ("mcp", 1), ("off", 0)];
        set_var("OBLETH_CHAT_TEST_CHOICE", " mcp ");
        assert_eq!(
            choice("OBLETH_CHAT_TEST_CHOICE", &options).unwrap(),
            Some(1)
        );
        set_var("OBLETH_CHAT_TEST_CHOICE", "some");
        assert_eq!(
            choice("OBLETH_CHAT_TEST_CHOICE", &options)
                .unwrap_err()
                .to_string(),
            "OBLETH_CHAT_TEST_CHOICE must be all, mcp or off"
        );
        set_var("OBLETH_CHAT_TEST_CHOICE", "");
        assert_eq!(choice("OBLETH_CHAT_TEST_CHOICE", &options).unwrap(), None);
    }
}
