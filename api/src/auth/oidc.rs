use super::*;
use axum::{extract::Query, response::Redirect};
use openidconnect::{
    core::{
        CoreAuthDisplay, CoreAuthenticationFlow, CoreClaimName, CoreClaimType, CoreClient,
        CoreClientAuthMethod, CoreErrorResponseType, CoreGrantType, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreJwsSigningAlgorithm,
        CoreResponseMode, CoreResponseType, CoreSubjectIdentifierType,
    },
    AccessToken, AccessTokenHash, AdditionalProviderMetadata, AsyncHttpClient, AuthorizationCode,
    ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet, EndpointSet,
    HttpClientError, HttpRequest, HttpResponse, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, ProviderMetadata, RedirectUrl, RefreshToken,
    RequestTokenError, RevocationUrl, Scope, TokenResponse,
};
use tokio::sync::Mutex;

type DiscoveredClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// Discovery fields beyond the core set: the RFC 7009 revocation endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct RevocationMetadata {
    revocation_endpoint: Option<String>,
}
impl AdditionalProviderMetadata for RevocationMetadata {}

type Metadata = ProviderMetadata<
    RevocationMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

/// What discovery found: the client, the ID token algorithms Chat accepts from this
/// provider, and where it revokes tokens, if it says.
#[derive(Clone)]
struct Discovered {
    client: DiscoveredClient,
    algorithms: Vec<CoreJwsSigningAlgorithm>,
    revocation: Option<RevocationUrl>,
}

/// Discovery is repeated this often, so provider key rotation needs no deployment.
const DISCOVERY_AGE: Duration = Duration::from_secs(300);
/// The longest one discovery may take: the metadata and the JWKS.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
/// After a failed discovery, callers fail at once for this long instead of each
/// asking the provider again.
const DISCOVERY_RETRY: Duration = Duration::from_secs(15);

struct Cache {
    at: std::time::Instant,
    discovered: Discovered,
    failed_at: Option<std::time::Instant>,
}

pub(super) struct Oidc {
    config: OidcConfig,
    origin: String,
    http: reqwest::Client,
    cache: std::sync::Mutex<Cache>,
    /// One discovery at a time; the lock on the cache is never held across it.
    discovering: Mutex<()>,
}

/// Why a refresh did not produce a token.
pub(super) enum RefreshFailure {
    /// The provider refused the grant (invalid_grant): the sign-in is over.
    Rejected,
    /// The provider did not take the token: the request never left Chat, or the
    /// provider answered with an error. The grant may still be good.
    Unavailable,
    /// The request went out and no answer Chat could read came back, so the
    /// provider may or may not have rotated the token.
    Unanswered,
}

/// How far a token request got.
#[derive(Clone, Copy)]
enum Reached {
    NotSent,
    Sent,
    Answered(StatusCode),
}

/// Chat's HTTP client for one token request, noting how far the request got.
struct Tracked<F> {
    http: reqwest::Client,
    /// Asked right before the request goes out.
    may_send: F,
    reached: std::sync::Mutex<Reached>,
}

impl<'c, F: Fn() -> bool + Send + Sync + 'c> AsyncHttpClient<'c> for Tracked<F> {
    type Error = HttpClientError<reqwest::Error>;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, Self::Error>> + Send + 'c>,
    >;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            if !(self.may_send)() {
                return Err(HttpClientError::Other("not sent".into()));
            }
            *self.reached.lock().unwrap() = Reached::Sent;
            let result = self.http.call(request).await;
            match &result {
                Ok(response) => {
                    *self.reached.lock().unwrap() = Reached::Answered(response.status())
                }
                // No connection, so the request never left Chat.
                Err(HttpClientError::Reqwest(error))
                    if error.is_connect() || error.is_builder() =>
                {
                    *self.reached.lock().unwrap() = Reached::NotSent
                }
                Err(_) => {}
            }
            result
        })
    }
}

/// A refreshed access token, and the refresh token to use next when the provider
/// rotated it.
pub(super) struct Refreshed {
    pub access: AccessToken,
    pub refresh: Option<RefreshToken>,
    pub expires_in: Option<Duration>,
}

impl Oidc {
    pub(super) async fn new(config: &OidcConfig, origin: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()?;
        let discovered = discover(config, origin, &http).await?;
        Ok(Self {
            config: config.clone(),
            origin: origin.into(),
            http,
            cache: std::sync::Mutex::new(Cache {
                at: std::time::Instant::now(),
                discovered,
                failed_at: None,
            }),
            discovering: Mutex::new(()),
        })
    }

    /// The cached discovery while it is current; an error while a recent attempt to
    /// renew it failed; None when it is due.
    fn cached(&self) -> Result<Option<Discovered>, ApiError> {
        let cache = self.cache.lock().unwrap();
        if cache.at.elapsed() < DISCOVERY_AGE {
            return Ok(Some(cache.discovered.clone()));
        }
        if cache
            .failed_at
            .is_some_and(|at| at.elapsed() < DISCOVERY_RETRY)
        {
            return Err(provider_unavailable());
        }
        Ok(None)
    }

    async fn discovered(&self) -> Result<Discovered, ApiError> {
        if let Some(discovered) = self.cached()? {
            return Ok(discovered);
        }
        // Refresh discovery/JWKS regularly so provider key rotation does not require
        // a deployment. A failed refresh fails closed without exposing its response,
        // and those waiting on it fail with it rather than each trying again.
        let _turn = self.discovering.lock().await;
        if let Some(discovered) = self.cached()? {
            return Ok(discovered);
        }
        let refreshed = tokio::time::timeout(
            DISCOVERY_TIMEOUT,
            discover(&self.config, &self.origin, &self.http),
        )
        .await;
        let mut cache = self.cache.lock().unwrap();
        match refreshed {
            Ok(Ok(discovered)) => {
                *cache = Cache {
                    at: std::time::Instant::now(),
                    discovered: discovered.clone(),
                    failed_at: None,
                };
                Ok(discovered)
            }
            _ => {
                tracing::warn!("OIDC discovery refresh failed");
                cache.failed_at = Some(std::time::Instant::now());
                Err(provider_unavailable())
            }
        }
    }

    /// Trades a refresh token for a new access token. `may_send` is asked right
    /// before the request goes out, after discovery; when it says no, nothing is
    /// sent and the refresh is Unavailable.
    pub(super) async fn refresh(
        &self,
        token: &str,
        may_send: impl Fn() -> bool + Send + Sync,
    ) -> Result<Refreshed, RefreshFailure> {
        let discovered = self
            .discovered()
            .await
            .map_err(|_| RefreshFailure::Unavailable)?;
        let http = Tracked {
            http: self.http.clone(),
            may_send,
            reached: std::sync::Mutex::new(Reached::NotSent),
        };
        let response = discovered
            .client
            .exchange_refresh_token(&RefreshToken::new(token.into()))
            .map_err(|_| RefreshFailure::Unavailable)?
            .request_async(&http)
            .await;
        let reached = *http.reached.lock().unwrap();
        match response {
            Ok(tokens) => Ok(Refreshed {
                access: tokens.access_token().clone(),
                refresh: tokens.refresh_token().cloned(),
                expires_in: tokens.expires_in(),
            }),
            // Only invalid_grant is about this grant. invalid_client and the like
            // say Chat's own client is misconfigured at the provider, which ending
            // people's sign-ins would not fix.
            Err(RequestTokenError::ServerResponse(error))
                if *error.error() == CoreErrorResponseType::InvalidGrant =>
            {
                Err(RefreshFailure::Rejected)
            }
            Err(RequestTokenError::ServerResponse(error)) => {
                if matches!(
                    error.error(),
                    CoreErrorResponseType::InvalidClient
                        | CoreErrorResponseType::UnauthorizedClient
                ) {
                    tracing::error!(
                        error = error.error().as_ref(),
                        "the identity provider refused Chat's client credentials; check OBLETH_CHAT_OIDC_CLIENT_ID and OBLETH_CHAT_OIDC_CLIENT_SECRET"
                    );
                }
                Err(RefreshFailure::Unavailable)
            }
            // An error status the provider gave without a readable error, or a
            // request that never left: the token was not taken.
            Err(_) => match reached {
                Reached::NotSent => Err(RefreshFailure::Unavailable),
                Reached::Answered(status) if !status.is_success() => {
                    Err(RefreshFailure::Unavailable)
                }
                // No answer, or a success Chat could not read: the provider may
                // have rotated the token.
                Reached::Sent | Reached::Answered(_) => Err(RefreshFailure::Unanswered),
            },
        }
    }

    /// Revokes a refresh token at the provider's revocation endpoint (RFC 7009).
    /// Returns false when the provider could not take it now; a provider without a
    /// revocation endpoint has nothing to do.
    pub(super) async fn revoke(&self, token: &str) -> bool {
        let Ok(discovered) = self.discovered().await else {
            return false;
        };
        let Some(url) = discovered.revocation else {
            return true;
        };
        // client_secret_basic, with the id and secret form-encoded first (RFC 6749
        // 2.3.1), as the token endpoint gets them.
        let encode = |value: &str| {
            url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
        };
        let response = self
            .http
            .post(url.url().clone())
            .basic_auth(
                encode(&self.config.client_id),
                Some(encode(&self.config.client_secret)),
            )
            .form(&[("token", token), ("token_type_hint", "refresh_token")])
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        // An unknown or already revoked token is a success too (RFC 7009 2.2).
        matches!(response, Ok(response) if response.status().is_success())
    }
}

fn provider_unavailable() -> ApiError {
    ApiError(
        StatusCode::BAD_GATEWAY,
        "The identity provider is unavailable".into(),
    )
}

/// ID token algorithms Chat accepts: the asymmetric ones, never a MAC keyed with the
/// client secret and never `none`.
fn asymmetric(algorithm: &CoreJwsSigningAlgorithm) -> bool {
    use CoreJwsSigningAlgorithm as A;
    matches!(
        algorithm,
        A::RsaSsaPkcs1V15Sha256
            | A::RsaSsaPkcs1V15Sha384
            | A::RsaSsaPkcs1V15Sha512
            | A::RsaSsaPssSha256
            | A::RsaSsaPssSha384
            | A::RsaSsaPssSha512
            | A::EcdsaP256Sha256
            | A::EcdsaP384Sha384
            | A::EcdsaP521Sha512
            | A::EdDsa
    )
}

async fn discover(
    config: &OidcConfig,
    origin: &str,
    http: &reqwest::Client,
) -> anyhow::Result<Discovered> {
    // Keys come only from the discovered jwks_uri; header jku or x5u are never used.
    // The JWKS may live on another host than the issuer.
    let metadata = Metadata::discover_async(IssuerUrl::new(config.issuer.clone())?, http)
        .await
        .map_err(|_| anyhow::anyhow!("OIDC discovery failed for the configured issuer"))?;
    validate_transport(metadata.authorization_endpoint().url())?;
    validate_transport(metadata.jwks_uri().url())?;
    validate_transport(
        metadata
            .token_endpoint()
            .context("OIDC issuer does not provide a token endpoint")?
            .url(),
    )?;
    let algorithms: Vec<_> = metadata
        .id_token_signing_alg_values_supported()
        .iter()
        .filter(|algorithm| asymmetric(algorithm))
        .cloned()
        .collect();
    if algorithms.is_empty() {
        bail!("The OIDC issuer offers no asymmetric ID token signing algorithm");
    }
    let revocation = match &metadata.additional_metadata().revocation_endpoint {
        Some(endpoint) => {
            let url = RevocationUrl::new(endpoint.clone())
                .context("The OIDC revocation endpoint must be a URL")?;
            validate_transport(url.url())?;
            Some(url)
        }
        None => None,
    };
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(config.client_id.clone()),
        Some(ClientSecret::new(config.client_secret.clone())),
    )
    .set_redirect_uri(RedirectUrl::new(format!(
        "{origin}/api/auth/oidc/callback"
    ))?);
    Ok(Discovered {
        client,
        algorithms,
        revocation,
    })
}

pub(super) async fn login(State(auth): State<Auth>, caller: Client) -> Result<Response, ApiError> {
    let provider = auth.oidc.as_ref().ok_or_else(ApiError::not_found)?;
    // A separate bucket keeps redirects from consuming the password-login budget.
    auth.throttle("oidc", &caller, None).await?;
    let browser = random_token();
    let client = provider.discovered().await?.client;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    // The library adds openid itself.
    let (url, state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scopes(
            provider
                .config
                .scopes
                .iter()
                .filter(|scope| *scope != "openid")
                .map(|scope| Scope::new(scope.clone())),
        )
        .set_pkce_challenge(challenge)
        .url();
    sqlx::query("DELETE FROM oidc_transactions WHERE expires_at <= NOW()")
        .execute(&auth.pool)
        .await?;
    sqlx::query("INSERT INTO oidc_transactions (state_hash, browser_hash, nonce, pkce_verifier, expires_at) VALUES ($1, $2, $3, $4, NOW() + INTERVAL '5 minutes')")
        .bind(digest(state.secret())).bind(digest(&browser)).bind(nonce.secret()).bind(verifier.secret()).execute(&auth.pool).await?;
    let mut response = Redirect::to(url.as_str()).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        auth.cookie(auth.oidc_cookie_name(), &browser, 300),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    Ok(response)
}

#[derive(Deserialize)]
pub(super) struct CallbackInput {
    code: Option<String>,
    state: String,
    error: Option<String>,
}

#[derive(FromRow)]
struct OidcTransaction {
    nonce: String,
    pkce_verifier: String,
}

/// The username Chat makes up when the provider's is missing, invalid or taken.
fn generated_username(id: Uuid) -> String {
    format!("oidc_{}", id.simple())
}

/// Renames the account to `name` if no other account has it, recording whether the
/// name came from the provider. A savepoint keeps a name taken at the same moment
/// from failing the whole sign-in.
pub(super) async fn take_username(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    name: &str,
    from_provider: bool,
) -> Result<Option<String>, ApiError> {
    let mut savepoint = sqlx::Acquire::begin(&mut **tx).await?;
    let renamed = sqlx::query_scalar::<_, String>(
        "UPDATE users SET username = $2, username_from_provider = $3, updated_at = NOW() \
         WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM users WHERE username = $2 AND id <> $1) \
         RETURNING username",
    )
    .bind(id)
    .bind(name)
    .bind(from_provider)
    .fetch_optional(&mut *savepoint)
    .await;
    match renamed {
        Ok(renamed) => {
            savepoint.commit().await?;
            Ok(renamed)
        }
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            savepoint.rollback().await?;
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// Keeps a username that came from the provider equal to the provider's current one,
/// and gives an account still on its generated name the provider's once it is free.
/// When another account holds the provider's name, or the provider no longer sends a
/// usable one, a provider-derived name goes back to a generated one rather than keep
/// showing a name the provider no longer gives this person. Names an administrator or
/// bootstrap chose are left alone.
async fn follow_provider_username(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    identity: &mut Identity,
    issuer: &str,
    name: Option<&str>,
) -> Result<(), ApiError> {
    let (from_provider, identities): (bool, i64) = sqlx::query_as(
        "SELECT u.username_from_provider, \
         (SELECT COUNT(*) FROM oidc_identities o WHERE o.user_id = u.id AND o.issuer = $2) \
         FROM users u WHERE u.id = $1",
    )
    .bind(identity.id)
    .bind(issuer)
    .fetch_one(&mut **tx)
    .await?;
    // An account linked to two identities at the provider would swap between their
    // names on every sign-in.
    if identities != 1 {
        return Ok(());
    }
    let generated = generated_username(identity.id);
    let renamed = match name {
        Some(name) if identity.username == name => None,
        Some(name) if from_provider || identity.username == generated => {
            match take_username(tx, identity.id, name, true).await? {
                None if from_provider => take_username(tx, identity.id, &generated, false).await?,
                renamed => renamed,
            }
        }
        None if from_provider => take_username(tx, identity.id, &generated, false).await?,
        _ => None,
    };
    if let Some(renamed) = renamed {
        identity.username = renamed;
    }
    Ok(())
}

pub(super) async fn callback(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Query(input): Query<CallbackInput>,
) -> Result<Response, ApiError> {
    let provider = auth.oidc.as_ref().ok_or_else(ApiError::not_found)?;
    if input.state.len() > 1024 {
        return Err(ApiError::forbidden());
    }
    let browser =
        cookie_value(&headers, auth.oidc_cookie_name()).ok_or_else(ApiError::forbidden)?;
    // Consuming the transaction atomically gives exactly one callback a chance to
    // exchange the code, across all replicas. A different browser cannot consume it.
    let transaction = sqlx::query_as::<_, OidcTransaction>(
        "DELETE FROM oidc_transactions WHERE state_hash = $1 AND browser_hash = $2 AND expires_at > NOW() RETURNING nonce, pkce_verifier"
    ).bind(digest(&input.state)).bind(digest(&browser)).fetch_optional(&auth.pool).await?.ok_or_else(ApiError::forbidden)?;
    if input.error.is_some() {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Identity provider sign-in was not completed".into(),
        ));
    }
    let code = input
        .code
        .filter(|s| !s.is_empty() && s.len() <= 8192)
        .ok_or_else(ApiError::forbidden)?;
    let discovered = provider.discovered().await?;
    let client = discovered.client;
    let tokens = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|_| ApiError::internal())?
        .set_pkce_verifier(PkceCodeVerifier::new(transaction.pkce_verifier))
        .request_async(&provider.http)
        .await
        .map_err(|_| {
            tracing::warn!("OIDC authorization code exchange failed");
            ApiError(
                StatusCode::UNAUTHORIZED,
                "Identity provider sign-in failed".into(),
            )
        })?;
    // Everything after the exchange runs here, so a failure can revoke the grant.
    let outcome: Result<Response, ApiError> = async {
        let id_token = tokens.id_token().ok_or_else(ApiError::unauthorized)?;
        // Only the asymmetric algorithms the provider advertises; the library would
        // otherwise also accept an HMAC keyed with the client secret.
        let verifier = client
            .id_token_verifier()
            .set_allowed_algs(discovered.algorithms);
        let nonce = Nonce::new(transaction.nonce);
        // openidconnect validates signature, issuer, audience, expiry, and nonce.
        let claims = id_token.claims(&verifier, &nonce).map_err(|_| {
            tracing::warn!("OIDC ID token validation failed");
            ApiError::unauthorized()
        })?;
        // The library deliberately leaves the azp check to the application.
        if claims
            .authorized_party()
            .is_some_and(|party| party.as_str() != provider.config.client_id)
            || (claims.audiences().len() > 1 && claims.authorized_party().is_none())
            || claims.issue_time() > Utc::now() + chrono::Duration::seconds(60)
        {
            return Err(ApiError::unauthorized());
        }
        if let Some(expected) = claims.access_token_hash() {
            let actual = AccessTokenHash::from_token(
                tokens.access_token(),
                id_token
                    .signing_alg()
                    .map_err(|_| ApiError::unauthorized())?,
                id_token
                    .signing_key(&verifier)
                    .map_err(|_| ApiError::unauthorized())?,
            )
            .map_err(|_| ApiError::unauthorized())?;
            if actual != *expected {
                return Err(ApiError::unauthorized());
            }
        }
        let mapped_groups = auth
            .group_policy
            .groups_from_verified_token(&id_token.to_string())?;
        let subject = claims.subject().as_str();
        if subject.is_empty() || subject.len() > 255 {
            return Err(ApiError::unauthorized());
        }
        // Forwarding the provider's tokens needs a refresh token to keep them current.
        let refresh = match (&auth.grants, tokens.refresh_token()) {
            (Some(_), None) => {
                tracing::warn!("the identity provider issued no refresh token; check offline_access");
                return Err(ApiError(
                    StatusCode::UNAUTHORIZED,
                    "The identity provider did not allow Chat to stay signed in".into(),
                ));
            }
            (_, refresh) => refresh,
        };
        // A session is capped at the ID token's expiry, except while the provider's
        // grant is kept: then the provider ends the sign-in when it ends the grant.
        let lifetime = match auth.grants {
            Some(_) => auth.config.session_hours as i64 * 3600,
            None => (claims.expiration() - Utc::now())
                .num_seconds()
                .min(auth.config.session_hours as i64 * 3600),
        };
        if lifetime <= 0 {
            return Err(ApiError::unauthorized());
        }
        let name = claims
            .name()
            .and_then(|n| n.get(None))
            .map(|name| name.as_str())
            .or_else(|| claims.preferred_username().map(|name| name.as_str()))
            .unwrap_or("OIDC user");
        let display_name = validate_display_name(name).unwrap_or_else(|_| "OIDC user".into());
        // With provider usernames on, the provider's username names the account when
        // it is valid and free. The oidc_ prefix stays reserved for the names Chat
        // makes up.
        let provider_username = claims
            .preferred_username()
            .filter(|_| provider.config.provider_usernames)
            .and_then(|name| normalize_username(name.as_str()).ok())
            .filter(|name| !name.starts_with("oidc_"));
        let follow_provider = provider.config.provider_usernames;
        let mut tx = auth.pool.begin().await?;
        // Serialize provisioning for this issuer/subject, including the first sign-in.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("oidc:{}:{subject}", provider.config.issuer))
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query_as::<_, PasswordUser>(
            "SELECT u.id, u.username, u.display_name, u.role, u.password_hash, u.enabled, u.password_change_required FROM oidc_identities o \
             JOIN users u ON u.id = o.user_id WHERE o.issuer = $1 AND o.subject = $2 FOR UPDATE OF u"
        ).bind(&provider.config.issuer).bind(subject).fetch_optional(&mut *tx).await?;
        let identity = match existing {
            Some(user) if user.enabled => {
                let mut identity = user.identity();
                if follow_provider {
                    follow_provider_username(
                        &mut tx,
                        &mut identity,
                        &provider.config.issuer,
                        provider_username.as_deref(),
                    )
                    .await?;
                }
                identity
            }
            Some(_) => return Err(ApiError::forbidden()),
            None if !provider.config.allow_signup => {
                return Err(ApiError(
                    StatusCode::FORBIDDEN,
                    "This OIDC identity has not been provisioned; contact your administrator".into(),
                ))
            }
            None => {
                let id = Uuid::new_v4();
                // Names and emails from the IdP cannot claim an existing local account:
                // a taken username falls back to a generated one.
                let mut identity = None;
                if let Some(name) = &provider_username {
                    identity = sqlx::query_as::<_, Identity>("INSERT INTO users (id, username, display_name, role, username_from_provider) VALUES ($1, $2, $3, $4, TRUE) ON CONFLICT (username) DO NOTHING RETURNING id, username, display_name, role")
                        .bind(id).bind(name).bind(&display_name).bind(Role::User).fetch_optional(&mut *tx).await?;
                }
                let identity = match identity {
                    Some(identity) => identity,
                    None => sqlx::query_as::<_, Identity>("INSERT INTO users (id, username, display_name, role) VALUES ($1, $2, $3, $4) RETURNING id, username, display_name, role")
                        .bind(id).bind(generated_username(id)).bind(&display_name).bind(Role::User).fetch_one(&mut *tx).await?,
                };
                sqlx::query(
                    "INSERT INTO oidc_identities (issuer, subject, user_id) VALUES ($1, $2, $3)",
                )
                .bind(&provider.config.issuer)
                .bind(subject)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                auth.group_policy.initial(&mut tx, identity.id).await?;
                identity
            }
        };
        auth.group_policy
            .reconcile(&mut tx, identity.id, &mapped_groups)
            .await?;
        if let Some(old) = auth.session_token(&headers) {
            sqlx::query("DELETE FROM auth_sessions WHERE token_hash = $1")
                .bind(digest(&old))
                .execute(&mut *tx)
                .await?;
        }
        let token = random_token();
        let session = auth
            .insert_session(&mut tx, identity.id, &token, lifetime, &headers, true)
            .await?;
        if let (Some(grants), Some(refresh)) = (&auth.grants, refresh) {
            grants
                .store(
                    &mut tx,
                    identity.id,
                    session,
                    subject,
                    tokens.access_token(),
                    refresh,
                    tokens.expires_in(),
                )
                .await?;
        }
        tx.commit().await?;
        auth.after_sign_out();
        let mut response = Redirect::to("/").into_response();
        response.headers_mut().append(
            header::SET_COOKIE,
            auth.cookie(auth.session_cookie_name(), &token, lifetime),
        );
        response.headers_mut().append(
            header::SET_COOKIE,
            auth.cookie(auth.oidc_cookie_name(), "", 0),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
            .headers_mut()
            .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
        Ok(response)
    }
    .await;
    // The exchange made a grant at the provider (some providers also register a
    // device and tell the person). When this sign-in then failed, revoke its
    // refresh token so no live grant is left behind.
    // Only when Chat would have kept the grant: in sign mode it never keeps one.
    if outcome.is_err() && auth.grants.is_some() {
        if let Some(refresh) = tokens.refresh_token() {
            let (provider, token) = (provider.clone(), refresh.secret().clone());
            tokio::spawn(async move {
                if !provider.revoke(&token).await {
                    tracing::warn!("could not revoke the grant of a failed sign-in");
                }
            });
        }
    }
    outcome
}
