//! The identity provider's tokens for each sign-in, kept when Chat forwards people's
//! own access tokens to the gateway (OBLETH_CHAT_GATEWAY_AUTH=forward). A grant
//! belongs to one browser session and ends with it. Its tokens are sealed with the
//! installation's secret key, and the access token leaves Chat only in the gateway's
//! Authorization header.
use super::oidc::{Oidc, RefreshFailure};
use super::*;
use openidconnect::{AccessToken, RefreshToken};

/// Format byte of sealed grant tokens.
const SEALED: u8 = 1;
/// An access token with less than this left is refreshed before use.
const REFRESH_MARGIN_SECS: f64 = 60.0;
/// Assumed lifetime of an access token the provider gave no expiry for.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(300);
/// Revocations tried this many times are dropped; the token expires on its own.
const REVOKE_ATTEMPTS: i32 = 5;

#[derive(Clone)]
pub struct Grants {
    pool: PgPool,
    key: SecretKey,
    oidc: Arc<Oidc>,
    issuer: String,
}

/// A person's provider access token. It has no Debug, Display or Serialize, and
/// is only ever turned into the gateway's Authorization header.
pub struct ProviderToken(String);

impl ProviderToken {
    pub(crate) fn authorization(&self) -> Result<HeaderValue, ApiError> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", self.0))
            .map_err(|_| ApiError::internal())?;
        value.set_sensitive(true);
        Ok(value)
    }
}

/// No live grant is left for someone who signs in with the provider.
pub fn sign_in_again() -> ApiError {
    ApiError(StatusCode::UNAUTHORIZED, SIGN_IN_AGAIN.into())
}
pub const SIGN_IN_AGAIN: &str = "Sign in again to continue";

#[derive(FromRow)]
struct GrantRow {
    id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    access_token: Vec<u8>,
    refresh_token: Vec<u8>,
    fresh: bool,
}

#[derive(FromRow)]
struct Revocation {
    grant_id: Uuid,
    user_id: Uuid,
    issuer: String,
    refresh_token: Vec<u8>,
    attempts: i32,
}

fn aad(field: &str, grant: Uuid, user: Uuid) -> Vec<u8> {
    [field.as_bytes(), grant.as_bytes(), user.as_bytes()].concat()
}

/// Seconds an access token lives, as the provider said or the default.
fn lifetime(expires_in: Option<Duration>) -> f64 {
    expires_in.unwrap_or(DEFAULT_LIFETIME).as_secs_f64()
}

impl Grants {
    pub(super) fn new(pool: PgPool, key: SecretKey, oidc: Arc<Oidc>, issuer: String) -> Self {
        Self {
            pool,
            key,
            oidc,
            issuer,
        }
    }

    fn seal(&self, field: &str, grant: Uuid, user: Uuid, token: &str) -> Vec<u8> {
        self.key
            .seal(SEALED, &aad(field, grant, user), token.as_bytes())
    }

    fn open(&self, field: &str, grant: Uuid, user: Uuid, sealed: &[u8]) -> Option<String> {
        self.key
            .open(SEALED, &aad(field, grant, user), sealed)
            .and_then(|plain| String::from_utf8(plain).ok())
    }

    /// Keeps a new sign-in's tokens, in the transaction that creates its session.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn store(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user: Uuid,
        session: Uuid,
        subject: &str,
        access: &AccessToken,
        refresh: &RefreshToken,
        expires_in: Option<Duration>,
    ) -> Result<(), ApiError> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO oidc_grants (id, user_id, session_id, issuer, subject, access_token, refresh_token, access_expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, NOW() + make_interval(secs => $8))",
        )
        .bind(id)
        .bind(user)
        .bind(session)
        .bind(&self.issuer)
        .bind(subject)
        .bind(self.seal("access", id, user, access.secret()))
        .bind(self.seal("refresh", id, user, refresh.secret()))
        .bind(lifetime(expires_in))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// The access token a gateway call for `user` carries. None for someone without
    /// an identity at this provider, such as a local password account. Someone with
    /// one only ever gets their own token: their most recently refreshed live grant,
    /// refreshed first when it is about to expire. A grant the provider refuses ends
    /// its sign-in and the next one is tried; with none left, they must sign in again.
    pub async fn access_token(&self, user: Uuid) -> Result<Option<ProviderToken>, ApiError> {
        let linked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM oidc_identities WHERE user_id = $1 AND issuer = $2)",
        )
        .bind(user)
        .bind(&self.issuer)
        .fetch_one(&self.pool)
        .await?;
        if !linked {
            return Ok(None);
        }
        loop {
            let grant = sqlx::query_as::<_, GrantRow>(
                "SELECT g.id, g.user_id, g.session_id, g.access_token, g.refresh_token, \
                 g.access_expires_at > NOW() + make_interval(secs => $3) AS fresh \
                 FROM oidc_grants g JOIN auth_sessions s ON s.id = g.session_id \
                 WHERE g.user_id = $1 AND g.issuer = $2 AND s.expires_at > NOW() \
                 ORDER BY g.refreshed_at DESC, g.id LIMIT 1",
            )
            .bind(user)
            .bind(&self.issuer)
            .bind(REFRESH_MARGIN_SECS)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(sign_in_again)?;
            if grant.fresh {
                if let Some(token) = self.open("access", grant.id, user, &grant.access_token) {
                    return Ok(Some(ProviderToken(token)));
                }
            }
            if let Some(token) = self.refresh(grant.id).await? {
                return Ok(Some(token));
            }
        }
    }

    /// Refreshes one grant while holding its row lock, so replicas never present
    /// the same refresh token twice: the provider would take that as theft. Returns
    /// None when the grant has ended, so the caller tries the next one.
    async fn refresh(&self, id: Uuid) -> Result<Option<ProviderToken>, ApiError> {
        let mut tx = self.pool.begin().await?;
        let Some(grant) = sqlx::query_as::<_, GrantRow>(
            "SELECT id, user_id, session_id, access_token, refresh_token, \
             access_expires_at > NOW() + make_interval(secs => $2) AS fresh \
             FROM oidc_grants WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .bind(REFRESH_MARGIN_SECS)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(None);
        };
        let user = grant.user_id;
        // Another replica may have refreshed it while this one waited for the lock.
        if grant.fresh {
            if let Some(token) = self.open("access", grant.id, user, &grant.access_token) {
                tx.commit().await?;
                return Ok(Some(ProviderToken(token)));
            }
        }
        let Some(refresh) = self.open("refresh", grant.id, user, &grant.refresh_token) else {
            tracing::warn!("a stored sign-in could not be decrypted; ending it");
            self.end(&mut tx, &grant, "unreadable").await?;
            tx.commit().await?;
            return Ok(None);
        };
        match self.oidc.refresh(&refresh).await {
            Ok(refreshed) => {
                let next = refreshed
                    .refresh
                    .as_ref()
                    .map(|token| token.secret().as_str())
                    .unwrap_or(&refresh);
                sqlx::query(
                    "UPDATE oidc_grants SET access_token = $2, refresh_token = $3, \
                     access_expires_at = NOW() + make_interval(secs => $4), refreshed_at = NOW() WHERE id = $1",
                )
                .bind(grant.id)
                .bind(self.seal("access", grant.id, user, refreshed.access.secret()))
                .bind(self.seal("refresh", grant.id, user, next))
                .bind(lifetime(refreshed.expires_in))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(Some(ProviderToken(refreshed.access.secret().clone())))
            }
            Err(RefreshFailure::Rejected) => {
                self.end(&mut tx, &grant, "refused").await?;
                tx.commit().await?;
                Ok(None)
            }
            Err(RefreshFailure::Unavailable) => Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "The identity provider is unavailable; try again shortly".into(),
            )),
        }
    }

    /// Ends the sign-in a refused grant belonged to, with the API tokens made in it.
    /// The provider already refused the token, so it is not queued for revocation.
    async fn end(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        grant: &GrantRow,
        reason: &str,
    ) -> Result<(), ApiError> {
        sqlx::query("DELETE FROM oidc_grants WHERE id = $1")
            .bind(grant.id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM oidc_revocations WHERE grant_id = $1")
            .bind(grant.id)
            .execute(&mut **tx)
            .await?;
        end_session(tx, grant.user_id, grant.session_id).await?;
        accounts::record_audit(
            tx,
            None,
            "sign_in_ended",
            "session",
            Some(&grant.session_id.to_string()),
            json!({"userId": grant.user_id, "reason": reason}),
        )
        .await?;
        tracing::info!(reason, "the identity provider ended a sign-in");
        Ok(())
    }

    /// Revokes the refresh tokens of sign-ins that ended, at the provider's
    /// revocation endpoint when it has one. Best effort: a token the provider can't
    /// take now is tried again later, a few times.
    pub async fn revoke_ended(&self) {
        loop {
            let claimed = sqlx::query_as::<_, Revocation>(
                "UPDATE oidc_revocations SET attempts = attempts + 1, claimed_at = NOW() \
                 WHERE grant_id IN (SELECT grant_id FROM oidc_revocations \
                 WHERE claimed_at IS NULL OR claimed_at < NOW() - INTERVAL '1 minute' \
                 ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 20) \
                 RETURNING grant_id, user_id, issuer, refresh_token, attempts",
            )
            .fetch_all(&self.pool)
            .await;
            let claimed = match claimed {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::warn!("could not read sign-ins to revoke: {error}");
                    return;
                }
            };
            for row in &claimed {
                let done = row.issuer != self.issuer
                    || match self.open("refresh", row.grant_id, row.user_id, &row.refresh_token) {
                        Some(token) => self.oidc.revoke(&token).await,
                        None => true,
                    };
                if !done && row.attempts < REVOKE_ATTEMPTS {
                    continue;
                }
                if !done {
                    tracing::warn!("gave up revoking an ended sign-in at the identity provider");
                }
                let _ = sqlx::query("DELETE FROM oidc_revocations WHERE grant_id = $1")
                    .bind(row.grant_id)
                    .execute(&self.pool)
                    .await;
            }
            if claimed.len() < 20 {
                return;
            }
        }
    }

    /// Ends the grants of expired sessions and revokes what ended.
    pub async fn sweep(&self) {
        if let Err(error) = sqlx::query(
            "DELETE FROM oidc_grants g USING auth_sessions s WHERE s.id = g.session_id AND s.expires_at <= NOW()",
        )
        .execute(&self.pool)
        .await
        {
            tracing::warn!("could not end expired sign-ins: {error}");
        }
        let _ =
            sqlx::query("DELETE FROM oidc_revocations WHERE created_at < NOW() - INTERVAL '1 day'")
                .execute(&self.pool)
                .await;
        self.revoke_ended().await;
    }

    /// Sweeps every 30 seconds until shutdown.
    pub async fn worker(self, shutdown: tokio_util::sync::CancellationToken) {
        loop {
            self.sweep().await;
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
        }
    }
}
