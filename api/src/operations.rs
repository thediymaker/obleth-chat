//! Durable usage and distributed admission. The caller holds the same user row
//! lock as conversation creation, so replicas cannot oversubscribe a daily cap.
//! Token admission uses reported totals plus reserved output budgets. Missing
//! provider usage remains explicitly incomplete and is never reported as usage,
//! but admission charges an estimate so Stop or errors cannot skip the allowance.
use crate::{auth::Role, error::ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{Extensions, HeaderMap, StatusCode},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

const QUOTA_BASIS: &str =
    "reported tokens (estimated when a provider does not report them) plus reserved output tokens; requests include automatic follow-up suggestions";
/// Admission charge for a finished turn without reported usage: about four bytes
/// per token of context, plus streamed output (all of the reservation if unknown).
fn charge_estimate(output_bytes: &str) -> String {
    format!("CEIL(context_bytes/4.0)::bigint+LEAST(reserved_output_tokens,COALESCE(CEIL(({output_bytes})/4.0)::bigint,reserved_output_tokens))")
}
/// Both durable auxiliary leases outlast the whole task and its bounded cleanup.
pub const FOLLOW_UP_TIMEOUT: Duration = Duration::from_secs(60);
pub const FOLLOW_UP_LEASE_SECONDS: i64 = FOLLOW_UP_TIMEOUT.as_secs() as i64 + 15;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub daily_requests: Option<i64>,
    pub daily_tokens: Option<i64>,
    pub concurrent_requests: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            daily_requests: None,
            daily_tokens: None,
            concurrent_requests: 3,
        }
    }
}
fn merge(base: &mut Value, patch: &Value) {
    if let (Some(a), Some(b)) = (base.as_object_mut(), patch.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
}
fn validate_patch(value: &Value) -> Result<(), ApiError> {
    let Some(object) = value.as_object() else {
        return Err(ApiError::bad("Limits must be an object"));
    };
    for (key, value) in object {
        let valid = match key.as_str() {
            "dailyRequests" => {
                value.is_null() || value.as_i64().is_some_and(|n| (1..=1_000_000).contains(&n))
            }
            "dailyTokens" => {
                value.is_null()
                    || value
                        .as_i64()
                        .is_some_and(|n| (1..=1_000_000_000).contains(&n))
            }
            "concurrentRequests" => value.as_i64().is_some_and(|n| (1..=32).contains(&n)),
            _ => false,
        };
        if !valid {
            return Err(ApiError::bad("Use dailyRequests (null or 1–1,000,000), dailyTokens (null or 1–1,000,000,000), and concurrentRequests (1–32)"));
        }
    }
    Ok(())
}
fn resolved(defaults: Value, overrides: Value) -> Result<Limits, ApiError> {
    validate_patch(&defaults).map_err(|_| ApiError::internal())?;
    validate_patch(&overrides).map_err(|_| ApiError::internal())?;
    let mut value = json!(Limits::default());
    merge(&mut value, &defaults);
    merge(&mut value, &overrides);
    serde_json::from_value(value).map_err(|_| ApiError::internal())
}
async fn limits_in(tx: &mut Transaction<'_, Postgres>, owner: Uuid) -> Result<Limits, ApiError> {
    let defaults: Value = sqlx::query_scalar("SELECT limits FROM site_limits WHERE id=TRUE")
        .fetch_one(&mut **tx)
        .await?;
    let overrides = sqlx::query_scalar("SELECT limits FROM user_limit_overrides WHERE user_id=$1")
        .bind(owner)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or_else(|| json!({}));
    resolved(defaults, overrides)
}
pub async fn effective_limits(pool: &PgPool, owner: Uuid) -> Result<Limits, ApiError> {
    let mut tx = pool.begin().await?;
    let limits = limits_in(&mut tx, owner).await?;
    tx.commit().await?;
    Ok(limits)
}
/// Call after recovering expired conversation leases, before setting the new
/// active_generation. The caller must already hold its owner's users row lock.
pub async fn reserve_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    generation: Uuid,
    max_tokens: i32,
    context_bytes: usize,
) -> Result<(), ApiError> {
    // Interactive turns take priority over this owner's optional housekeeping.
    // The worker observes this revocation and closes its upstream within 1s.
    // Keep the admitted request in the ledger, and accept any actual usage later.
    sqlx::query(&format!("WITH preempted AS (UPDATE generation_usage SET status='interrupted',finished_at=NOW(),usage_complete=FALSE,charged_tokens={} WHERE user_id=$1 AND purpose='follow_up' AND status='reserved' RETURNING generation_id) UPDATE messages SET follow_ups_status='error',follow_ups_expires_at=NULL WHERE id IN (SELECT generation_id FROM preempted) AND follow_ups_status='pending'", charge_estimate("NULL")))
        .bind(owner).execute(&mut **tx).await?;
    reserve_kind_in(tx, owner, generation, max_tokens, context_bytes, false).await
}
/// Automatic suggestions are real, bounded requests charged to the same limits.
/// The caller holds the enabled owner's users row lock, as for primary turns.
pub async fn reserve_follow_up_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    generation: Uuid,
    max_tokens: i32,
    context_bytes: usize,
) -> Result<(), ApiError> {
    if !(1..=256).contains(&max_tokens) {
        return Err(ApiError::bad(
            "Follow-up output must be between 1 and 256 tokens",
        ));
    }
    reserve_kind_in(tx, owner, generation, max_tokens, context_bytes, true).await
}
async fn reserve_kind_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    generation: Uuid,
    max_tokens: i32,
    context_bytes: usize,
    follow_up: bool,
) -> Result<(), ApiError> {
    if !(1..=32768).contains(&max_tokens) {
        return Err(ApiError::bad("Maximum tokens must be between 1 and 32,768"));
    }
    let context_bytes =
        i64::try_from(context_bytes).map_err(|_| ApiError::bad("Context size is too large"))?;
    recover_in(tx, owner).await?;
    let limits = limits_in(tx, owner).await?;
    let active:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM conversations WHERE owner_id=$1 AND active_generation IS NOT NULL AND generation_expires>NOW()) + (SELECT COUNT(*) FROM generation_usage WHERE user_id=$1 AND status='reserved' AND purpose='follow_up' AND reservation_expires>NOW())").bind(owner).fetch_one(&mut **tx).await?;
    if active >= limits.concurrent_requests {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS,format!("Your account allows {} responses at once. Stop or wait for a response before starting another.",limits.concurrent_requests)));
    }
    let (requests,tokens,reserved):(i64,i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(COALESCE(total_tokens,charged_tokens)),0)::bigint,COALESCE(SUM(CASE WHEN status='reserved' AND ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose='follow_up' AND g.reservation_expires>NOW())) THEN reserved_output_tokens ELSE 0 END),0)::bigint FROM generation_usage g WHERE user_id=$1 AND created_at >= (date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')").bind(owner).fetch_one(&mut **tx).await?;
    if limits.daily_requests.is_some_and(|limit| requests >= limit) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Your daily Chat request limit has been reached. It resets at 00:00 UTC.".into(),
        ));
    }
    if limits.daily_tokens.is_some_and(|limit| {
        tokens
            .saturating_add(reserved)
            .saturating_add(i64::from(max_tokens))
            > limit
    }) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS,"Your daily token allowance cannot reserve this output budget. Lower Maximum output tokens or wait until 00:00 UTC.".into()));
    }
    sqlx::query("INSERT INTO generation_usage(generation_id,user_id,reserved_output_tokens,context_bytes,purpose,reservation_expires)VALUES($1,$2,$3,$4,$5,CASE WHEN $6 THEN NOW()+make_interval(secs=>$7) ELSE NULL END)").bind(generation).bind(owner).bind(i64::from(max_tokens)).bind(context_bytes).bind(if follow_up { "follow_up" } else { "chat" }).bind(follow_up).bind(FOLLOW_UP_LEASE_SECONDS as f64).execute(&mut **tx).await?;
    Ok(())
}
/// Preserve the accepted request count but release abandoned token reservations.
/// Safe both from lease recovery and before admitting the next turn. Abandoned
/// turns are charged an estimate from their last checkpointed output.
pub async fn recover_in(tx: &mut Transaction<'_, Postgres>, owner: Uuid) -> Result<(), ApiError> {
    let charge = charge_estimate("SELECT OCTET_LENGTH(m.content)+OCTET_LENGTH(m.reasoning_content) FROM messages m WHERE m.generation_id=g.generation_id");
    sqlx::query(&format!("UPDATE generation_usage g SET status='interrupted',finished_at=NOW(),usage_complete=FALSE,charged_tokens={charge} WHERE user_id=$1 AND status='reserved' AND NOT ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose='follow_up' AND g.reservation_expires>NOW()))")).bind(owner).execute(&mut **tx).await?;
    Ok(())
}
/// Settle a turn whose streamed output size is unknown; without reported usage,
/// admission charges its whole output reservation.
pub async fn finish_in(
    tx: &mut Transaction<'_, Postgres>,
    generation: Uuid,
    usage: Option<&Value>,
    status: &str,
) -> Result<(), ApiError> {
    finish_with_output_in(tx, generation, usage, status, None).await
}
/// `output_bytes` is the saved answer plus reasoning, used only when the
/// provider did not report a total.
pub async fn finish_with_output_in(
    tx: &mut Transaction<'_, Postgres>,
    generation: Uuid,
    usage: Option<&Value>,
    status: &str,
    output_bytes: Option<usize>,
) -> Result<(), ApiError> {
    if !matches!(status, "completed" | "interrupted" | "error") {
        return Err(ApiError::bad("Invalid generation outcome"));
    }
    let counter = |key| {
        usage
            .and_then(|u| u.get(key))
            .and_then(Value::as_i64)
            .filter(|n| (0..=1_000_000_000).contains(n))
    };
    let prompt = counter("prompt_tokens");
    let completion = counter("completion_tokens");
    let reported = counter("total_tokens");
    let sum = prompt.zip(completion).map(|(a, b)| a + b);
    let total = match (reported, sum) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let complete =
        prompt.is_some() && completion.is_some() && reported.is_some() && reported == sum;
    let output_bytes = output_bytes.and_then(|n| i64::try_from(n).ok());
    let charge = charge_estimate("$7::bigint");
    sqlx::query(&format!("UPDATE generation_usage SET status=CASE WHEN status='interrupted' THEN status ELSE $2 END,prompt_tokens=$3,completion_tokens=$4,total_tokens=$5,usage_complete=$6,charged_tokens=CASE WHEN $5::bigint IS NULL THEN {charge} END,finished_at=NOW() WHERE generation_id=$1 AND (status='reserved' OR (purpose='follow_up' AND status='interrupted' AND NOT usage_complete))")).bind(generation).bind(status).bind(prompt).bind(completion).bind(total).bind(complete).bind(output_bytes).execute(&mut **tx).await?;
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Counts {
    requests: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
    errors: i64,
    active_generations: i64,
    incomplete_usage: i64,
    reserved_tokens: i64,
}
impl Counts {
    fn add(&mut self, other: &Self) {
        self.requests += other.requests;
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
        self.errors += other.errors;
        self.active_generations += other.active_generations;
        self.incomplete_usage += other.incomplete_usage;
        self.reserved_tokens += other.reserved_tokens;
    }
}
// Shared expressions keep daily, personal, and administrative totals consistent.
// The active-generation lookup uses the partial index in migration 007.
const AGGREGATES: &str = "
    COUNT(*) AS requests,
    COALESCE(SUM(prompt_tokens),0)::bigint AS prompt_tokens,
    COALESCE(SUM(completion_tokens),0)::bigint AS completion_tokens,
    COALESCE(SUM(total_tokens),0)::bigint AS total_tokens,
    COUNT(*) FILTER(WHERE status IN ('error','interrupted') OR
        (status='reserved' AND NOT ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose='follow_up' AND g.reservation_expires>NOW())))) AS errors,
    COUNT(*) FILTER(WHERE status='reserved' AND ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose='follow_up' AND g.reservation_expires>NOW()))) AS active_generations,
    COUNT(*) FILTER(WHERE NOT usage_complete AND (status<>'reserved' OR
        NOT ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose='follow_up' AND g.reservation_expires>NOW())))) AS incomplete_usage,
    COALESCE(SUM(CASE WHEN status='reserved' AND ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose='follow_up' AND g.reservation_expires>NOW()))
        THEN reserved_output_tokens ELSE 0 END),0)::bigint AS reserved_tokens";
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Daily {
    date: String,
    #[serde(flatten)]
    #[sqlx(flatten)]
    counts: Counts,
}
async fn counts(pool: &PgPool, owner: Uuid, days: i32) -> Result<Counts, ApiError> {
    Ok(sqlx::query_as::<_,Counts>(&format!("SELECT {AGGREGATES} FROM generation_usage g WHERE user_id=$1 AND created_at >= (date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') - make_interval(days => $2-1)")).bind(owner).bind(days).fetch_one(pool).await?)
}
async fn daily(pool: &PgPool, owner: Uuid) -> Result<Vec<Daily>, ApiError> {
    Ok(sqlx::query_as::<_,Daily>(&format!("SELECT to_char(g.created_at AT TIME ZONE 'UTC','YYYY-MM-DD') AS date,{AGGREGATES} FROM generation_usage g WHERE user_id=$1 AND created_at >= (date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')-INTERVAL '29 days' GROUP BY date ORDER BY date")).bind(owner).fetch_all(pool).await?)
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/usage", get(own_usage))
        .route("/api/admin/usage", get(admin_usage))
        .route("/api/admin/limits", get(get_limits).patch(save_limits))
        .route(
            "/api/admin/limits/{id}",
            put(save_override).delete(delete_override),
        )
        .route("/api/admin/health", get(health))
        .with_state(state)
}
async fn own_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, false).await?;
    let (today, totals, daily, limits) = tokio::try_join!(
        counts(&state.store.pool, user.id, 1),
        counts(&state.store.pool, user.id, 30),
        daily(&state.store.pool, user.id),
        effective_limits(&state.store.pool, user.id)
    )?;
    Ok(Json(
        json!({"periodDays":30,"timezone":"UTC","quotaBasis":QUOTA_BASIS,"today":today,"totals":totals,"daily":daily,"limits":limits}),
    ))
}
#[derive(Deserialize)]
struct UserQuery {
    q: Option<String>,
    limit: Option<i64>,
}
#[derive(FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageUser {
    id: Uuid,
    username: String,
    display_name: String,
    role: Role,
    enabled: bool,
}
#[derive(FromRow)]
struct UserCounts {
    user_id: Uuid,
    today: bool,
    #[sqlx(flatten)]
    counts: Counts,
}
async fn admin_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<Value>, ApiError> {
    state.auth.require_admin(&headers, false).await?;
    let search = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(100);
    if search.len() > 256 || !(1..=1000).contains(&limit) {
        return Err(ApiError::bad("Invalid usage search or limit"));
    }
    let mut users=sqlx::query_as::<_,UsageUser>("SELECT id,username,display_name,role,enabled FROM users WHERE $1='' OR strpos(lower(username),lower($1))>0 OR strpos(lower(display_name),lower($1))>0 OR id::text=$1 ORDER BY username,id LIMIT $2").bind(search.trim()).bind(limit+1).fetch_all(&state.store.pool).await?;
    let has_more = users.len() > limit as usize;
    users.truncate(limit as usize);
    // Batch all selected accounts instead of issuing several queries per user.
    let ids: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    let aggregate_sql = format!("SELECT user_id,created_at >= (date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS today,{AGGREGATES} FROM generation_usage g WHERE user_id=ANY($1) AND created_at >= (date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')-INTERVAL '29 days' GROUP BY user_id,today");
    let (aggregates, overrides, defaults) = tokio::try_join!(
        sqlx::query_as::<_, UserCounts>(&aggregate_sql)
            .bind(&ids)
            .fetch_all(&state.store.pool),
        sqlx::query_as::<_, (Uuid, Value)>(
            "SELECT user_id,limits FROM user_limit_overrides WHERE user_id=ANY($1)"
        )
        .bind(&ids)
        .fetch_all(&state.store.pool),
        sqlx::query_scalar::<_, Value>("SELECT limits FROM site_limits WHERE id=TRUE")
            .fetch_one(&state.store.pool)
    )?;
    let mut by_user: HashMap<Uuid, (Counts, Counts)> = HashMap::new();
    for aggregate in aggregates {
        let (today, totals) = by_user.entry(aggregate.user_id).or_default();
        totals.add(&aggregate.counts);
        if aggregate.today {
            *today = aggregate.counts;
        }
    }
    let overrides: HashMap<Uuid, Value> = overrides.into_iter().collect();
    let users: Vec<Value> = users
        .into_iter()
        .map(|user| {
            let (today, totals) = by_user.remove(&user.id).unwrap_or_default();
            let limits = resolved(
                defaults.clone(),
                overrides
                    .get(&user.id)
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )?;
            let mut value = json!(user);
            value["today"] = json!(today);
            value["totals"] = json!(totals);
            value["limits"] = json!(limits);
            Ok(value)
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(Json(
        json!({"users":users,"hasMore":has_more,"periodDays":30,"timezone":"UTC","quotaBasis":QUOTA_BASIS}),
    ))
}
async fn limit_document(pool: &PgPool) -> Result<Value, ApiError> {
    let defaults: Value = sqlx::query_scalar("SELECT limits FROM site_limits WHERE id=TRUE")
        .fetch_one(pool)
        .await?;
    let overrides: Vec<(Uuid, Value)> =
        sqlx::query_as("SELECT user_id,limits FROM user_limit_overrides ORDER BY user_id")
            .fetch_all(pool)
            .await?;
    Ok(
        json!({"limits":resolved(defaults,json!({}))?,"overrides":overrides.into_iter().map(|(id,limits)|json!({"userId":id,"limits":limits})).collect::<Vec<_>>(),"timezone":"UTC","quotaBasis":QUOTA_BASIS}),
    )
}
async fn get_limits(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.auth.require_admin(&headers, false).await?;
    Ok(Json(limit_document(&state.store.pool).await?))
}
async fn admin_transaction(
    pool: &PgPool,
    admin: Uuid,
) -> Result<Transaction<'_, Postgres>, ApiError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(7346281951102)")
        .execute(&mut *tx)
        .await?;
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND enabled AND role='admin' AND NOT password_change_required)",
    )
    .bind(admin)
    .fetch_one(&mut *tx)
    .await?;
    if !valid {
        return Err(ApiError::forbidden());
    }
    Ok(tx)
}
async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    admin: Uuid,
    target: Option<Uuid>,
    limits: Value,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,target_id,metadata)VALUES($1,$2,'limits.update',$3,$4,$5)").bind(Uuid::new_v4()).bind(admin).bind(if target.is_some(){"user"}else{"site"}).bind(target.map(|id|id.to_string())).bind(limits).execute(&mut **tx).await?;
    Ok(())
}
async fn save_limits(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require_admin(&headers, true).await?;
    validate_patch(&body)?;
    let mut tx = admin_transaction(&state.store.pool, user.id).await?;
    let mut defaults: Value =
        sqlx::query_scalar("SELECT limits FROM site_limits WHERE id=TRUE FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
    merge(&mut defaults, &body);
    sqlx::query("UPDATE site_limits SET limits=$1,updated_at=NOW() WHERE id=TRUE")
        .bind(&defaults)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, user.id, None, defaults).await?;
    tx.commit().await?;
    Ok(Json(limit_document(&state.store.pool).await?))
}
async fn save_override(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require_admin(&headers, true).await?;
    validate_patch(&body)?;
    let mut tx = admin_transaction(&state.store.pool, user.id).await?;
    if sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found());
    }
    sqlx::query("INSERT INTO user_limit_overrides(user_id,limits)VALUES($1,$2)ON CONFLICT(user_id)DO UPDATE SET limits=EXCLUDED.limits,updated_at=NOW()").bind(id).bind(&body).execute(&mut *tx).await?;
    audit(&mut tx, user.id, Some(id), body).await?;
    tx.commit().await?;
    Ok(Json(limit_document(&state.store.pool).await?))
}
async fn delete_override(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = state.auth.require_admin(&headers, true).await?;
    let mut tx = admin_transaction(&state.store.pool, user.id).await?;
    if sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found());
    }
    sqlx::query("DELETE FROM user_limit_overrides WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, user.id, Some(id), json!({"inherit":true})).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn health(
    State(state): State<AppState>,
    extensions: Extensions,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.auth.require_admin(&headers, false).await?;
    let client = state.auth.client_report(&extensions, &headers);
    let db = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&state.store.pool),
    )
    .await
    .is_ok_and(|r| r.is_ok());
    Ok(Json(
        json!({"application":"obleth-chat","version":env!("CARGO_PKG_VERSION"),"database":{"ready":db,"poolSize":state.store.pool.size(),"idleConnections":state.store.pool.num_idle()},"gateway":{"configured":state.gateway.is_some()},"oidc":{"configured":state.auth.oidc_enabled()},"client":client,"streams":{"activeOnReplica":state.metrics.streams.get(),"availableOnReplica":state.slots.available_permits()},"shuttingDown":state.shutdown.is_cancelled(),"metricsPath":"/metrics","readinessPath":"/readyz","scope":"Current application replica; gateway readiness is checked when discovering models."}),
    ))
}
