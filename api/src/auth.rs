//! Local accounts are always available; OIDC is an optional additional sign-in method.
//! Browser credentials are opaque, revocable server-side sessions. Identity and roles
//! always come from Postgres, never from a request header or browser-stored claim.

use std::{
    convert::Infallible,
    env,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{ConnectInfo, FromRequestParts, Path, Query, State},
    http::{header, request::Parts, Extensions, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef},
    FromRow, PgPool, Postgres, Transaction,
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use url::Url;
use uuid::Uuid;

use crate::{error::ApiError, secrets::SecretKey};
pub(crate) mod accounts;
pub mod grants;
mod group_policy;
mod oidc;
#[cfg(test)]
mod tests;
pub mod tokens;

#[derive(Clone)]
pub struct AuthConfig {
    pub origin: String,
    pub session_hours: u32,
    pub bootstrap_username: Option<String>,
    pub bootstrap_password: Option<String>,
    pub oidc: Option<OidcConfig>,
    pub login_rate_per_minute: u32,
    /// Whether people may use passwords at all (OBLETH_CHAT_PASSWORD_SIGN_IN).
    pub password_sign_in: bool,
    /// Keep each sign-in's provider tokens and send them to the gateway instead of
    /// Chat's own signed token (OBLETH_CHAT_GATEWAY_AUTH=forward).
    pub forward_tokens: bool,
    /// Seals stored provider tokens (OBLETH_CHAT_SECRET_KEY_FILE).
    pub secret_key: Option<SecretKey>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            origin: "http://localhost:8085".into(),
            session_hours: 12,
            bootstrap_username: None,
            bootstrap_password: None,
            oidc: None,
            login_rate_per_minute: 300,
            password_sign_in: true,
            forward_tokens: false,
            secret_key: None,
        }
    }
}

#[derive(Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub allow_signup: bool,
    /// Requested scopes; always includes `openid`.
    pub scopes: Vec<String>,
}

/// Longest session that does not follow a provider grant.
pub const PASSWORD_SESSION_HOURS: u32 = 168;

/// The scopes Chat asks for when OBLETH_CHAT_OIDC_SCOPES is not set.
pub const DEFAULT_OIDC_SCOPES: &str = "openid profile";

/// Space-separated scopes, with `openid` always included. Each must be a scope token
/// (RFC 6749 3.3).
pub fn parse_scopes(value: &str) -> anyhow::Result<Vec<String>> {
    let mut scopes = vec!["openid".to_owned()];
    for scope in value.split_whitespace() {
        if !scope
            .bytes()
            .all(|b| b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
        {
            bail!("OBLETH_CHAT_OIDC_SCOPES must list space-separated scope names");
        }
        if !scopes.iter().any(|s| s == scope) {
            scopes.push(scope.to_owned());
        }
    }
    Ok(scopes)
}

impl AuthConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let origin =
            env::var("OBLETH_CHAT_ORIGIN").unwrap_or_else(|_| "http://localhost:8085".into());
        let issuer = optional_env("OBLETH_CHAT_OIDC_ISSUER");
        let client_id = optional_env("OBLETH_CHAT_OIDC_CLIENT_ID");
        let client_secret = optional_env("OBLETH_CHAT_OIDC_CLIENT_SECRET");
        let allow_signup = optional_env("OBLETH_CHAT_OIDC_ALLOW_SIGNUP");
        let scopes = optional_env("OBLETH_CHAT_OIDC_SCOPES");
        let oidc = match (issuer, client_id, client_secret) {
            (None, None, None) if allow_signup.is_none() && scopes.is_none() => None,
            (Some(issuer), Some(client_id), Some(client_secret)) => Some(OidcConfig {
                issuer, client_id, client_secret,
                allow_signup: match allow_signup.as_deref().unwrap_or("false") {
                    "true" => true,
                    "false" => false,
                    _ => bail!("OBLETH_CHAT_OIDC_ALLOW_SIGNUP must be true or false"),
                },
                scopes: parse_scopes(scopes.as_deref().unwrap_or(DEFAULT_OIDC_SCOPES))?,
            }),
            _ => bail!("OIDC requires OBLETH_CHAT_OIDC_ISSUER, OBLETH_CHAT_OIDC_CLIENT_ID and OBLETH_CHAT_OIDC_CLIENT_SECRET"),
        };
        let config = Self {
            origin,
            session_hours: optional_env("OBLETH_CHAT_SESSION_HOURS")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(12),
            bootstrap_username: optional_env("OBLETH_CHAT_BOOTSTRAP_USERNAME"),
            bootstrap_password: optional_env("OBLETH_CHAT_BOOTSTRAP_PASSWORD"),
            oidc,
            login_rate_per_minute: optional_env("OBLETH_CHAT_LOGIN_RATE_PER_MINUTE")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(300),
            password_sign_in: match optional_env("OBLETH_CHAT_PASSWORD_SIGN_IN").as_deref() {
                None | Some("true") => true,
                Some("false") => false,
                Some(_) => bail!("OBLETH_CHAT_PASSWORD_SIGN_IN must be true or false"),
            },
            forward_tokens: crate::config::GatewayAuth::from_env()?
                == crate::config::GatewayAuth::Forward,
            secret_key: optional_env("OBLETH_CHAT_SECRET_KEY_FILE")
                .map(|path| SecretKey::from_file(std::path::Path::new(&path)))
                .transpose()?,
        };
        config.validated()
    }

    fn validated(mut self) -> anyhow::Result<Self> {
        let url = Url::parse(&self.origin).context("OBLETH_CHAT_ORIGIN must be a URL")?;
        validate_transport(&url)?;
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            bail!("OBLETH_CHAT_ORIGIN must be an origin without a path, query or fragment");
        }
        self.origin = url.origin().ascii_serialization();
        // With forwarded tokens the provider ends a sign-in whenever it ends the
        // grant, so those sessions may be set to last much longer. Password
        // sessions stay capped at PASSWORD_SESSION_HOURS either way.
        let max_hours = if self.forward_tokens {
            8760
        } else {
            PASSWORD_SESSION_HOURS
        };
        if !(1..=max_hours).contains(&self.session_hours) {
            bail!("OBLETH_CHAT_SESSION_HOURS must be between 1 and {max_hours}");
        }
        if !(1..=10_000).contains(&self.login_rate_per_minute) {
            bail!("OBLETH_CHAT_LOGIN_RATE_PER_MINUTE must be between 1 and 10000");
        }
        match (&self.bootstrap_username, &self.bootstrap_password) {
            (None, None) => (),
            (Some(username), Some(password)) => {
                self.bootstrap_username =
                    Some(normalize_username(username).map_err(|e| anyhow::anyhow!(e.1))?);
                validate_password(password).map_err(|e| anyhow::anyhow!(e.1))?;
            }
            _ => {
                bail!("Set both OBLETH_CHAT_BOOTSTRAP_USERNAME and OBLETH_CHAT_BOOTSTRAP_PASSWORD")
            }
        }
        if let Some(config) = &self.oidc {
            let issuer = Url::parse(&config.issuer).context("OIDC issuer must be a URL")?;
            validate_transport(&issuer)?;
            if issuer.query().is_some() || issuer.fragment().is_some() {
                bail!("OIDC issuer cannot have a query or fragment");
            }
            if config.client_id.is_empty() || config.client_secret.is_empty() {
                bail!("OIDC client ID and secret cannot be empty");
            }
            if !config.scopes.iter().any(|s| s == "openid") {
                bail!("OIDC scopes must include openid");
            }
        }
        if !self.password_sign_in {
            if self.oidc.is_none() {
                bail!("OBLETH_CHAT_PASSWORD_SIGN_IN=false needs OIDC sign-in configured");
            }
            // Refused rather than ignored: the password would otherwise sit in the
            // environment, ready to work again the day passwords are turned back on.
            if self.bootstrap_password.is_some() {
                bail!("Remove OBLETH_CHAT_BOOTSTRAP_USERNAME and OBLETH_CHAT_BOOTSTRAP_PASSWORD when OBLETH_CHAT_PASSWORD_SIGN_IN=false");
            }
        }
        if self.forward_tokens {
            let Some(config) = &self.oidc else {
                bail!("OBLETH_CHAT_GATEWAY_AUTH=forward needs OIDC sign-in configured");
            };
            if !config.scopes.iter().any(|s| s == "offline_access") {
                bail!("OBLETH_CHAT_GATEWAY_AUTH=forward needs offline_access in OBLETH_CHAT_OIDC_SCOPES");
            }
            if self.secret_key.is_none() {
                bail!("OBLETH_CHAT_GATEWAY_AUTH=forward needs OBLETH_CHAT_SECRET_KEY_FILE");
            }
        }
        Ok(self)
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

fn validate_transport(url: &Url) -> anyhow::Result<()> {
    let loopback = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        bail!("Use HTTPS; HTTP is allowed only for an explicit loopback host");
    }
    Ok(())
}

/// Unrefunded password attempts per account from one client, and from all clients, per minute.
const CLIENT_ACCOUNT_FAILURES: i32 = 10;
const ACCOUNT_FAILURES: i32 = 50;
/// Attempts per account and minute from clients not failing there, on top of
/// `ACCOUNT_FAILURES`, so rotating addresses cannot add guesses without bound.
const ACCOUNT_FIRST_TRIES: i32 = 50;
/// How long, in one-minute windows, a client's unrefunded attempt on an account
/// marks it as failing there. Throttle rows are kept this long too.
const FAILING_WINDOWS: i64 = 10;

#[derive(Clone)]
pub struct Auth {
    pool: PgPool,
    config: Arc<AuthConfig>,
    oidc: Option<Arc<oidc::Oidc>>,
    /// Provider tokens kept per sign-in, when they are forwarded to the gateway.
    grants: Option<grants::Grants>,
    dummy_password_hash: Arc<String>,
    password_slots: Arc<Semaphore>,
    group_policy: Arc<group_policy::GroupPolicy>,
    trusted_proxies: Arc<Vec<(IpAddr, u8)>>,
}

/// Throttling identity of the caller: the peer address, or the nearest untrusted
/// X-Forwarded-For hop when the peer is a configured trusted proxy.
pub(crate) struct Client(String);

impl FromRequestParts<Auth> for Client {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, auth: &Auth) -> Result<Self, Infallible> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip());
        Ok(Self(client_key(
            &auth.trusted_proxies,
            peer,
            &parts.headers,
        )))
    }
}

/// Account role. JSON and the `users.role` TEXT column (CHECK user/admin) both use
/// the lowercase names; an unexpected stored value is a decode error, never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
        }
    }
    fn parse(role: &str) -> Option<Self> {
        match role {
            "user" => Some(Self::User),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
    /// Request-supplied role from account administration; exact lowercase names only.
    pub(crate) fn requested(role: Option<&str>) -> Result<Option<Self>, ApiError> {
        role.map(|r| Self::parse(r).ok_or_else(|| ApiError::bad("Role must be user or admin")))
            .transpose()
    }
}

impl sqlx::Type<Postgres> for Role {
    fn type_info() -> PgTypeInfo {
        <str as sqlx::Type<Postgres>>::type_info()
    }
    fn compatible(ty: &PgTypeInfo) -> bool {
        <str as sqlx::Type<Postgres>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, Postgres> for Role {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <&str as sqlx::Encode<Postgres>>::encode_by_ref(&self.as_str(), buf)
    }
}

impl sqlx::Decode<'_, Postgres> for Role {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        let role = <&str as sqlx::Decode<Postgres>>::decode(value)?;
        Self::parse(role).ok_or_else(|| format!("unknown account role {role:?}").into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Identity {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    /// The browser session a request came from, or the one an API token was made
    /// in. Gateway calls carry that sign-in's own provider token. Never serialized.
    #[serde(skip)]
    #[sqlx(default)]
    pub session: Option<Uuid>,
}

impl Identity {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

#[derive(FromRow)]
struct PasswordUser {
    id: Uuid,
    username: String,
    display_name: String,
    role: Role,
    password_hash: Option<String>,
    enabled: bool,
    password_change_required: bool,
}

#[derive(FromRow)]
struct SessionIdentity {
    #[sqlx(flatten)]
    user: Identity,
    password_change_required: bool,
}

impl PasswordUser {
    fn identity(&self) -> Identity {
        Identity {
            id: self.id,
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            role: self.role,
            session: None,
        }
    }
}

impl Auth {
    pub async fn new(pool: PgPool, config: AuthConfig) -> anyhow::Result<Self> {
        let config = config.validated()?;
        let group_policy = group_policy::GroupPolicy::from_env(config.oidc.is_some())?;
        let trusted_proxies =
            parse_trusted_proxies(optional_env("OBLETH_CHAT_TRUSTED_PROXY").as_deref())?;
        group_policy.validate(&pool).await?;
        let oidc = match &config.oidc {
            Some(provider) => Some(Arc::new(oidc::Oidc::new(provider, &config.origin).await?)),
            None => None,
        };
        let grants = match (
            &oidc,
            &config.oidc,
            &config.secret_key,
            config.forward_tokens,
        ) {
            (Some(oidc), Some(provider), Some(key), true) => Some(grants::Grants::new(
                pool.clone(),
                key.clone(),
                oidc.clone(),
                provider.issuer.clone(),
            )),
            _ => None,
        };
        let dummy_password_hash = hash_password(random_token())
            .await
            .map_err(|_| anyhow::anyhow!("Password hashing failed"))?;
        let mut auth = Self {
            pool,
            config: Arc::new(config),
            oidc,
            grants,
            dummy_password_hash: Arc::new(dummy_password_hash),
            password_slots: Arc::new(Semaphore::new(4)),
            group_policy: Arc::new(group_policy),
            trusted_proxies: Arc::new(trusted_proxies),
        };
        auth.bootstrap().await?;
        if !auth.config.password_sign_in {
            // A forced change replaces a temporary password; with passwords off no
            // one can use or change one, and the flag would also hold back their
            // single sign-on.
            let cleared = sqlx::query(
                "UPDATE users SET password_change_required = FALSE WHERE password_change_required",
            )
            .execute(&auth.pool)
            .await?
            .rows_affected();
            if cleared > 0 {
                tracing::info!(
                    cleared,
                    "passwords are off; no account waits on a password change"
                );
            }
            // Sign-ins made with a password, and the API tokens made in them, end
            // too. Requests also refuse them, for any a replica that still allows
            // passwords makes during a rolling update.
            let mut tx = auth.pool.begin().await?;
            let sessions = sqlx::query("DELETE FROM auth_sessions WHERE NOT single_sign_on")
                .execute(&mut *tx)
                .await?
                .rows_affected();
            let tokens = sqlx::query(
                "UPDATE api_tokens SET revoked_at = NOW() WHERE NOT single_sign_on AND revoked_at IS NULL",
            )
            .execute(&mut *tx)
            .await?
            .rows_affected();
            tx.commit().await?;
            if sessions + tokens > 0 {
                tracing::info!(
                    sessions,
                    tokens,
                    "passwords are off; password sign-ins ended"
                );
                auth.after_sign_out();
            }
        }
        if let Some(config) = Arc::get_mut(&mut auth.config) {
            config.bootstrap_username = None;
            config.bootstrap_password = None;
        }
        Ok(auth)
    }

    pub fn oidc_enabled(&self) -> bool {
        self.oidc.is_some()
    }

    /// The provider tokens kept per sign-in, when Chat forwards them to the gateway.
    pub fn grants(&self) -> Option<grants::Grants> {
        self.grants.clone()
    }

    /// After a change that ended sign-ins commits, revokes their provider tokens in
    /// the background, so signing out never waits on the provider.
    pub(crate) fn after_sign_out(&self) {
        if let Some(grants) = self.grants.clone() {
            tokio::spawn(async move { grants.revoke_ended().await });
        }
    }

    /// Seconds a session lasts when it did not begin with the identity provider.
    /// Only a sign-in whose grant Chat keeps may last longer than the 168 hours
    /// sessions were always capped at: the provider ends that one when it ends the
    /// grant, while nothing outside Chat ends a password session.
    fn password_session_lifetime(&self) -> i64 {
        self.config.session_hours.min(PASSWORD_SESSION_HOURS) as i64 * 3600
    }

    /// Whether people may sign in with, set or reset a password.
    pub fn password_sign_in(&self) -> bool {
        self.config.password_sign_in
    }

    /// Every password path checks this first, so turning passwords off closes them
    /// all at once.
    fn require_passwords(&self) -> Result<(), ApiError> {
        if self.config.password_sign_in {
            Ok(())
        } else {
            Err(ApiError(
                StatusCode::FORBIDDEN,
                "Passwords are turned off; sign in with single sign-on".into(),
            ))
        }
    }
    pub fn client_report(&self, extensions: &Extensions, headers: &HeaderMap) -> Value {
        let peer = extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip());
        client_report(&self.trusted_proxies, peer, headers)
    }

    pub fn origin(&self) -> &str {
        &self.config.origin
    }

    pub fn router(&self) -> Router<()> {
        Router::new()
            .route("/api/session", get(session))
            .route("/api/auth/login", post(login))
            .route("/api/auth/logout", post(logout))
            .route("/api/auth/password", post(change_password))
            .route("/api/admin/users", get(list_users).post(create_user))
            .route("/api/admin/users/{id}", patch(update_user))
            .route("/api/admin/users/{id}/password", post(reset_password))
            .route("/api/admin/oidc-identities", post(link_oidc_identity))
            .route("/api/auth/oidc/login", get(oidc::login))
            .route("/api/auth/oidc/callback", get(oidc::callback))
            .merge(accounts::router())
            .merge(tokens::router())
            .with_state(self.clone())
    }

    pub async fn require(&self, headers: &HeaderMap, mutation: bool) -> Result<Identity, ApiError> {
        self.require_session(headers, mutation, false).await
    }

    async fn require_session(
        &self,
        headers: &HeaderMap,
        mutation: bool,
        allow_password_change: bool,
    ) -> Result<Identity, ApiError> {
        if mutation {
            self.check_origin(headers)?;
        }
        let token = self
            .session_token(headers)
            .ok_or_else(ApiError::unauthorized)?;
        // With passwords off, only sessions that began with single sign-on count.
        let identity = sqlx::query_as::<_, SessionIdentity>(
            "SELECT u.id, u.username, u.display_name, u.role, s.id AS session, u.password_change_required FROM auth_sessions s \
             JOIN users u ON u.id = s.user_id WHERE s.token_hash = $1 \
             AND s.expires_at > NOW() AND u.enabled = TRUE AND (s.single_sign_on OR $2)",
        )
        .bind(digest(&token))
        .bind(self.config.password_sign_in)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
        if mutation {
            let supplied = headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let expected = csrf_token(&token);
            if supplied.len() != expected.len()
                || !bool::from(supplied.as_bytes().ct_eq(expected.as_bytes()))
            {
                return Err(ApiError::forbidden());
            }
        }
        if identity.password_change_required && !allow_password_change {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "Change your password before continuing".into(),
            ));
        }
        self.confirm_grant(identity.user.session).await?;
        Ok(identity.user)
    }

    /// In forward mode a sign-in lasts only as long as the provider keeps its grant:
    /// once the access token has expired, the next request refreshes it, and a
    /// refused grant ends the session (and the API tokens made in it) right away.
    /// Only activity refreshes, so the provider's idle expiry still applies.
    pub(crate) async fn confirm_grant(&self, session: Option<Uuid>) -> Result<(), ApiError> {
        match (&self.grants, session) {
            (Some(grants), Some(session)) => {
                if grants.confirm(session).await? {
                    Ok(())
                } else {
                    Err(ApiError::unauthorized())
                }
            }
            _ => Ok(()),
        }
    }

    pub(crate) async fn require_admin(
        &self,
        headers: &HeaderMap,
        mutation: bool,
    ) -> Result<Identity, ApiError> {
        let identity = self.require(headers, mutation).await?;
        if !identity.is_admin() {
            return Err(ApiError::forbidden());
        }
        Ok(identity)
    }

    async fn lock_admin(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        admin_id: Uuid,
    ) -> Result<(), ApiError> {
        // A serialized recheck prevents simultaneous administrators disabling one
        // another and prevents a revoked administrator finishing a queued mutation.
        sqlx::query("SELECT pg_advisory_xact_lock(7346281951102)")
            .execute(&mut **tx)
            .await?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND enabled AND role = 'admin' AND NOT password_change_required)",
        )
        .bind(admin_id)
        .fetch_one(&mut **tx)
        .await?;
        if !active {
            return Err(ApiError::forbidden());
        }
        Ok(())
    }

    fn check_origin(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        if headers.get_all(header::ORIGIN).iter().count() != 1
            || headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
                != Some(&self.config.origin)
        {
            return Err(ApiError::forbidden());
        }
        Ok(())
    }

    fn session_cookie_name(&self) -> &'static str {
        if self.secure() {
            "__Host-obleth_chat"
        } else {
            "obleth_chat_session"
        }
    }

    fn oidc_cookie_name(&self) -> &'static str {
        if self.secure() {
            "__Host-obleth_oidc"
        } else {
            "obleth_oidc"
        }
    }

    fn secure(&self) -> bool {
        self.config.origin.starts_with("https://")
    }

    fn cookie(&self, name: &str, token: &str, max_age: i64) -> HeaderValue {
        HeaderValue::from_str(&format!(
            "{name}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
            if self.secure() { "; Secure" } else { "" }
        ))
        .expect("cookie contains only internal ASCII values")
    }

    fn session_token(&self, headers: &HeaderMap) -> Option<String> {
        cookie_value(headers, self.session_cookie_name())
    }

    /// The id of the live browser session these headers carry, so access granted
    /// elsewhere can end with it.
    pub(crate) async fn session_id(&self, headers: &HeaderMap) -> Result<Option<Uuid>, ApiError> {
        let Some(token) = self.session_token(headers) else {
            return Ok(None);
        };
        Ok(sqlx::query_scalar(
            "SELECT id FROM auth_sessions WHERE token_hash = $1 AND expires_at > NOW()",
        )
        .bind(digest(&token))
        .fetch_optional(&self.pool)
        .await?)
    }

    fn session_response(
        &self,
        user: Identity,
        token: &str,
        lifetime: i64,
        password_change_required: bool,
    ) -> Response {
        let mut response = Json(json!({"authenticated": true, "user": user, "csrfToken": csrf_token(token), "oidcEnabled": self.oidc_enabled(), "passwordSignIn": self.password_sign_in(), "passwordChangeRequired": password_change_required})).into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            self.cookie(self.session_cookie_name(), token, lifetime),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }

    async fn bootstrap(&self) -> anyhow::Result<()> {
        let (Some(username), Some(password)) = (
            &self.config.bootstrap_username,
            &self.config.bootstrap_password,
        ) else {
            return Ok(());
        };
        // All replicas take the same transaction lock before testing database emptiness.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(7346281951101)")
            .execute(&mut *tx)
            .await?;
        sqlx::query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
            .fetch_one(&mut *tx)
            .await?;
        if !exists {
            let password_hash = hash_password(password.clone())
                .await
                .map_err(|_| anyhow::anyhow!("Password hashing failed"))?;
            sqlx::query("INSERT INTO users (id, username, display_name, role, password_hash) VALUES ($1, $2, $2, $3, $4)")
                .bind(Uuid::new_v4()).bind(username).bind(Role::Admin).bind(password_hash).execute(&mut *tx).await?;
            tracing::info!("Created the configured bootstrap administrator");
        }
        tx.commit().await?;
        Ok(())
    }

    /// Waits briefly for an Argon2 slot so a short burst queues instead of failing.
    async fn password_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
        tokio::time::timeout(
            Duration::from_secs(2),
            self.password_slots.clone().acquire_owned(),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(too_many_requests)
    }

    async fn check_password(&self, password: String, encoded: String) -> Result<bool, ApiError> {
        let permit = self.password_permit().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            PasswordHash::new(&encoded)
                .ok()
                .map(|hash| {
                    Argon2::default()
                        .verify_password(password.as_bytes(), &hash)
                        .is_ok()
                })
                .unwrap_or(false)
        })
        .await
        .map_err(|_| ApiError::internal())
    }

    async fn new_password_hash(&self, password: String) -> Result<String, ApiError> {
        let permit = self.password_permit().await?;
        tokio::task::spawn_blocking(move || {
            // Keep the CPU/memory permit inside the blocking task, including when
            // its originating HTTP request is cancelled.
            let _permit = permit;
            let salt = SaltString::generate(&mut OsRng);
            Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|_| ApiError::internal())
        })
        .await
        .map_err(|_| ApiError::internal())?
    }

    /// Every request counts toward its client's per-minute budget. Account attempts
    /// are counted before verification, so concurrent guesses cannot overrun the
    /// limits, and a verified password refunds its own attempt. A client stopped by
    /// its account limit does not spend the higher all-client backstop, so one client
    /// cannot lock another out.
    ///
    /// The all-client backstop only stops clients that are failing on the account
    /// themselves. A client with no unrefunded attempt there in the last
    /// `FAILING_WINDOWS` minutes is counted against a second account limit instead
    /// (`ACCOUNT_FIRST_TRIES` a minute), so guesses from failing addresses cannot keep
    /// the real person from signing in, and rotating addresses still adds at most
    /// that many guesses a minute. A refused attempt leaves the client's failing
    /// state as it was, so retrying while refused does not extend it.
    async fn throttle(
        &self,
        scope: &str,
        client: &Client,
        account: Option<&str>,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let window = throttle_window(&mut tx).await?;
        sqlx::query("DELETE FROM login_throttles WHERE window_start < $1")
            .bind(window - FAILING_WINDOWS)
            .execute(&mut *tx)
            .await?;
        let requests = throttle_count(&mut tx, &format!("{scope}:{}", client.0), window).await?;
        let mut allowed = requests <= self.config.login_rate_per_minute as i32;
        if let (true, Some(account)) = (allowed, account) {
            let (account_key, client_key) = failure_keys(client, account);
            let failing_key = failing_key(client, account);
            allowed = throttle_take(&mut tx, &client_key, window, CLIENT_ACCOUNT_FAILURES)
                .await?
                .is_some();
            if allowed {
                // One statement per key, so concurrent attempts from a client with
                // no recent failures see exactly one of them as its first.
                let (marks, before) = throttle_mark(&mut tx, &failing_key, window).await?;
                let counted = if marks > 1 {
                    throttle_take(&mut tx, &account_key, window, ACCOUNT_FAILURES).await?
                } else {
                    let first = first_tries_key(account);
                    let taken = throttle_take(&mut tx, &first, window, ACCOUNT_FIRST_TRIES).await?;
                    if taken == Some(ACCOUNT_FIRST_TRIES) {
                        tracing::warn!(
                            "an account reached its limit of sign-ins from new addresses for this minute"
                        );
                    }
                    match taken {
                        Some(_) => Some(throttle_count(&mut tx, &account_key, window).await?),
                        None => None,
                    }
                };
                if counted == Some(ACCOUNT_FAILURES) {
                    tracing::warn!("an account reached its sign-in failure limit for this minute");
                }
                if counted.is_none() {
                    allowed = false;
                    throttle_release(&mut tx, &[client_key], window).await?;
                    throttle_unmark(&mut tx, &failing_key, before).await?;
                }
            }
        }
        tx.commit().await?;
        if !allowed {
            return Err(too_many_requests());
        }
        Ok(())
    }

    /// A verified password returns the attempt it took from both account limits.
    /// It never clears them, so a user sharing a client address cannot reset
    /// someone else's guesses.
    async fn throttle_refund(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        client: &Client,
        account: &str,
    ) -> Result<(), ApiError> {
        let window = throttle_window(tx).await?;
        let (account_key, client_key) = failure_keys(client, account);
        throttle_release(tx, &[account_key, client_key], window).await?;
        throttle_unmark(tx, &failing_key(client, account), None).await
    }

    async fn insert_session(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        token: &str,
        lifetime: i64,
        headers: &HeaderMap,
        single_sign_on: bool,
    ) -> Result<Uuid, ApiError> {
        sqlx::query("DELETE FROM auth_sessions WHERE expires_at <= NOW()")
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM auth_sessions WHERE id IN (SELECT id FROM auth_sessions WHERE user_id=$1 ORDER BY created_at DESC,id DESC OFFSET 31)")
            .bind(user_id).execute(&mut **tx).await?;
        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(256)
            .collect::<String>();
        Ok(sqlx::query_scalar("INSERT INTO auth_sessions (token_hash, user_id, expires_at, user_agent, single_sign_on) VALUES ($1, $2, NOW() + make_interval(secs => $3), $4, $5) RETURNING id")
            .bind(digest(token)).bind(user_id).bind(lifetime as f64).bind(user_agent).bind(single_sign_on).fetch_one(&mut **tx).await?)
    }
}

fn too_many_requests() -> ApiError {
    ApiError(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many sign-in attempts; try again shortly".into(),
    )
}

async fn throttle_window(tx: &mut Transaction<'_, Postgres>) -> Result<i64, ApiError> {
    Ok(
        sqlx::query_scalar("SELECT FLOOR(EXTRACT(EPOCH FROM NOW()) / 60)::BIGINT")
            .fetch_one(&mut **tx)
            .await?,
    )
}

fn failure_keys(client: &Client, account: &str) -> (String, String) {
    let account = URL_SAFE_NO_PAD.encode(digest(account));
    (
        format!("account:{account}"),
        format!("account:{account}:{}", client.0),
    )
}

/// Counts a client's unrefunded attempts on an account across `FAILING_WINDOWS`.
fn failing_key(client: &Client, account: &str) -> String {
    let account = URL_SAFE_NO_PAD.encode(digest(account));
    format!("failing:{account}:{}", client.0)
}

/// Counts an account's attempts from clients not failing on it, per minute.
fn first_tries_key(account: &str) -> String {
    format!("first:{}", URL_SAFE_NO_PAD.encode(digest(account)))
}

/// `loopback` stands for a reverse proxy on the same host. There is deliberately no
/// preset for private ranges: home and office clients use them too, and a trusted
/// client can choose its own throttling address.
const LOOPBACK: &[&str] = &["127.0.0.0/8", "::1"];

fn parse_trusted_proxies(value: Option<&str>) -> anyhow::Result<Vec<(IpAddr, u8)>> {
    value
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .flat_map(|entry| {
            if entry.eq_ignore_ascii_case("loopback") {
                LOOPBACK.to_vec()
            } else {
                vec![entry]
            }
        })
        .map(|entry| {
            let (address, prefix) = match entry.split_once('/') {
                Some((address, prefix)) => (address, Some(prefix)),
                None => (entry, None),
            };
            let address = address.parse::<IpAddr>().ok();
            let width = if address.is_some_and(|ip| ip.is_ipv4()) { 32 } else { 128 };
            let prefix = match prefix {
                Some(prefix) => prefix.parse::<u8>().ok().filter(|p| *p <= width),
                None => Some(width),
            };
            // An IPv4-mapped range such as ::ffff:10.0.0.0/104 becomes 10.0.0.0/8.
            let (address, prefix) = match (address, prefix) {
                (Some(ip), Some(p)) if ip.is_ipv6() && ip.to_canonical().is_ipv4() => {
                    (Some(ip.to_canonical()), p.checked_sub(96))
                }
                pair => pair,
            };
            match (address, prefix) {
                (Some(address), Some(prefix)) => Ok((address, prefix)),
                _ => bail!(
                    "OBLETH_CHAT_TRUSTED_PROXY must list IP addresses, CIDR ranges or `loopback`, separated by commas"
                ),
            }
        })
        .collect()
}

fn forwarded_hops(headers: &HeaderMap) -> Vec<&str> {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .map(|v| v.to_str())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect()
}

fn client_address(trusted: &[(IpAddr, u8)], peer: IpAddr, headers: &HeaderMap) -> IpAddr {
    let mut client = peer.to_canonical();
    let trusted_proxy = |ip| trusted.iter().any(|network| in_network(ip, *network));
    if trusted_proxy(client) {
        // Walk back from the nearest hop; anything left of an untrusted or
        // malformed entry can be client-supplied.
        for hop in forwarded_hops(headers).into_iter().rev() {
            let Ok(ip) = hop.parse::<IpAddr>() else {
                break;
            };
            client = ip.to_canonical();
            if !trusted_proxy(client) {
                break;
            }
        }
    }
    client
}

/// Explains how this request's client address was derived, so an administrator
/// can check OBLETH_CHAT_TRUSTED_PROXY against the real proxy chain.
fn client_report(trusted: &[(IpAddr, u8)], peer: Option<IpAddr>, headers: &HeaderMap) -> Value {
    let trusted_proxy = |ip| trusted.iter().any(|network| in_network(ip, *network));
    let hops = forwarded_hops(headers);
    let Some(peer) = peer.map(|ip| ip.to_canonical()) else {
        return json!({"peer":null,"resolved":null,"status":"unknown","trustedProxyConfigured":!trusted.is_empty()});
    };
    let resolved = client_address(trusted, peer, headers);
    let has_header = headers.contains_key("x-forwarded-for");
    let status = if !trusted_proxy(peer) {
        if has_header {
            "untrusted-proxy"
        } else if local(peer) {
            // A same-host or container-network peer is usually a proxy that
            // does not forward the client address.
            "local-peer"
        } else {
            "direct"
        }
    } else if !has_header {
        "missing-header"
    } else if hops.last().is_none_or(|hop| hop.parse::<IpAddr>().is_err()) {
        "malformed-header"
    } else if trusted_proxy(resolved) {
        "chain-ends-trusted"
    } else {
        "forwarded"
    };
    json!({
        "peer": peer.to_string(),
        "peerTrusted": trusted_proxy(peer),
        "forwardedFor": hops.iter().take(16).map(|hop| hop.chars().take(64).collect::<String>()).collect::<Vec<_>>(),
        "resolved": resolved.to_string(),
        "status": status,
        "trustedProxyConfigured": !trusted.is_empty(),
    })
}

fn local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn client_key(trusted: &[(IpAddr, u8)], peer: Option<IpAddr>, headers: &HeaderMap) -> String {
    let Some(peer) = peer else {
        return "unknown".into();
    };
    let client = client_address(trusted, peer, headers);
    // One IPv6 host can rotate through its whole /64.
    let bucket = match client {
        IpAddr::V6(ip) => format!(
            "{}/64",
            Ipv6Addr::from(u128::from(ip) & !(u64::MAX as u128))
        ),
        IpAddr::V4(ip) => ip.to_string(),
    };
    URL_SAFE_NO_PAD.encode(digest(&bucket))
}

fn in_network(ip: IpAddr, (network, prefix): (IpAddr, u8)) -> bool {
    let bits = |ip: IpAddr| match ip.to_canonical() {
        IpAddr::V4(ip) => (u128::from(u32::from(ip)), 32),
        IpAddr::V6(ip) => (u128::from(ip), 128),
    };
    let ((ip, width), (network, network_width)) = (bits(ip), bits(network));
    let prefix = u32::from(prefix);
    width == network_width && (prefix == 0 || (ip ^ network) >> (width - prefix) == 0)
}

async fn throttle_count(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    window: i64,
) -> Result<i32, ApiError> {
    Ok(sqlx::query_scalar("INSERT INTO login_throttles (key, window_start, attempts) VALUES ($1, $2, 1) \
        ON CONFLICT (key) DO UPDATE SET window_start = $2, \
        attempts = CASE WHEN login_throttles.window_start = $2 THEN LEAST(login_throttles.attempts + 1, 1000000) ELSE 1 END \
        RETURNING attempts").bind(key).bind(window).fetch_one(&mut **tx).await?)
}

/// Counts one attempt unless the key has already reached `limit` in this window,
/// and returns the count including it.
async fn throttle_take(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    window: i64,
    limit: i32,
) -> Result<Option<i32>, ApiError> {
    let counted: Option<i32> = sqlx::query_scalar("INSERT INTO login_throttles (key, window_start, attempts) VALUES ($1, $2, 1) \
        ON CONFLICT (key) DO UPDATE SET window_start = $2, \
        attempts = CASE WHEN login_throttles.window_start = $2 THEN login_throttles.attempts + 1 ELSE 1 END \
        WHERE login_throttles.window_start <> $2 OR login_throttles.attempts < $3 \
        RETURNING attempts").bind(key).bind(window).bind(limit).fetch_optional(&mut **tx).await?;
    Ok(counted)
}

/// Adds an attempt to a key that keeps counting until it has been quiet for
/// `FAILING_WINDOWS`. Returns the count including this attempt, and the window of
/// the attempt before it, for `throttle_unmark`.
async fn throttle_mark(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    window: i64,
) -> Result<(i32, Option<i64>), ApiError> {
    Ok(sqlx::query_as("WITH before AS (SELECT window_start FROM login_throttles WHERE key = $1) \
        INSERT INTO login_throttles (key, window_start, attempts) VALUES ($1, $2, 1) \
        ON CONFLICT (key) DO UPDATE SET window_start = $2, \
        attempts = CASE WHEN login_throttles.window_start >= $2 - $3 THEN LEAST(login_throttles.attempts + 1, 1000000) ELSE 1 END \
        RETURNING attempts, (SELECT window_start FROM before)").bind(key).bind(window).bind(FAILING_WINDOWS).fetch_one(&mut **tx).await?)
}

/// Takes back one attempt from a `throttle_mark` key. Given the window it had
/// before a refused attempt, it also puts that back, so the refused attempt does
/// not restart the quiet time the key decays after.
async fn throttle_unmark(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    before: Option<i64>,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE login_throttles SET attempts = attempts - 1, window_start = COALESCE($2, window_start) \
         WHERE key = $1 AND attempts > 0",
    )
    .bind(key)
    .bind(before)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn throttle_release(
    tx: &mut Transaction<'_, Postgres>,
    keys: &[String],
    window: i64,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE login_throttles SET attempts = attempts - 1 WHERE key = ANY($1) AND window_start = $2 AND attempts > 0")
        .bind(keys)
        .bind(window)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The person signed in by session `id`, while the session is unexpired and the
/// account enabled and not waiting on a password change. Sign-out, session
/// revocation and disabling the account end it.
pub(crate) async fn session_owner(pool: &PgPool, id: Uuid) -> Result<Option<Identity>, ApiError> {
    Ok(sqlx::query_as::<_, Identity>(
        "SELECT u.id, u.username, u.display_name, u.role, s.id AS session FROM auth_sessions s \
         JOIN users u ON u.id = s.user_id WHERE s.id = $1 \
         AND s.expires_at > NOW() AND u.enabled = TRUE AND NOT u.password_change_required",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Ends every way a person is signed in: all their browser sessions and personal
/// API tokens. Every path that resets someone's credentials or access calls this,
/// so nothing made with the old credentials outlives the reset. Returns how many
/// sessions ended.
pub(crate) async fn end_sign_ins(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
) -> Result<u64, ApiError> {
    let sessions = sqlx::query("DELETE FROM auth_sessions WHERE user_id = $1")
        .bind(user)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    tokens::revoke_all(tx, user).await?;
    Ok(sessions)
}

/// Ends one sign-in: its browser session and the personal API tokens made in it.
pub(crate) async fn end_session(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    session: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM auth_sessions WHERE id = $1 AND user_id = $2")
        .bind(session)
        .bind(user)
        .execute(&mut **tx)
        .await?;
    tokens::revoke_made_in(tx, user, &[session]).await?;
    Ok(())
}

fn digest(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}
fn csrf_token(session: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest(&format!("obleth-chat-csrf:{session}")))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut result = None;
    for header in headers.get_all(header::COOKIE) {
        for value in header.to_str().ok()?.split(';') {
            if let Some((key, value)) = value.trim().split_once('=') {
                if key == name {
                    // Reject duplicates to prevent ambiguous cookie selection.
                    if result.is_some()
                        || value.len() != 43
                        || !value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                    {
                        return None;
                    }
                    result = Some(value.to_string());
                }
            }
        }
    }
    result
}

fn normalize_username(username: &str) -> Result<String, ApiError> {
    let username = username.trim().to_ascii_lowercase();
    if !(3..=64).contains(&username.len())
        || !username
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'@'))
    {
        return Err(ApiError::bad(
            "Username must be 3–64 letters, digits, dots, underscores, hyphens or @ characters",
        ));
    }
    Ok(username)
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < 12 || password.len() > 1024 {
        return Err(ApiError::bad(
            "Password must contain at least 12 characters and at most 1024 bytes",
        ));
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(ApiError::bad("Display name must contain 1–100 characters"));
    }
    Ok(name.to_owned())
}

async fn hash_password(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| ApiError::internal())
    })
    .await
    .map_err(|_| ApiError::internal())?
}

async fn session(State(auth): State<Auth>, headers: HeaderMap) -> Result<Response, ApiError> {
    let body = match auth.require_session(&headers, false, true).await {
        Ok(user) => {
            let password_change_required: bool =
                sqlx::query_scalar("SELECT password_change_required FROM users WHERE id=$1")
                    .bind(user.id)
                    .fetch_one(&auth.pool)
                    .await?;
            json!({"authenticated": true, "user": user, "csrfToken": csrf_token(&auth.session_token(&headers).ok_or_else(ApiError::unauthorized)?), "oidcEnabled": auth.oidc_enabled(), "passwordSignIn": auth.password_sign_in(), "passwordChangeRequired": password_change_required})
        }
        Err(error) if error.0 == StatusCode::UNAUTHORIZED => {
            json!({"authenticated": false, "oidcEnabled": auth.oidc_enabled(), "passwordSignIn": auth.password_sign_in()})
        }
        Err(error) => return Err(error),
    };
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response())
}

#[derive(Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}

async fn login(
    State(auth): State<Auth>,
    client: Client,
    headers: HeaderMap,
    Json(input): Json<LoginInput>,
) -> Result<Response, ApiError> {
    auth.check_origin(&headers)?;
    auth.require_passwords()?;
    let username = normalize_username(&input.username).unwrap_or_else(|_| "!invalid".into());
    auth.throttle("login", &client, Some(&username)).await?;
    if input.password.len() > 1024 {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password".into(),
        ));
    }
    let user = sqlx::query_as::<_, PasswordUser>("SELECT id, username, display_name, role, password_hash, enabled, password_change_required FROM users WHERE username = $1")
        .bind(&username).fetch_optional(&auth.pool).await?;
    let encoded = user
        .as_ref()
        .and_then(|u| u.password_hash.clone())
        .unwrap_or_else(|| auth.dummy_password_hash.as_ref().clone());
    let verified = auth.check_password(input.password, encoded.clone()).await?;
    if !verified
        || user
            .as_ref()
            .is_none_or(|u| !u.enabled || u.password_hash.is_none())
    {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password".into(),
        ));
    }
    let mut tx = auth.pool.begin().await?;
    let user = sqlx::query_as::<_, PasswordUser>("SELECT id, username, display_name, role, password_hash, enabled, password_change_required FROM users WHERE username = $1 FOR UPDATE")
        .bind(&username).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::unauthorized)?;
    // A concurrent reset or disable cannot create a session using stale credentials.
    if !user.enabled || user.password_hash.as_deref() != Some(&encoded) {
        return Err(ApiError::unauthorized());
    }
    auth.throttle_refund(&mut tx, &client, &username).await?;
    if let Some(old_token) = auth.session_token(&headers) {
        sqlx::query("DELETE FROM auth_sessions WHERE token_hash = $1")
            .bind(digest(&old_token))
            .execute(&mut *tx)
            .await?;
    }
    let token = random_token();
    let lifetime = auth.password_session_lifetime();
    auth.insert_session(&mut tx, user.id, &token, lifetime, &headers, false)
        .await?;
    tx.commit().await?;
    auth.after_sign_out();
    Ok(auth.session_response(
        user.identity(),
        &token,
        lifetime,
        user.password_change_required,
    ))
}

async fn logout(State(auth): State<Auth>, headers: HeaderMap) -> Result<Response, ApiError> {
    auth.require_session(&headers, true, true).await?;
    let token = auth
        .session_token(&headers)
        .ok_or_else(ApiError::unauthorized)?;
    sqlx::query("DELETE FROM auth_sessions WHERE token_hash = $1")
        .bind(digest(&token))
        .execute(&auth.pool)
        .await?;
    auth.after_sign_out();
    Ok((
        [(
            header::SET_COOKIE,
            auth.cookie(auth.session_cookie_name(), "", 0),
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordInput {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(auth): State<Auth>,
    client: Client,
    headers: HeaderMap,
    Json(input): Json<PasswordInput>,
) -> Result<Response, ApiError> {
    let identity = auth.require_session(&headers, true, true).await?;
    auth.require_passwords()?;
    validate_password(&input.new_password)?;
    if input.current_password.len() > 1024 {
        return Err(ApiError::bad("Current password is incorrect"));
    }
    auth.throttle("login", &client, Some(&identity.username))
        .await?;
    let encoded: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1 AND enabled")
            .bind(identity.id)
            .fetch_optional(&auth.pool)
            .await?
            .flatten();
    let Some(encoded) = encoded else {
        return Err(ApiError::bad("This account signs in through OIDC"));
    };
    if !auth
        .check_password(input.current_password, encoded.clone())
        .await?
    {
        return Err(ApiError::bad("Current password is incorrect"));
    }
    let next_hash = auth.new_password_hash(input.new_password).await?;
    let mut tx = auth.pool.begin().await?;
    let updated = sqlx::query("UPDATE users SET password_hash = $1, password_change_required = FALSE, updated_at = NOW() WHERE id = $2 AND password_hash = $3 AND enabled")
        .bind(next_hash).bind(identity.id).bind(encoded).execute(&mut *tx).await?.rows_affected();
    if updated != 1 {
        return Err(ApiError::unauthorized());
    }
    auth.throttle_refund(&mut tx, &client, &identity.username)
        .await?;
    end_sign_ins(&mut tx, identity.id).await?;
    let token = random_token();
    let lifetime = auth.password_session_lifetime();
    auth.insert_session(&mut tx, identity.id, &token, lifetime, &headers, false)
        .await?;
    sqlx::query("DELETE FROM account_access_tokens WHERE user_id=$1")
        .bind(identity.id)
        .execute(&mut *tx)
        .await?;
    accounts::record_audit(
        &mut tx,
        Some(identity.id),
        "password_changed",
        "user",
        Some(&identity.id.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    auth.after_sign_out();
    Ok(auth.session_response(identity, &token, lifetime, false))
}

#[derive(Serialize, FromRow)]
struct AdminUser {
    id: Uuid,
    username: String,
    display_name: String,
    role: Role,
    enabled: bool,
    local_password: bool,
    password_change_required: bool,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct UserQuery {
    q: Option<String>,
    limit: Option<i64>,
}

async fn list_users(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    auth.require_admin(&headers, false).await?;
    let search = query.q.as_deref().unwrap_or("").trim();
    let limit = query.limit.unwrap_or(1000);
    if search.len() > 256 || !(1..=1000).contains(&limit) {
        return Err(ApiError::bad(
            "Search is limited to 256 bytes and limit must be between 1 and 1000",
        ));
    }
    // strpos treats wildcard characters literally; a search cannot silently match
    // every user through SQL LIKE patterns. UUIDs can also be searched exactly.
    let mut users = sqlx::query_as::<_, AdminUser>("SELECT id, username, display_name, role, enabled, password_hash IS NOT NULL AS local_password, password_change_required, created_at FROM users \
        WHERE $1 = '' OR strpos(lower(username), lower($1)) > 0 OR strpos(lower(display_name), lower($1)) > 0 OR id::text = $1 \
        ORDER BY created_at DESC, id DESC LIMIT $2")
        .bind(search).bind(limit + 1)
        .fetch_all(&auth.pool).await?;
    let has_more = users.len() > limit as usize;
    users.truncate(limit as usize);
    Ok(Json(json!({"users": users, "hasMore": has_more})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUserInput {
    username: String,
    display_name: Option<String>,
    /// Required while passwords are on; with them off, the account is created
    /// without one, to be linked to a single sign-on identity.
    password: Option<String>,
    role: Option<String>,
    #[serde(default)]
    force_password_change: bool,
}

async fn create_user(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Json(input): Json<CreateUserInput>,
) -> Result<(StatusCode, Json<Identity>), ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    let username = normalize_username(&input.username)?;
    let display_name = validate_display_name(input.display_name.as_deref().unwrap_or(&username))?;
    let role = Role::requested(input.role.as_deref())?.unwrap_or(Role::User);
    let hash = match (input.password, auth.config.password_sign_in) {
        (Some(password), true) => {
            validate_password(&password)?;
            Some(auth.new_password_hash(password).await?)
        }
        (None, true) => {
            return Err(ApiError::bad(
                "Password must contain at least 12 characters and at most 1024 bytes",
            ))
        }
        (Some(_), false) => auth.require_passwords().map(|_| None)?,
        (None, false) => None,
    };
    let force_password_change = input.force_password_change && hash.is_some();
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    let result = sqlx::query_as::<_, Identity>("INSERT INTO users (id, username, display_name, role, password_hash, password_change_required) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, username, display_name, role")
        .bind(Uuid::new_v4()).bind(username).bind(display_name).bind(role).bind(hash).bind(force_password_change).fetch_one(&mut *tx).await;
    match result {
        Ok(identity) => {
            auth.group_policy.initial(&mut tx, identity.id).await?;
            accounts::record_audit(
                &mut tx,
                Some(admin.id),
                "user_created",
                "user",
                Some(&identity.id.to_string()),
                json!({"role":identity.role,"forcePasswordChange":force_password_change}),
            )
            .await?;
            tx.commit().await?;
            Ok((StatusCode::CREATED, Json(identity)))
        }
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => Err(ApiError(
            StatusCode::CONFLICT,
            "Username already exists".into(),
        )),
        Err(error) => Err(error.into()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateUserInput {
    enabled: Option<bool>,
    role: Option<String>,
    display_name: Option<String>,
}

async fn update_user(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateUserInput>,
) -> Result<StatusCode, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    if input.enabled.is_none() && input.role.is_none() && input.display_name.is_none() {
        return Err(ApiError::bad("Provide an account field to update"));
    }
    let requested_role = Role::requested(input.role.as_deref())?;
    let display_name = input
        .display_name
        .as_deref()
        .map(validate_display_name)
        .transpose()?;
    if admin.id == id && input.enabled == Some(false) {
        return Err(ApiError::bad(
            "Administrators cannot disable their own account",
        ));
    }
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    let (enabled, role): (bool, Role) =
        sqlx::query_as("SELECT enabled,role FROM users WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(ApiError::not_found)?;
    let next_enabled = input.enabled.unwrap_or(enabled);
    let next_role = requested_role.unwrap_or(role);
    if enabled && role == Role::Admin && (!next_enabled || next_role != Role::Admin) {
        // A password counts only while passwords are on.
        let another_ready_admin: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u WHERE u.id<>$1 AND u.enabled AND u.role='admin' AND (($2 AND u.password_hash IS NOT NULL) OR EXISTS(SELECT 1 FROM oidc_identities o WHERE o.user_id=u.id)))")
            .bind(id).bind(auth.config.password_sign_in).fetch_one(&mut *tx).await?;
        if !another_ready_admin {
            return Err(ApiError::bad(
                "At least one enabled administrator is required",
            ));
        }
    }
    sqlx::query("UPDATE users SET enabled=$2,role=$3,display_name=COALESCE($4,display_name),updated_at=NOW() WHERE id=$1")
        .bind(id).bind(next_enabled).bind(next_role).bind(&display_name).execute(&mut *tx).await?;
    if enabled != next_enabled || role != next_role {
        end_sign_ins(&mut tx, id).await?;
        sqlx::query("DELETE FROM account_access_tokens WHERE user_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    accounts::record_audit(&mut tx, Some(admin.id), "user_updated", "user", Some(&id.to_string()), json!({"enabled":next_enabled,"role":next_role,"displayNameChanged":display_name.is_some()})).await?;
    tx.commit().await?;
    auth.after_sign_out();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetPasswordInput {
    new_password: String,
    #[serde(default)]
    force_password_change: bool,
}

async fn reset_password(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ResetPasswordInput>,
) -> Result<StatusCode, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    auth.require_passwords()?;
    if admin.id == id {
        return Err(ApiError::bad("Use change password for your own account"));
    }
    validate_password(&input.new_password)?;
    let hash = auth.new_password_hash(input.new_password).await?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    // Administrators cannot silently add local credentials to an OIDC-only identity.
    if sqlx::query("UPDATE users SET password_hash = $1, password_change_required=$3, updated_at = NOW() WHERE id = $2 AND password_hash IS NOT NULL")
        .bind(hash).bind(id).bind(input.force_password_change).execute(&mut *tx).await?.rows_affected() == 0 { return Err(ApiError::not_found()); }
    end_sign_ins(&mut tx, id).await?;
    sqlx::query("DELETE FROM account_access_tokens WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    accounts::record_audit(
        &mut tx,
        Some(admin.id),
        "password_reset",
        "user",
        Some(&id.to_string()),
        json!({"forcePasswordChange":input.force_password_change}),
    )
    .await?;
    tx.commit().await?;
    auth.after_sign_out();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkOidcInput {
    user_id: Uuid,
    subject: String,
}

async fn link_oidc_identity(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Json(input): Json<LinkOidcInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    let provider = auth
        .config
        .oidc
        .as_ref()
        .ok_or_else(|| ApiError::bad("OIDC is not configured"))?;
    if input.subject.is_empty()
        || input.subject.len() > 255
        || !input.subject.is_ascii()
        || input.subject.chars().any(char::is_control)
    {
        return Err(ApiError::bad(
            "OIDC subject must be 1–255 printable ASCII characters",
        ));
    }
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    let user: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id = $1 AND enabled FOR UPDATE")
            .bind(input.user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if user.is_none() {
        return Err(ApiError::not_found());
    }
    // When gateway calls carry provider tokens, an account with two identities at
    // the provider would send one person's calls with the other's token.
    if auth.grants.is_some() {
        let linked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM oidc_identities WHERE user_id = $1 AND issuer = $2)",
        )
        .bind(input.user_id)
        .bind(&provider.issuer)
        .fetch_one(&mut *tx)
        .await?;
        if linked {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "This account is already linked to a single sign-on identity".into(),
            ));
        }
    }
    let inserted = sqlx::query("INSERT INTO oidc_identities (issuer, subject, user_id) VALUES ($1, $2, $3) ON CONFLICT (issuer, subject) DO NOTHING")
        .bind(&provider.issuer).bind(&input.subject).bind(input.user_id).execute(&mut *tx).await?.rows_affected();
    if inserted == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This OIDC identity is already linked".into(),
        ));
    }
    // Pending invitation or recovery links must not add a local password afterwards.
    sqlx::query("DELETE FROM account_access_tokens WHERE user_id=$1")
        .bind(input.user_id)
        .execute(&mut *tx)
        .await?;
    accounts::record_audit(
        &mut tx,
        Some(admin.id),
        "oidc_linked",
        "user",
        Some(&input.user_id.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"userId": input.user_id, "issuer": provider.issuer, "subject": input.subject})),
    ))
}
