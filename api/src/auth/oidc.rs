use super::*;
use axum::{extract::Query, response::Redirect};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
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

pub(super) struct Oidc {
    config: OidcConfig,
    origin: String,
    http: reqwest::Client,
    cache: Mutex<(std::time::Instant, DiscoveredClient)>,
}

impl Oidc {
    pub(super) async fn new(config: &OidcConfig, origin: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()?;
        let client = discover(config, origin, &http).await?;
        Ok(Self {
            config: config.clone(),
            origin: origin.into(),
            http,
            cache: Mutex::new((std::time::Instant::now(), client)),
        })
    }

    async fn client(&self) -> Result<DiscoveredClient, ApiError> {
        let mut cache = self.cache.lock().await;
        // Refresh discovery/JWKS regularly so provider key rotation does not require
        // a deployment. A failed refresh fails closed without exposing its response.
        if cache.0.elapsed() >= Duration::from_secs(300) {
            let refreshed = discover(&self.config, &self.origin, &self.http)
                .await
                .map_err(|_| {
                    tracing::warn!("OIDC discovery refresh failed");
                    ApiError(
                        StatusCode::BAD_GATEWAY,
                        "The identity provider is unavailable".into(),
                    )
                })?;
            *cache = (std::time::Instant::now(), refreshed);
        }
        Ok(cache.1.clone())
    }
}

async fn discover(
    config: &OidcConfig,
    origin: &str,
    http: &reqwest::Client,
) -> anyhow::Result<DiscoveredClient> {
    let metadata =
        CoreProviderMetadata::discover_async(IssuerUrl::new(config.issuer.clone())?, http)
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
    Ok(CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(config.client_id.clone()),
        Some(ClientSecret::new(config.client_secret.clone())),
    )
    .set_redirect_uri(RedirectUrl::new(format!(
        "{origin}/api/auth/oidc/callback"
    ))?))
}

pub(super) async fn login(State(auth): State<Auth>, caller: Client) -> Result<Response, ApiError> {
    let provider = auth.oidc.as_ref().ok_or_else(ApiError::not_found)?;
    // A separate bucket keeps redirects from consuming the password-login budget.
    auth.throttle("oidc", &caller, None).await?;
    let browser = random_token();
    let client = provider.client().await?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("profile".into()))
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
    let client = provider.client().await?;
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
    let id_token = tokens.id_token().ok_or_else(ApiError::unauthorized)?;
    let verifier = client.id_token_verifier();
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
    let lifetime = (claims.expiration() - Utc::now())
        .num_seconds()
        .min(auth.config.session_hours as i64 * 3600);
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
        Some(user) if user.enabled => user.identity(),
        Some(_) => return Err(ApiError::forbidden()),
        None if !provider.config.allow_signup => {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "This OIDC identity has not been provisioned; contact your administrator".into(),
            ))
        }
        None => {
            let id = Uuid::new_v4();
            // Names and emails from the IdP cannot claim an existing local account.
            let identity = sqlx::query_as::<_, Identity>("INSERT INTO users (id, username, display_name, role) VALUES ($1, $2, $3, $4) RETURNING id, username, display_name, role")
                .bind(id).bind(format!("oidc_{}", id.simple())).bind(display_name).bind(Role::User).fetch_one(&mut *tx).await?;
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
    auth.insert_session(&mut tx, identity.id, &token, lifetime, &headers)
        .await?;
    tx.commit().await?;
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
