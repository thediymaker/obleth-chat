//! Personal API tokens: bearer credentials for the MCP endpoint and nothing else.
//! A signed-in person makes one from a recent sign-in; it is shown once, stored only
//! as a hash, expires within 30 days, and reads or runs only its owner's chats.
use super::*;
use axum::routing::delete;

pub const READ: &str = "chats:read";
pub const WRITE: &str = "chats:write";
const MARKER: &str = "obc_";
const MAX_ACTIVE: i64 = 10;
const CALLS_PER_MINUTE: i32 = 120;
const LIFETIMES: [i32; 3] = [1, 7, 30];
/// Making a token needs a sign-in this recent, so a borrowed or stolen session
/// cookie cannot mint one: sessions last much longer than this.
const RECENT_SIGN_IN_MINUTES: i32 = 15;

pub(super) fn router() -> Router<Auth> {
    Router::new()
        .route("/api/auth/tokens", get(list).post(create))
        .route("/api/auth/tokens/{id}", delete(revoke))
}

/// Who a live token acts for, and what it may do.
pub struct TokenUser {
    pub id: Uuid,
    pub user: Identity,
    pub scopes: Vec<String>,
}

impl TokenUser {
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

#[derive(FromRow)]
struct TokenRow {
    token_id: Uuid,
    scopes: Vec<String>,
    window_calls: i32,
    #[sqlx(flatten)]
    user: Identity,
}

impl Auth {
    /// The person a bearer token acts for, when the token is live and its owner may
    /// still sign in. Each call counts against the token's per-minute allowance.
    pub async fn require_token(&self, headers: &HeaderMap) -> Result<TokenUser, ApiError> {
        let mut values = headers.get_all(header::AUTHORIZATION).iter();
        let (Some(value), None) = (values.next(), values.next()) else {
            return Err(ApiError::unauthorized());
        };
        let secret = value
            .to_str()
            .ok()
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .and_then(|(_, token)| token.trim().strip_prefix(MARKER))
            .filter(|s| {
                s.len() == 43
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            })
            .ok_or_else(ApiError::unauthorized)?;
        // One statement checks the token and its owner and counts the call, so
        // replicas share the allowance.
        let row = sqlx::query_as::<_, TokenRow>(
            "UPDATE api_tokens t SET \
             window_calls = CASE WHEN t.window_start <= NOW() - INTERVAL '1 minute' THEN 1 ELSE t.window_calls + 1 END, \
             window_start = CASE WHEN t.window_start <= NOW() - INTERVAL '1 minute' THEN NOW() ELSE t.window_start END, \
             last_used_at = NOW() \
             FROM users u WHERE t.token_hash = $1 AND t.revoked_at IS NULL AND t.expires_at > NOW() \
             AND u.id = t.user_id AND u.enabled AND NOT u.password_change_required \
             RETURNING t.id AS token_id, t.scopes, t.window_calls, u.id, u.username, u.display_name, u.role",
        )
        .bind(digest(&format!("{MARKER}{secret}")))
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
        if row.window_calls > CALLS_PER_MINUTE {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "This token is making too many calls. Wait a minute and try again.".into(),
            ));
        }
        Ok(TokenUser {
            id: row.token_id,
            user: row.user,
            scopes: row.scopes,
        })
    }
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct TokenView {
    id: Uuid,
    name: String,
    prefix: String,
    scopes: Vec<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}
const VIEW: &str = "id,name,prefix,scopes,created_at,expires_at,last_used_at";

async fn list(State(auth): State<Auth>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = auth.require(&headers, false).await?;
    let tokens = sqlx::query_as::<_, TokenView>(&format!(
        "SELECT {VIEW} FROM api_tokens WHERE user_id=$1 AND revoked_at IS NULL AND expires_at>NOW() ORDER BY created_at DESC,id"
    ))
    .bind(user.id)
    .fetch_all(&auth.pool)
    .await?;
    Ok(Json(json!({"tokens":tokens})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateInput {
    name: String,
    days: i32,
    #[serde(default)]
    read_only: bool,
}

async fn create(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Json(input): Json<CreateInput>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = auth.require(&headers, true).await?;
    let session = auth
        .session_token(&headers)
        .ok_or_else(ApiError::unauthorized)?;
    let session: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, created_at > NOW() - make_interval(mins => $2) FROM auth_sessions WHERE token_hash=$1",
    )
    .bind(digest(&session))
    .bind(RECENT_SIGN_IN_MINUTES)
    .fetch_optional(&auth.pool)
    .await?;
    let Some((session, true)) = session else {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Sign in again to create a token".into(),
        ));
    };
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(ApiError::bad(
            "Give the token a name of up to 64 characters",
        ));
    }
    if !LIFETIMES.contains(&input.days) {
        return Err(ApiError::bad("A token lasts 1, 7 or 30 days"));
    }
    let scopes: Vec<&str> = if input.read_only {
        vec![READ]
    } else {
        vec![READ, WRITE]
    };
    let mut tx = auth.pool.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM api_tokens WHERE user_id=$1 AND revoked_at IS NULL AND expires_at>NOW()",
    )
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if active >= MAX_ACTIVE {
        return Err(ApiError::bad(format!(
            "You can have {MAX_ACTIVE} tokens at once. Revoke one first."
        )));
    }
    let secret = random_token();
    let token = format!("{MARKER}{secret}");
    let id = Uuid::new_v4();
    let view = sqlx::query_as::<_, TokenView>(&format!(
        "INSERT INTO api_tokens(id,user_id,name,token_hash,prefix,scopes,expires_at,session_id) \
         VALUES($1,$2,$3,$4,$5,$6,NOW() + make_interval(days => $7),$8) RETURNING {VIEW}"
    ))
    .bind(id)
    .bind(user.id)
    .bind(name)
    .bind(digest(&token))
    .bind(&secret[..6])
    .bind(&scopes)
    .bind(input.days)
    .bind(session)
    .fetch_one(&mut *tx)
    .await?;
    accounts::record_audit(
        &mut tx,
        Some(user.id),
        "api_token_created",
        "api_token",
        Some(&id.to_string()),
        json!({"name":name,"scopes":scopes,"days":input.days}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"token":token,"apiToken":view})),
    ))
}

async fn revoke(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = auth.require(&headers, true).await?;
    let mut tx = auth.pool.begin().await?;
    sqlx::query_scalar::<_, Uuid>(
        "UPDATE api_tokens SET revoked_at=NOW() WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL RETURNING id",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    accounts::record_audit(
        &mut tx,
        Some(user.id),
        "api_token_revoked",
        "api_token",
        Some(&id.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Ends the tokens made in browser sessions that were just signed out, so a stolen
/// session's tokens end when the person signs it out from another device. Returns
/// how many ended.
pub(super) async fn revoke_made_in(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    sessions: &[Uuid],
) -> Result<u64, ApiError> {
    Ok(sqlx::query(
        "UPDATE api_tokens SET revoked_at=NOW() WHERE user_id=$1 AND session_id = ANY($2) AND revoked_at IS NULL",
    )
    .bind(user)
    .bind(sessions)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

/// Ends a person's tokens. Reset paths call it through `end_sign_ins`, which also
/// ends their sessions.
pub(super) async fn revoke_all(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE api_tokens SET revoked_at=NOW() WHERE user_id=$1 AND revoked_at IS NULL")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
