//! Account administration stays in the same authenticated/CSRF-protected service.
//! Access links are random bearer credentials shown once and stored only as hashes.
use super::*;
use axum::routing::{delete, put};
use serde_json::Value;

pub(super) fn router() -> Router<Auth> {
    Router::new()
        .route("/api/profile", patch(profile))
        .route("/api/auth/sessions", get(sessions))
        .route("/api/auth/sessions/{id}", delete(revoke_session))
        .route("/api/auth/sessions/revoke-others", post(revoke_others))
        .route(
            "/api/admin/users/{id}/revoke-sessions",
            post(admin_revoke_sessions),
        )
        .route("/api/admin/groups", get(groups).post(create_group))
        .route(
            "/api/admin/groups/{id}",
            get(group).patch(update_group).delete(delete_group),
        )
        .route(
            "/api/admin/groups/{id}/members/{user_id}",
            put(add_member).delete(remove_member),
        )
        .route("/api/admin/audit", get(audit))
        .route("/api/admin/invitations", post(invite))
        .route("/api/admin/users/{id}/access-link", post(access_link))
        .route("/api/auth/activate", post(activate))
}

pub(crate) async fn record_audit(
    tx: &mut Transaction<'_, Postgres>,
    actor_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Option<&str>,
    metadata: Value,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,target_id,metadata) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(Uuid::new_v4()).bind(actor_id).bind(action).bind(target_type).bind(target_id).bind(metadata).execute(&mut **tx).await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileInput {
    display_name: String,
}
async fn profile(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Json(input): Json<ProfileInput>,
) -> Result<Json<Value>, ApiError> {
    let user = auth.require(&headers, true).await?;
    let name = validate_display_name(&input.display_name)?;
    let mut tx = auth.pool.begin().await?;
    let user = sqlx::query_as::<_, Identity>("UPDATE users SET display_name=$2,updated_at=NOW() WHERE id=$1 AND enabled RETURNING id,username,display_name,role")
        .bind(user.id).bind(name).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::unauthorized)?;
    record_audit(
        &mut tx,
        Some(user.id),
        "profile_updated",
        "user",
        Some(&user.id.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"user":user})))
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct SessionView {
    id: Uuid,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    current: bool,
    user_agent: String,
}
async fn sessions(State(auth): State<Auth>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = auth.require_session(&headers, false, true).await?;
    let token = auth
        .session_token(&headers)
        .ok_or_else(ApiError::unauthorized)?;
    let sessions=sqlx::query_as::<_,SessionView>("SELECT id,created_at,expires_at,token_hash=$2 AS current,user_agent FROM auth_sessions WHERE user_id=$1 AND expires_at>NOW() ORDER BY created_at DESC,id DESC LIMIT 32")
        .bind(user.id).bind(digest(&token)).fetch_all(&auth.pool).await?;
    Ok(Json(json!({"sessions":sessions})))
}
async fn revoke_session(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let user = auth.require_session(&headers, true, true).await?;
    let token = auth
        .session_token(&headers)
        .ok_or_else(ApiError::unauthorized)?;
    let mut tx = auth.pool.begin().await?;
    let current: bool = sqlx::query_scalar(
        "DELETE FROM auth_sessions WHERE id=$1 AND user_id=$2 RETURNING token_hash=$3",
    )
    .bind(id)
    .bind(user.id)
    .bind(digest(&token))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    // Signing yourself out keeps your tokens; signing out another device ends the
    // tokens made on it.
    let tokens = if current {
        0
    } else {
        super::tokens::revoke_made_in(&mut tx, user.id, &[id]).await?
    };
    record_audit(
        &mut tx,
        Some(user.id),
        "session_revoked",
        "session",
        Some(&id.to_string()),
        json!({"current":current,"tokens":tokens}),
    )
    .await?;
    tx.commit().await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    if current {
        response.headers_mut().insert(
            header::SET_COOKIE,
            auth.cookie(auth.session_cookie_name(), "", 0),
        );
    }
    Ok(response)
}
async fn revoke_others(
    State(auth): State<Auth>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user = auth.require_session(&headers, true, true).await?;
    let token = auth
        .session_token(&headers)
        .ok_or_else(ApiError::unauthorized)?;
    let mut tx = auth.pool.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let ended: Vec<Uuid> = sqlx::query_scalar(
        "DELETE FROM auth_sessions WHERE user_id=$1 AND token_hash<>$2 RETURNING id",
    )
    .bind(user.id)
    .bind(digest(&token))
    .fetch_all(&mut *tx)
    .await?;
    let tokens = super::tokens::revoke_made_in(&mut tx, user.id, &ended).await?;
    record_audit(
        &mut tx,
        Some(user.id),
        "other_sessions_revoked",
        "user",
        Some(&user.id.to_string()),
        json!({"count":ended.len(),"tokens":tokens}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn admin_revoke_sessions(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    require_user(&mut tx, id).await?;
    let count = super::end_sign_ins(&mut tx, id).await?;
    record_audit(
        &mut tx,
        Some(admin.id),
        "user_sessions_revoked",
        "user",
        Some(&id.to_string()),
        json!({"count":count}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn require_user(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<(), ApiError> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(())
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct GroupView {
    id: Uuid,
    name: String,
    description: String,
    member_count: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupInput {
    name: String,
    #[serde(default)]
    description: String,
}
fn group_input(input: GroupInput) -> Result<(String, String), ApiError> {
    let name = validate_display_name(&input.name).map_err(|_| group_name_error())?;
    let description = input.description.trim().to_owned();
    if description.chars().count() > 1000
        || description
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(ApiError::bad(
            "Group description must contain at most 1000 characters",
        ));
    }
    Ok((name, description))
}
fn group_error(error: sqlx::Error) -> ApiError {
    match error {
        sqlx::Error::Database(e) if e.is_unique_violation() => {
            ApiError(StatusCode::CONFLICT, "Group name already exists".into())
        }
        error => error.into(),
    }
}
fn group_name_error() -> ApiError {
    ApiError::bad("Group names are 1 to 100 characters")
}

async fn groups(State(auth): State<Auth>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    auth.require_admin(&headers, false).await?;
    let groups=sqlx::query_as::<_,GroupView>("SELECT g.id,g.name,g.description,COUNT(m.user_id) AS member_count FROM user_groups g LEFT JOIN user_group_members m ON m.group_id=g.id GROUP BY g.id ORDER BY lower(g.name),g.id LIMIT 500").fetch_all(&auth.pool).await?;
    Ok(Json(json!({"groups":groups})))
}
async fn create_group(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Json(input): Json<GroupInput>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    let (name, description) = group_input(input)?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_groups")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 500 {
        return Err(ApiError::bad(
            "This installation has reached its 500-group limit",
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups(id,name,description) VALUES($1,$2,$3)")
        .bind(id)
        .bind(&name)
        .bind(&description)
        .execute(&mut *tx)
        .await
        .map_err(group_error)?;
    record_audit(
        &mut tx,
        Some(admin.id),
        "group_created",
        "group",
        Some(&id.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"group":GroupView{id,name,description,member_count:0}})),
    ))
}
#[derive(Deserialize)]
struct MemberQuery {
    limit: Option<i64>,
    after: Option<Uuid>,
    q: Option<String>,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct MemberView {
    id: Uuid,
    username: String,
    display_name: String,
    role: Role,
    enabled: bool,
    managed_oidc: bool,
}
async fn group(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<MemberQuery>,
) -> Result<Json<Value>, ApiError> {
    auth.require_admin(&headers, false).await?;
    let limit = query.limit.unwrap_or(200);
    let search = query.q.as_deref().unwrap_or("").trim();
    if !(1..=1000).contains(&limit) || search.len() > 256 {
        return Err(ApiError::bad("Invalid member search or limit"));
    }
    let group=sqlx::query_as::<_,GroupView>("SELECT g.id,g.name,g.description,COUNT(m.user_id) AS member_count FROM user_groups g LEFT JOIN user_group_members m ON m.group_id=g.id WHERE g.id=$1 GROUP BY g.id").bind(id).fetch_optional(&auth.pool).await?.ok_or_else(ApiError::not_found)?;
    let mut members=sqlx::query_as::<_,MemberView>("SELECT u.id,u.username,u.display_name,u.role,u.enabled,m.managed_oidc FROM users u JOIN user_group_members m ON m.user_id=u.id WHERE m.group_id=$1 AND ($2::uuid IS NULL OR u.id>$2) AND ($3='' OR strpos(lower(u.username),lower($3))>0 OR strpos(lower(u.display_name),lower($3))>0) ORDER BY u.id LIMIT $4")
        .bind(id).bind(query.after).bind(search).bind(limit+1).fetch_all(&auth.pool).await?;
    let has_more = members.len() > limit as usize;
    members.truncate(limit as usize);
    let cursor = if has_more {
        members.last().map(|m| m.id)
    } else {
        None
    };
    Ok(Json(
        json!({"group":group,"members":members,"hasMore":has_more,"nextCursor":cursor}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupPatch {
    name: Option<String>,
    description: Option<String>,
}
async fn update_group(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<GroupPatch>,
) -> Result<StatusCode, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    if input.name.is_none() && input.description.is_none() {
        return Err(ApiError::bad("Provide a group field to update"));
    }
    let name = input
        .name
        .as_deref()
        .map(|n| validate_display_name(n).map_err(|_| group_name_error()))
        .transpose()?;
    let description = input
        .description
        .map(|description| {
            group_input(GroupInput {
                name: "group".into(),
                description,
            })
            .map(|(_, d)| d)
        })
        .transpose()?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    let changed=sqlx::query("UPDATE user_groups SET name=COALESCE($2,name),description=COALESCE($3,description) WHERE id=$1").bind(id).bind(name).bind(description).execute(&mut *tx).await.map_err(group_error)?.rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found());
    }
    record_audit(
        &mut tx,
        Some(admin.id),
        "group_updated",
        "group",
        Some(&id.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn delete_group(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    // What was shared with the group goes back to its owner alone.
    for table in ["projects", "knowledge_collections"] {
        sqlx::query(&format!(
            "UPDATE {table} SET scope='personal', group_id=NULL WHERE group_id=$1"
        ))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE conversations SET share_scope='none', share_group_id=NULL WHERE share_group_id=$1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if sqlx::query("DELETE FROM user_groups WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 0
    {
        return Err(ApiError::not_found());
    }
    record_audit(
        &mut tx,
        Some(admin.id),
        "group_deleted",
        "group",
        Some(&id.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn add_member(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    change_member(auth, headers, id, user_id, true).await
}
async fn remove_member(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    change_member(auth, headers, id, user_id, false).await
}
async fn change_member(
    auth: Auth,
    headers: HeaderMap,
    id: Uuid,
    user_id: Uuid,
    add: bool,
) -> Result<StatusCode, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    require_user(&mut tx, user_id).await?;
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM user_groups WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let changed = if add {
        sqlx::query(
            "INSERT INTO user_group_members(group_id,user_id) VALUES($1,$2) ON CONFLICT (group_id,user_id) DO UPDATE SET managed_oidc=FALSE WHERE user_group_members.managed_oidc",
        )
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
    } else {
        sqlx::query("DELETE FROM user_group_members WHERE group_id=$1 AND user_id=$2")
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
    };
    if changed > 0 {
        record_audit(
            &mut tx,
            Some(admin.id),
            if add {
                "group_member_added"
            } else {
                "group_member_removed"
            },
            "group",
            Some(&id.to_string()),
            json!({"userId":user_id}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AuditQuery {
    limit: Option<i64>,
    before: Option<Uuid>,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct AuditView {
    id: Uuid,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    action: String,
    target_type: String,
    target_id: Option<String>,
    metadata: Value,
    created_at: DateTime<Utc>,
}
async fn audit(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Value>, ApiError> {
    auth.require_admin(&headers, false).await?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::bad("Audit limit must be between 1 and 200"));
    }
    let cursor = if let Some(id) = query.before {
        Some(
            sqlx::query_as::<_, (DateTime<Utc>, Uuid)>(
                "SELECT created_at,id FROM audit_events WHERE id=$1",
            )
            .bind(id)
            .fetch_optional(&auth.pool)
            .await?
            .ok_or_else(|| ApiError::bad("Audit cursor not found"))?,
        )
    } else {
        None
    };
    let mut events=sqlx::query_as::<_,AuditView>("SELECT e.id,e.actor_id,u.username AS actor_name,e.action,e.target_type,e.target_id,e.metadata,e.created_at FROM audit_events e LEFT JOIN users u ON u.id=e.actor_id WHERE ($1::timestamptz IS NULL OR (e.created_at,e.id)<($1,$2)) ORDER BY e.created_at DESC,e.id DESC LIMIT $3")
        .bind(cursor.map(|c|c.0)).bind(cursor.map(|c|c.1)).bind(limit+1).fetch_all(&auth.pool).await?;
    let more = events.len() > limit as usize;
    events.truncate(limit as usize);
    let next = if more {
        events.last().map(|e| e.id)
    } else {
        None
    };
    Ok(Json(json!({"events":events,"nextCursor":next})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InviteInput {
    username: String,
    display_name: Option<String>,
    role: Option<String>,
    expires_in_hours: Option<i64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccessInput {
    kind: String,
    expires_in_hours: Option<i64>,
}
fn expiry(hours: Option<i64>) -> Result<i64, ApiError> {
    let hours = hours.unwrap_or(24);
    if !(1..=168).contains(&hours) {
        return Err(ApiError::bad("Link expiry must be between 1 and 168 hours"));
    }
    Ok(hours)
}
async fn new_link(
    auth: &Auth,
    tx: &mut Transaction<'_, Postgres>,
    admin: Uuid,
    user_id: Uuid,
    kind: &str,
    hours: i64,
) -> Result<Value, ApiError> {
    let token = random_token();
    let expires_at = Utc::now() + chrono::Duration::hours(hours);
    sqlx::query("DELETE FROM account_access_tokens WHERE user_id=$1 OR expires_at<=NOW()")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO account_access_tokens(token_hash,user_id,kind,expires_at) VALUES($1,$2,$3,$4)",
    )
    .bind(digest(&token))
    .bind(user_id)
    .bind(kind)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    record_audit(
        tx,
        Some(admin),
        "access_link_created",
        "user",
        Some(&user_id.to_string()),
        json!({"kind":kind,"expiresAt":expires_at}),
    )
    .await?;
    Ok(
        json!({"url":format!("{}/#activate/{}",auth.origin(),token),"expiresAt":expires_at,"kind":kind}),
    )
}
async fn invite(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Json(input): Json<InviteInput>,
) -> Result<Response, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    let username = normalize_username(&input.username)?;
    let name = validate_display_name(input.display_name.as_deref().unwrap_or(&username))?;
    let role = Role::requested(input.role.as_deref())?.unwrap_or(Role::User);
    let hours = expiry(input.expires_in_hours)?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    let user=sqlx::query_as::<_,Identity>("INSERT INTO users(id,username,display_name,role) VALUES($1,$2,$3,$4) RETURNING id,username,display_name,role")
        .bind(Uuid::new_v4()).bind(username).bind(name).bind(role).fetch_one(&mut *tx).await.map_err(|e|match e{sqlx::Error::Database(e)if e.is_unique_violation()=>ApiError(StatusCode::CONFLICT,"Username already exists".into()),e=>e.into()})?;
    auth.group_policy.initial(&mut tx, user.id).await?;
    record_audit(
        &mut tx,
        Some(admin.id),
        "user_invited",
        "user",
        Some(&user.id.to_string()),
        json!({"role":user.role}),
    )
    .await?;
    let mut value = new_link(&auth, &mut tx, admin.id, user.id, "invite", hours).await?;
    value["user"] = json!(user);
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(value),
    )
        .into_response())
}
async fn access_link(
    State(auth): State<Auth>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<AccessInput>,
) -> Result<Response, ApiError> {
    let admin = auth.require_admin(&headers, true).await?;
    if !["invite", "reset"].contains(&input.kind.as_str()) {
        return Err(ApiError::bad("Link kind must be invite or reset"));
    }
    let hours = expiry(input.expires_in_hours)?;
    let mut tx = auth.pool.begin().await?;
    auth.lock_admin(&mut tx, admin.id).await?;
    let (local, enabled): (bool, bool) = sqlx::query_as(
        "SELECT password_hash IS NOT NULL,enabled FROM users WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let oidc: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_identities WHERE user_id=$1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !enabled || (input.kind == "reset" && !local) || (input.kind == "invite" && (local || oidc))
    {
        return Err(ApiError::bad(
            "That link type is not available for this account",
        ));
    }
    let value = new_link(&auth, &mut tx, admin.id, id, &input.kind, hours).await?;
    tx.commit().await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivateInput {
    token: String,
    new_password: String,
}
fn invalid_link() -> ApiError {
    ApiError::bad("This account link is invalid, expired, or already used")
}
async fn activate(
    State(auth): State<Auth>,
    client: Client,
    headers: HeaderMap,
    Json(input): Json<ActivateInput>,
) -> Result<Response, ApiError> {
    auth.check_origin(&headers)?;
    if input.token.len() != 43
        || !input
            .token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid_link());
    }
    let link_key = format!(
        "access-link:{}",
        URL_SAFE_NO_PAD.encode(digest(&input.token))
    );
    auth.throttle("login", &client, Some(&link_key)).await?;
    validate_password(&input.new_password)?;
    let user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM account_access_tokens WHERE token_hash=$1 AND expires_at>NOW()",
    )
    .bind(digest(&input.token))
    .fetch_optional(&auth.pool)
    .await?;
    let Some(user_id) = user_id else {
        return Err(invalid_link());
    };
    let hash = auth.new_password_hash(input.new_password).await?;
    let mut tx = auth.pool.begin().await?;
    let user = sqlx::query_as::<_, Identity>(
        "SELECT id,username,display_name,role FROM users WHERE id=$1 AND enabled FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(invalid_link)?;
    let kind:String=sqlx::query_scalar("DELETE FROM account_access_tokens WHERE token_hash=$1 AND user_id=$2 AND expires_at>NOW() RETURNING kind").bind(digest(&input.token)).bind(user.id).fetch_optional(&mut *tx).await?.ok_or_else(invalid_link)?;
    // Recheck issuance rules: an OIDC link or local password added after the
    // link was issued must not let it set or replace a password.
    if sqlx::query("UPDATE users SET password_hash=$2,password_change_required=FALSE,updated_at=NOW() WHERE id=$1 \
        AND (($3='invite' AND password_hash IS NULL AND NOT EXISTS(SELECT 1 FROM oidc_identities WHERE user_id=$1)) \
        OR ($3='reset' AND password_hash IS NOT NULL))")
        .bind(user.id).bind(hash).bind(&kind).execute(&mut *tx).await?.rows_affected() == 0
    {
        return Err(invalid_link());
    }
    sqlx::query("DELETE FROM account_access_tokens WHERE user_id=$1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    // A reset ends every session and personal token made with the old password.
    super::end_sign_ins(&mut tx, user.id).await?;
    // The browser's previous session, possibly another user's, is replaced too.
    if let Some(old_token) = auth.session_token(&headers) {
        sqlx::query("DELETE FROM auth_sessions WHERE token_hash = $1")
            .bind(digest(&old_token))
            .execute(&mut *tx)
            .await?;
    }
    let token = random_token();
    let lifetime = auth.config.session_hours as i64 * 3600;
    auth.insert_session(&mut tx, user.id, &token, lifetime, &headers)
        .await?;
    record_audit(
        &mut tx,
        Some(user.id),
        "access_link_redeemed",
        "user",
        Some(&user.id.to_string()),
        json!({"kind":kind}),
    )
    .await?;
    tx.commit().await?;
    Ok(auth.session_response(user, &token, lifetime, false))
}
