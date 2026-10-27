//! The identity provider's tokens for each sign-in, kept when Chat forwards people's
//! own access tokens to the gateway (OBLETH_CHAT_GATEWAY_AUTH=forward). A grant
//! belongs to one browser session and ends with it. Its tokens are sealed with the
//! installation's secret key, and the access token leaves Chat only in the gateway's
//! Authorization header.
use super::oidc::{Oidc, RefreshFailure, Refreshed};
use super::*;
use openidconnect::{AccessToken, RefreshToken};
use tokio_util::task::TaskTracker;

/// Format byte of sealed grant tokens.
const SEALED: u8 = 1;
/// An access token with less than this left is refreshed before use.
const REFRESH_MARGIN_SECS: f64 = 60.0;
/// Assumed lifetime of an access token the provider gave no expiry for.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(300);
/// Once a refresh token went to the provider without an answer Chat could read,
/// the provider may have rotated it. Providers that rotate refresh tokens refuse
/// the old one for a short while and may then take its reuse as theft, ending the
/// sign-in at the provider. So Chat presents a token again only within this long
/// of first sending it; after that it ends the sign-in itself.
const PRESENT_WINDOW: Duration = Duration::from_secs(45);
/// How long one replica holds a grant's refresh. Well past PRESENT_WINDOW plus the
/// last answer and its save, so a holder still at work keeps its lease; a lease
/// that lapses means its holder stopped, maybe after the provider rotated the
/// token, and the grant ends rather than present that token again.
const LEASE: Duration = Duration::from_secs(120);
/// How long a request waits for another's refresh of an expired access token.
const WAIT: Duration = Duration::from_secs(80);
/// After the provider couldn't answer a grant's refresh, no one asks it again for
/// that grant for this long.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// How long saving a refreshed token is retried while the database fails.
const SAVE_FOR: Duration = Duration::from_secs(90);
/// How long a sign-in's check waits for a refresh while it is still in the grace.
const CONFIRM_WAIT: Duration = Duration::from_secs(3);
/// How long after its access token expired a sign-in is still trusted when the
/// provider can't be asked.
const UNCONFIRMED_GRACE: Duration = Duration::from_secs(15 * 60);
/// The columns of a GrantRow, from `oidc_grants g`; $3 is the refresh margin.
const COLUMNS: &str = "g.id, g.user_id, g.session_id, g.access_token, g.refresh_token, \
     g.access_expires_at > NOW() + make_interval(secs => $3) AS fresh, \
     g.access_expires_at > NOW() AS live";
/// Revocations tried this many times are dropped; the token expires on its own.
const REVOKE_ATTEMPTS: i32 = 5;

#[derive(Clone)]
pub struct Grants {
    pool: PgPool,
    key: SecretKey,
    oidc: Arc<Oidc>,
    issuer: String,
    /// Refreshes under way, which shutdown waits for so a rotated token is saved.
    tasks: TaskTracker,
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

impl From<String> for ProviderToken {
    fn from(token: String) -> Self {
        Self(token)
    }
}

fn unavailable() -> ApiError {
    ApiError(
        StatusCode::BAD_GATEWAY,
        "The identity provider is unavailable; try again shortly".into(),
    )
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
    /// Not due for a refresh.
    fresh: bool,
    /// The access token has not expired yet.
    live: bool,
}

/// A grant whose refresh this replica claimed, until `lease`.
#[derive(FromRow)]
struct Claimed {
    #[sqlx(flatten)]
    grant: GrantRow,
    lease: DateTime<Utc>,
    /// An earlier request sent the token and no answer came back.
    doubt: bool,
    /// Seconds left in which the token may still be sent.
    send_for: f64,
}

#[derive(FromRow)]
struct WaitingRow {
    #[sqlx(flatten)]
    grant: GrantRow,
    /// Whether the provider failed to answer a refresh of it just now.
    failing: bool,
    /// Whether its refresh token may have been rotated too long ago to present: a
    /// request sent it with no answer longer than PRESENT_WINDOW ago, or a holder
    /// stopped without giving its lease back.
    doomed: bool,
}

/// What became of a refreshed token on saving it.
enum Saved {
    /// It is the grant's token now, or another refresh already replaced the one it
    /// came from; either way the grant goes on.
    Kept,
    /// The sign-in ended while the provider answered.
    Ended,
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
            tasks: TaskTracker::new(),
        }
    }

    /// Starts no new refresh, and sends no more tokens to the provider, from now
    /// on: one begun at shutdown could outlast the process.
    pub fn close(&self) {
        self.tasks.close();
    }

    /// Waits for the refreshes under way, so none loses a rotated token at shutdown.
    pub async fn finish(&self) {
        self.tasks.close();
        self.tasks.wait().await;
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
    /// one only ever gets their own token. A call from a sign-in that has a grant
    /// (a browser session, or an API token made in one) uses that grant alone, so
    /// ending one sign-in at the provider ends it in Chat too. Other calls, such as
    /// Auto mode and Knowledge indexing, use the most recently refreshed live grant.
    /// A grant is refreshed first when it is about to expire; one the provider
    /// refuses ends its sign-in, and then the call must sign in again or, for work
    /// without a sign-in of its own, tries the next grant.
    pub async fn access_token(
        &self,
        user: Uuid,
        session: Option<Uuid>,
    ) -> Result<Option<ProviderToken>, ApiError> {
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
        if let Some(session) = session {
            let own = sqlx::query_as::<_, GrantRow>(&format!(
                "SELECT {COLUMNS} FROM oidc_grants g JOIN auth_sessions s ON s.id = g.session_id \
                 WHERE g.session_id = $1 AND g.user_id = $2 AND g.issuer = $4 AND s.expires_at > NOW()"
            ))
            .bind(session)
            .bind(user)
            .bind(REFRESH_MARGIN_SECS)
            .bind(&self.issuer)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(grant) = own {
                return match self.current(&grant).await? {
                    Some(token) => Ok(Some(token)),
                    None => Err(sign_in_again()),
                };
            }
        }
        loop {
            let grant = sqlx::query_as::<_, GrantRow>(&format!(
                "SELECT {COLUMNS} FROM oidc_grants g JOIN auth_sessions s ON s.id = g.session_id \
                 WHERE g.user_id = $1 AND g.issuer = $2 AND s.expires_at > NOW() \
                 ORDER BY g.refreshed_at DESC, g.id LIMIT 1"
            ))
            .bind(user)
            .bind(&self.issuer)
            .bind(REFRESH_MARGIN_SECS)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(sign_in_again)?;
            if let Some(token) = self.current(&grant).await? {
                return Ok(Some(token));
            }
        }
    }

    /// Whether the sign-in behind `session` still holds its grant. Chat checks on
    /// each request once the access token has expired, so the provider ending the
    /// grant ends the sign-in; a refused grant ends the session. A provider that
    /// can't answer is trusted for a short while after the token expired.
    pub async fn confirm(&self, session: Uuid) -> Result<bool, ApiError> {
        // A session that has ended is refused by its own checks, and the sweep ends
        // its grant; a dead sign-in is never refreshed.
        let Some((id, expired, recent, busy)) = sqlx::query_as::<_, (Uuid, bool, bool, bool)>(
            "SELECT g.id, g.access_expires_at <= NOW(), \
             g.access_expires_at > NOW() - make_interval(secs => $3), \
             COALESCE(g.refreshing_until > NOW(), FALSE) \
             OR COALESCE(g.refresh_failed_at > NOW() - make_interval(secs => $4), FALSE) \
             FROM oidc_grants g JOIN auth_sessions s ON s.id = g.session_id \
             WHERE g.session_id = $1 AND g.issuer = $2 AND s.expires_at > NOW()",
        )
        .bind(session)
        .bind(&self.issuer)
        .bind(UNCONFIRMED_GRACE.as_secs_f64())
        .bind(RETRY_AFTER.as_secs_f64())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(true);
        };
        if !expired {
            return Ok(true);
        }
        // Another request is asking the provider, or it just failed to answer: inside
        // the grace there is no need to wait for it or to ask again.
        if busy && recent {
            return Ok(true);
        }
        if !recent {
            return Ok(self.refresh(id).await?.is_some());
        }
        // Inside the grace a slow provider holds no request up: the refresh goes on
        // without it, and a refusal still ends the session for the next one.
        Ok(
            match tokio::time::timeout(CONFIRM_WAIT, self.refresh(id)).await {
                Ok(Ok(token)) => token.is_some(),
                Ok(Err(_)) | Err(_) => true,
            },
        )
    }

    /// The grant's access token, refreshed first when it is about to expire. None
    /// when the grant has ended.
    async fn current(&self, grant: &GrantRow) -> Result<Option<ProviderToken>, ApiError> {
        if grant.fresh {
            if let Some(token) = self.open("access", grant.id, grant.user_id, &grant.access_token) {
                return Ok(Some(token.into()));
            }
        }
        self.refresh(grant.id).await
    }

    /// Refreshes one grant under a lease, so replicas never present the same
    /// refresh token at once. The lease is a column, not a row lock, so no database
    /// connection is held while the provider answers; others use the access token
    /// while it lasts, or wait for the holder by polling. The claim and the refresh
    /// run in their own task, so a caller that goes away (a closed tab, a stopped
    /// chat, a timeout) can't leave a lease without a holder, or stop the holder
    /// between the provider rotating the refresh token and Chat saving the new one.
    /// A token that may have been rotated longer ago than PRESENT_WINDOW is never
    /// presented again: its sign-in ends. After the provider fails to answer, no
    /// one asks it again for a while. Returns None when the grant has ended.
    async fn refresh(&self, id: Uuid) -> Result<Option<ProviderToken>, ApiError> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let closing = self.tasks.is_closed();
            if !closing {
                let grants = self.clone();
                let held = self
                    .tasks
                    .spawn(async move {
                        match grants.claim(id).await? {
                            Some(claimed) => grants.hold(claimed).await.map(Some),
                            None => Ok(None),
                        }
                    })
                    .await
                    .map_err(|_| ApiError::internal())??;
                if let Some(token) = held {
                    return Ok(token);
                }
            }
            let Some(WaitingRow {
                grant,
                failing,
                doomed,
            }) = sqlx::query_as::<_, WaitingRow>(
                "SELECT id, user_id, session_id, access_token, refresh_token, \
                 access_expires_at > NOW() + make_interval(secs => $2) AS fresh, \
                 access_expires_at > NOW() AS live, \
                 COALESCE(refresh_failed_at > NOW() - make_interval(secs => $3), FALSE) AS failing, \
                 COALESCE(refreshing_until <= NOW(), FALSE) OR (refreshing_until IS NULL \
                 AND COALESCE(presented_at <= NOW() - make_interval(secs => $4), FALSE)) AS doomed \
                 FROM oidc_grants WHERE id = $1",
            )
            .bind(id)
            .bind(REFRESH_MARGIN_SECS)
            .bind(RETRY_AFTER.as_secs_f64())
            .bind(PRESENT_WINDOW.as_secs_f64())
            .fetch_optional(&self.pool)
            .await?
            else {
                return Ok(None);
            };
            // Another replica may have refreshed it meanwhile. Whatever keeps this
            // one from refreshing, the access token serves until it expires.
            if grant.fresh || grant.live {
                if let Some(token) =
                    self.open("access", grant.id, grant.user_id, &grant.access_token)
                {
                    return Ok(Some(token.into()));
                }
                if grant.fresh {
                    // Not due for a refresh, so it can't be claimed, and unreadable.
                    tracing::warn!("a stored sign-in could not be decrypted; ending it");
                    if self.end(&grant, "unreadable").await? {
                        return Ok(None);
                    }
                    continue;
                }
            }
            if doomed {
                if self.end(&grant, "unconfirmed").await? {
                    return Ok(None);
                }
                continue;
            }
            // The provider failed to answer just now, or shutdown began: asking again
            // won't help yet.
            if failing || closing || tokio::time::Instant::now() >= deadline {
                return Err(unavailable());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Claims a grant's refresh when it is due, no one holds it, and its refresh
    /// token may still be presented.
    async fn claim(&self, id: Uuid) -> Result<Option<Claimed>, ApiError> {
        Ok(sqlx::query_as::<_, Claimed>(
            "UPDATE oidc_grants SET refreshing_until = NOW() + make_interval(secs => $3), \
             presented_at = COALESCE(presented_at, NOW()) \
             WHERE id = $1 AND access_expires_at <= NOW() + make_interval(secs => $2) \
             AND refreshing_until IS NULL \
             AND (presented_at IS NULL OR presented_at > NOW() - make_interval(secs => $5)) \
             AND (refresh_failed_at IS NULL OR refresh_failed_at <= NOW() - make_interval(secs => $4)) \
             RETURNING id, user_id, session_id, access_token, refresh_token, FALSE AS fresh, \
             access_expires_at > NOW() AS live, refreshing_until AS lease, \
             presented_at < NOW() AS doubt, \
             EXTRACT(EPOCH FROM presented_at + make_interval(secs => $5) - NOW())::FLOAT8 AS send_for",
        )
        .bind(id)
        .bind(REFRESH_MARGIN_SECS)
        .bind(LEASE.as_secs_f64())
        .bind(RETRY_AFTER.as_secs_f64())
        .bind(PRESENT_WINDOW.as_secs_f64())
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Refreshes a grant this replica holds the lease of.
    async fn hold(&self, claimed: Claimed) -> Result<Option<ProviderToken>, ApiError> {
        let Claimed {
            grant,
            lease,
            mut doubt,
            send_for,
        } = claimed;
        let Some(refresh) = self.open("refresh", grant.id, grant.user_id, &grant.refresh_token)
        else {
            tracing::warn!("a stored sign-in could not be decrypted; ending it");
            if self.end(&grant, "unreadable").await? {
                return Ok(None);
            }
            return self.stored(grant.id).await;
        };
        let send_by = tokio::time::Instant::now() + Duration::from_secs_f64(send_for.max(0.0));
        let tasks = self.tasks.clone();
        let may_send = move || tokio::time::Instant::now() < send_by && !tasks.is_closed();
        // Once a request went unanswered, only an answer about the token settles
        // whether the provider rotated it, so it is asked again while it may be.
        let result = loop {
            match self.oidc.refresh(&refresh, may_send.clone()).await {
                Err(RefreshFailure::Unanswered) => doubt = true,
                Err(RefreshFailure::Unavailable) if doubt => {}
                result => break result,
            }
            if !may_send() {
                break Err(RefreshFailure::Unanswered);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        match result {
            Ok(refreshed) => {
                let access = ProviderToken(refreshed.access.secret().clone());
                let saved = self.save(&grant, &refresh, &refreshed).await;
                if !matches!(saved, Ok(Saved::Kept)) {
                    // The grant ended, or will once its lease lapses: its old token
                    // is queued for revocation or never presented again, and the new
                    // one goes too.
                    if let Some(token) = refreshed.refresh {
                        let oidc = self.oidc.clone();
                        self.tasks
                            .spawn(async move { oidc.revoke(token.secret()).await });
                    }
                }
                match saved? {
                    Saved::Kept => Ok(Some(access)),
                    Saved::Ended => Ok(None),
                }
            }
            Err(RefreshFailure::Rejected) => {
                if self.end(&grant, "refused").await? {
                    return Ok(None);
                }
                // Another refresh replaced the token meanwhile, and its result stands.
                self.stored(grant.id).await
            }
            Err(RefreshFailure::Unavailable | RefreshFailure::Unanswered) => {
                // A token the provider may have rotated keeps the time it was first
                // sent: another replica may settle it while that is recent, and the
                // grant ends after. Otherwise no one asks the failing provider again
                // for a while, unless it was shutdown that stopped this refresh.
                let failed = !doubt && !self.tasks.is_closed();
                sqlx::query(
                    "UPDATE oidc_grants SET refreshing_until = NULL, \
                     presented_at = CASE WHEN $3 THEN presented_at END, \
                     refresh_failed_at = CASE WHEN $4 THEN NOW() ELSE refresh_failed_at END \
                     WHERE id = $1 AND refreshing_until = $2",
                )
                .bind(grant.id)
                .bind(lease)
                .bind(doubt)
                .bind(failed)
                .execute(&self.pool)
                .await?;
                if grant.live {
                    if let Some(token) =
                        self.open("access", grant.id, grant.user_id, &grant.access_token)
                    {
                        return Ok(Some(token.into()));
                    }
                }
                Err(unavailable())
            }
        }
    }

    /// Saves a refresh's tokens over the ones it was made from. The provider may
    /// have rotated the refresh token, and the new one exists nowhere else, so a
    /// failing database is retried for a while rather than losing it, often enough
    /// to save it as soon as the database is back.
    async fn save(
        &self,
        grant: &GrantRow,
        old: &str,
        refreshed: &Refreshed,
    ) -> Result<Saved, ApiError> {
        let next = refreshed
            .refresh
            .as_ref()
            .map(|token| token.secret().as_str())
            .unwrap_or(old);
        let access = self.seal("access", grant.id, grant.user_id, refreshed.access.secret());
        let refresh = self.seal("refresh", grant.id, grant.user_id, next);
        let started = tokio::time::Instant::now();
        let mut pause = Duration::from_millis(100);
        loop {
            let attempt: Result<Saved, sqlx::Error> = async {
                let saved = sqlx::query(
                    "UPDATE oidc_grants SET access_token = $2, refresh_token = $3, \
                     access_expires_at = NOW() + make_interval(secs => $4), refreshed_at = NOW(), \
                     refreshing_until = NULL, refresh_failed_at = NULL, presented_at = NULL \
                     WHERE id = $1 AND refresh_token = $5",
                )
                .bind(grant.id)
                .bind(&access)
                .bind(&refresh)
                .bind(lifetime(refreshed.expires_in))
                .bind(&grant.refresh_token)
                .execute(&self.pool)
                .await?
                .rows_affected();
                if saved == 1 {
                    return Ok(Saved::Kept);
                }
                // An earlier attempt saved it and only the database's answer was
                // lost: the lease keeps other refreshes out, and a grant whose lease
                // lapsed ends instead of being refreshed.
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_grants WHERE id = $1)")
                        .bind(grant.id)
                        .fetch_one(&self.pool)
                        .await?;
                Ok(if exists { Saved::Kept } else { Saved::Ended })
            }
            .await;
            match attempt {
                Ok(saved) => return Ok(saved),
                Err(error) if started.elapsed() < SAVE_FOR => {
                    tracing::warn!("could not save a refreshed sign-in yet: {error}");
                    tokio::time::sleep(pause).await;
                    pause = (pause * 2).min(Duration::from_secs(1));
                }
                Err(error) => {
                    tracing::error!("could not save a refreshed sign-in: {error}");
                    return Err(error.into());
                }
            }
        }
    }

    /// The grant's stored access token while it lasts, after another refresh
    /// replaced the one this one began from. None when the grant has ended.
    async fn stored(&self, id: Uuid) -> Result<Option<ProviderToken>, ApiError> {
        let Some((user, sealed, live)) = sqlx::query_as::<_, (Uuid, Vec<u8>, bool)>(
            "SELECT user_id, access_token, access_expires_at > NOW() FROM oidc_grants WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        match self.open("access", id, user, &sealed) {
            Some(token) if live => Ok(Some(token.into())),
            _ => Err(unavailable()),
        }
    }

    /// Ends the sign-in a refused grant belonged to, with the API tokens made in it,
    /// unless another refresh replaced the grant's token meanwhile: then it returns
    /// false and ends nothing. The provider already refused the token, so it is not
    /// queued for revocation.
    async fn end(&self, grant: &GrantRow, reason: &str) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let ended = sqlx::query("DELETE FROM oidc_grants WHERE id = $1 AND refresh_token = $2")
            .bind(grant.id)
            .bind(&grant.refresh_token)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if ended == 0 {
            return Ok(false);
        }
        sqlx::query("DELETE FROM oidc_revocations WHERE grant_id = $1")
            .bind(grant.id)
            .execute(&mut *tx)
            .await?;
        end_session(&mut tx, grant.user_id, grant.session_id).await?;
        accounts::record_audit(
            &mut tx,
            None,
            "sign_in_ended",
            "session",
            Some(&grant.session_id.to_string()),
            json!({"userId": grant.user_id, "reason": reason}),
        )
        .await?;
        tx.commit().await?;
        tracing::info!(reason, "the identity provider ended a sign-in");
        Ok(true)
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
