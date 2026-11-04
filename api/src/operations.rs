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
/// Output held back against the daily allowance for a reply with no output limit.
pub const UNCAPPED_RESERVATION: i32 = 8_192;
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
    model: &str,
    max_tokens: i32,
    context_bytes: usize,
) -> Result<(), ApiError> {
    // Interactive turns take priority over this owner's optional housekeeping.
    // The worker observes this revocation and closes its upstream within 1s.
    // Keep the admitted request in the ledger, and accept any actual usage later.
    sqlx::query(&format!("WITH preempted AS (UPDATE generation_usage SET status='interrupted',finished_at=NOW(),usage_complete=FALSE,charged_tokens={} WHERE user_id=$1 AND purpose='follow_up' AND status='reserved' RETURNING generation_id) UPDATE messages SET follow_ups_status='error',follow_ups_expires_at=NULL WHERE id IN (SELECT generation_id FROM preempted) AND follow_ups_status='pending'", charge_estimate("NULL")))
        .bind(owner).execute(&mut **tx).await?;
    reserve_kind_in(
        tx,
        owner,
        generation,
        model,
        max_tokens,
        context_bytes,
        None,
    )
    .await
}
/// Automatic suggestions are real, bounded requests charged to the same limits.
/// The caller holds the enabled owner's users row lock, as for primary turns.
pub async fn reserve_follow_up_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    generation: Uuid,
    model: &str,
    max_tokens: i32,
    context_bytes: usize,
) -> Result<(), ApiError> {
    if !(1..=256).contains(&max_tokens) {
        return Err(ApiError::bad(
            "Follow-up output must be between 1 and 256 tokens",
        ));
    }
    reserve_kind_in(
        tx,
        owner,
        generation,
        model,
        max_tokens,
        context_bytes,
        Some(("follow_up", FOLLOW_UP_LEASE_SECONDS)),
    )
    .await
}
/// A worker agent's requests, on one row leased for `lease_seconds`. The caller
/// holds the enabled owner's users row lock. Workers run inside their turn, so they
/// count toward the daily limits but not the account's responses at once.
pub async fn reserve_agent_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    generation: Uuid,
    model: &str,
    max_tokens: i32,
    context_bytes: usize,
    lease_seconds: i64,
) -> Result<(), ApiError> {
    reserve_kind_in(
        tx,
        owner,
        generation,
        model,
        max_tokens,
        context_bytes,
        Some(("agent", lease_seconds)),
    )
    .await
}
async fn reserve_kind_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    generation: Uuid,
    model: &str,
    max_tokens: i32,
    context_bytes: usize,
    leased: Option<(&str, i64)>,
) -> Result<(), ApiError> {
    if !(0..=crate::models::MAX_OUTPUT_TOKENS).contains(&max_tokens) {
        return Err(ApiError::bad(
            "The output limit must be 0 (the model's own) or up to 262,144 tokens",
        ));
    }
    // What the daily allowance holds back until the reply settles on its real usage:
    // its limit, or an estimate when it has none.
    let max_tokens = if max_tokens == 0 {
        UNCAPPED_RESERVATION
    } else {
        max_tokens.min(32_768)
    };
    let context_bytes =
        i64::try_from(context_bytes).map_err(|_| ApiError::bad("Context size is too large"))?;
    recover_in(tx, owner).await?;
    let limits = limits_in(tx, owner).await?;
    let active:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM conversations WHERE owner_id=$1 AND active_generation IS NOT NULL AND generation_expires>NOW()) + (SELECT COUNT(*) FROM generation_usage WHERE user_id=$1 AND status='reserved' AND purpose='follow_up' AND reservation_expires>NOW())").bind(owner).fetch_one(&mut **tx).await?;
    if active >= limits.concurrent_requests {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS,format!("Your account allows {} responses at once. Stop or wait for a response before starting another.",limits.concurrent_requests)));
    }
    let (requests,tokens,reserved):(i64,i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(COALESCE(total_tokens,charged_tokens)),0)::bigint,COALESCE(SUM(CASE WHEN status='reserved' AND ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose IN ('follow_up','agent') AND g.reservation_expires>NOW())) THEN reserved_output_tokens ELSE 0 END),0)::bigint FROM generation_usage g WHERE user_id=$1 AND created_at >= (date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')").bind(owner).fetch_one(&mut **tx).await?;
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
    sqlx::query("INSERT INTO generation_usage(generation_id,user_id,reserved_output_tokens,context_bytes,purpose,reservation_expires,model)VALUES($1,$2,$3,$4,$5,CASE WHEN $6 THEN NOW()+make_interval(secs=>$7) ELSE NULL END,$8)").bind(generation).bind(owner).bind(i64::from(max_tokens)).bind(context_bytes).bind(leased.map_or("chat", |(purpose, _)| purpose)).bind(leased.is_some()).bind(leased.map_or(0, |(_, secs)| secs) as f64).bind(model).execute(&mut **tx).await?;
    Ok(())
}
/// Preserve the accepted request count but release abandoned token reservations.
/// Safe both from lease recovery and before admitting the next turn. Abandoned
/// turns are charged an estimate from their last checkpointed output.
pub async fn recover_in(tx: &mut Transaction<'_, Postgres>, owner: Uuid) -> Result<(), ApiError> {
    let charge = charge_estimate("SELECT OCTET_LENGTH(m.content)+OCTET_LENGTH(m.reasoning_content) FROM messages m WHERE m.generation_id=g.generation_id");
    sqlx::query(&format!("UPDATE generation_usage g SET status='interrupted',finished_at=NOW(),usage_complete=FALSE,charged_tokens={charge} WHERE user_id=$1 AND status='reserved' AND NOT ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose IN ('follow_up','agent') AND g.reservation_expires>NOW()))")).bind(owner).execute(&mut **tx).await?;
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
        (status='reserved' AND NOT ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose IN ('follow_up','agent') AND g.reservation_expires>NOW())))) AS errors,
    COUNT(*) FILTER(WHERE status='reserved' AND ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose IN ('follow_up','agent') AND g.reservation_expires>NOW()))) AS active_generations,
    COUNT(*) FILTER(WHERE NOT usage_complete AND (status<>'reserved' OR
        NOT ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose IN ('follow_up','agent') AND g.reservation_expires>NOW())))) AS incomplete_usage,
    COALESCE(SUM(CASE WHEN status='reserved' AND ((g.purpose='chat' AND EXISTS(SELECT 1 FROM conversations c WHERE c.active_generation=g.generation_id AND c.generation_expires>NOW())) OR (g.purpose IN ('follow_up','agent') AND g.reservation_expires>NOW()))
        THEN reserved_output_tokens ELSE 0 END),0)::bigint AS reserved_tokens";
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Daily {
    date: String,
    #[serde(flatten)]
    #[sqlx(flatten)]
    counts: Counts,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct ByModel {
    /// None for records from before models were recorded, whose chat is gone.
    model: Option<String>,
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
/// The last 30 days per model, most tokens first.
async fn by_model(pool: &PgPool, owner: Uuid) -> Result<Vec<ByModel>, ApiError> {
    Ok(sqlx::query_as::<_,ByModel>(&format!("SELECT g.model,{AGGREGATES} FROM generation_usage g WHERE user_id=$1 AND created_at >= (date_trunc('day',NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')-INTERVAL '29 days' GROUP BY g.model ORDER BY COALESCE(SUM(g.total_tokens),0) DESC LIMIT 20")).bind(owner).fetch_all(pool).await?)
}
/// When this replica started, for the About card.
static STARTED: std::sync::LazyLock<chrono::DateTime<chrono::Utc>> =
    std::sync::LazyLock::new(chrono::Utc::now);
pub fn router(state: AppState) -> Router {
    std::sync::LazyLock::force(&STARTED);
    Router::new()
        .route("/api/usage", get(own_usage))
        .route("/api/admin/usage", get(admin_usage))
        .route("/api/admin/usage/overview", get(admin_overview))
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
    let (today, totals, daily, models, limits) = tokio::try_join!(
        counts(&state.store.pool, user.id, 1),
        counts(&state.store.pool, user.id, 30),
        daily(&state.store.pool, user.id),
        by_model(&state.store.pool, user.id),
        effective_limits(&state.store.pool, user.id)
    )?;
    Ok(Json(
        json!({"periodDays":30,"timezone":"UTC","quotaBasis":QUOTA_BASIS,"today":today,"totals":totals,"daily":daily,"byModel":models,"limits":limits}),
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverviewQuery {
    days: Option<i32>,
    group: Option<Uuid>,
    model: Option<String>,
    q: Option<String>,
}
#[derive(Serialize, FromRow, Default)]
#[serde(rename_all = "camelCase")]
struct Totals {
    #[serde(flatten)]
    #[sqlx(flatten)]
    counts: Counts,
    people: i64,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    /// A UTC day (YYYY-MM-DD), or an hour (YYYY-MM-DDTHH) for one day.
    at: String,
    #[serde(flatten)]
    #[sqlx(flatten)]
    totals: Totals,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Share {
    /// A model, or what a request was for: chat, follow_up or agent.
    name: Option<String>,
    #[serde(flatten)]
    #[sqlx(flatten)]
    totals: Totals,
}
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Heavy {
    id: Uuid,
    username: String,
    display_name: String,
    enabled: bool,
    last_active: chrono::DateTime<chrono::Utc>,
    #[serde(flatten)]
    #[sqlx(flatten)]
    counts: Counts,
}
/// Usage in a period ($1 start, $2 end or none), for a group ($3) and a model ($4).
const SCOPE: &str = "g.created_at >= $1 AND ($2::timestamptz IS NULL OR g.created_at < $2)
    AND ($3::uuid IS NULL OR g.user_id IN (SELECT user_id FROM user_group_members WHERE group_id=$3))
    AND ($4::text IS NULL OR g.model=$4)";
const PERIODS: [i32; 4] = [1, 7, 28, 90];
const OVERVIEW_PEOPLE: i64 = 100;
/// Share of a daily allowance from which someone counts as close to it.
const NEAR_LIMIT: f64 = 0.8;
/// Usage for administrators over a period: totals against the period before, a day-by-day
/// (or, for today, hour-by-hour) series, models, what requests were for, the people who used
/// the most, who is near a daily limit, and, when insights are on, what the tokens went to.
async fn admin_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OverviewQuery>,
) -> Result<Json<Value>, ApiError> {
    state.auth.require_admin(&headers, false).await?;
    let days = query.days.unwrap_or(28);
    let model = query
        .model
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    let search = query.q.unwrap_or_default().trim().to_owned();
    if !PERIODS.contains(&days)
        || search.len() > 256
        || model.as_ref().is_some_and(|m| m.len() > 256)
    {
        return Err(ApiError::bad(
            "Choose 1, 7, 28 or 90 days, and a shorter model or search",
        ));
    }
    let pool = &state.store.pool;
    let groups: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id,name FROM user_groups ORDER BY name")
            .fetch_all(pool)
            .await?;
    if query
        .group
        .is_some_and(|g| !groups.iter().any(|(id, _)| *id == g))
    {
        return Err(ApiError::not_found());
    }
    let today = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc();
    let start = today - chrono::Duration::days(i64::from(days) - 1);
    let before = start - chrono::Duration::days(i64::from(days));
    let none: Option<chrono::DateTime<chrono::Utc>> = None;
    let bucket = if days == 1 {
        "YYYY-MM-DD\"T\"HH24"
    } else {
        "YYYY-MM-DD"
    };
    let totals_sql = format!("SELECT {AGGREGATES},COUNT(DISTINCT g.user_id) AS people FROM generation_usage g WHERE {SCOPE}");
    let series_sql = format!("SELECT to_char(g.created_at AT TIME ZONE 'UTC','{bucket}') AS at,{AGGREGATES},COUNT(DISTINCT g.user_id) AS people FROM generation_usage g WHERE {SCOPE} GROUP BY at ORDER BY at");
    let share_sql = |column: &str| {
        format!("SELECT g.{column} AS name,{AGGREGATES},COUNT(DISTINCT g.user_id) AS people FROM generation_usage g WHERE {SCOPE} GROUP BY g.{column} ORDER BY COALESCE(SUM(g.total_tokens),0) DESC,COUNT(*) DESC LIMIT 20")
    };
    let people_sql = format!("SELECT u.id,u.username,u.display_name,u.enabled,MAX(g.created_at) AS last_active,{AGGREGATES} FROM generation_usage g JOIN users u ON u.id=g.user_id WHERE {SCOPE} AND ($5='' OR strpos(lower(u.username),lower($5))>0 OR strpos(lower(u.display_name),lower($5))>0) GROUP BY u.id ORDER BY COALESCE(SUM(g.total_tokens),0) DESC,COUNT(*) DESC,u.username LIMIT $6");
    let (models_sql, purposes_sql) = (share_sql("model"), share_sql("purpose"));
    let (totals, previous, series, models, purposes, mut people, choices) = tokio::try_join!(
        sqlx::query_as::<_, Totals>(&totals_sql).bind(start).bind(none).bind(query.group).bind(&model).fetch_one(pool),
        sqlx::query_as::<_, Totals>(&totals_sql).bind(before).bind(start).bind(query.group).bind(&model).fetch_one(pool),
        sqlx::query_as::<_, Bucket>(&series_sql).bind(start).bind(none).bind(query.group).bind(&model).fetch_all(pool),
        sqlx::query_as::<_, Share>(&models_sql).bind(start).bind(none).bind(query.group).bind(&model).fetch_all(pool),
        sqlx::query_as::<_, Share>(&purposes_sql).bind(start).bind(none).bind(query.group).bind(&model).fetch_all(pool),
        sqlx::query_as::<_, Heavy>(&people_sql).bind(start).bind(none).bind(query.group).bind(&model).bind(&search).bind(OVERVIEW_PEOPLE + 1).fetch_all(pool),
        sqlx::query_scalar::<_, String>("SELECT DISTINCT model FROM generation_usage WHERE model IS NOT NULL AND created_at >= NOW()-INTERVAL '90 days' ORDER BY model").fetch_all(pool)
    )?;
    let has_more = people.len() > OVERVIEW_PEOPLE as usize;
    people.truncate(OVERVIEW_PEOPLE as usize);
    // Today's use against each person's own allowance, for everyone in the group who used
    // Chat today, whatever the model filter.
    // Tokens as admission counts them: reported, or estimated when a provider did not say.
    let today_sql = format!("SELECT t.user_id,t.requests,t.charged,t.reserved_tokens FROM (SELECT g.user_id,COALESCE(SUM(COALESCE(g.total_tokens,g.charged_tokens)),0)::bigint AS charged,{AGGREGATES} FROM generation_usage g WHERE {SCOPE} GROUP BY g.user_id) t");
    let (used_today, overrides, defaults) = tokio::try_join!(
        sqlx::query_as::<_, (Uuid, i64, i64, i64)>(&today_sql)
            .bind(today)
            .bind(none)
            .bind(query.group)
            .bind(None::<String>)
            .fetch_all(pool),
        sqlx::query_as::<_, (Uuid, Value)>("SELECT user_id,limits FROM user_limit_overrides")
            .fetch_all(pool),
        sqlx::query_scalar::<_, Value>("SELECT limits FROM site_limits WHERE id=TRUE")
            .fetch_one(pool)
    )?;
    let overrides: HashMap<Uuid, Value> = overrides.into_iter().collect();
    let limits_of = |id: &Uuid| {
        resolved(
            defaults.clone(),
            overrides.get(id).cloned().unwrap_or_else(|| json!({})),
        )
    };
    let mut today_of: HashMap<Uuid, Value> = HashMap::new();
    let (mut at_limit, mut near_limit) = (Vec::new(), Vec::new());
    for (id, requests, tokens, reserved) in used_today {
        let limits = limits_of(&id)?;
        let fraction = [
            limits.daily_requests.map(|l| requests as f64 / l as f64),
            limits
                .daily_tokens
                .map(|l| (tokens + reserved) as f64 / l as f64),
        ]
        .into_iter()
        .flatten()
        .fold(0.0, f64::max);
        if fraction >= 1.0 {
            at_limit.push(id);
        } else if fraction >= NEAR_LIMIT {
            near_limit.push(id);
        }
        today_of.insert(id, json!({"requests": requests, "totalTokens": tokens, "reservedTokens": reserved, "fraction": fraction}));
    }
    let people: Vec<Value> = people
        .into_iter()
        .map(|person| {
            let limits = limits_of(&person.id)?;
            let today = today_of.get(&person.id).cloned().unwrap_or_else(
                || json!({"requests": 0, "totalTokens": 0, "reservedTokens": 0, "fraction": 0.0}),
            );
            let mut value = json!(person);
            value["today"] = today;
            value["limits"] = json!(limits);
            value["override"] = json!(overrides.contains_key(&person.id));
            Ok(value)
        })
        .collect::<Result<_, ApiError>>()?;
    let named: Vec<(Uuid, String, String)> = sqlx::query_as("SELECT id,username,display_name FROM users WHERE id=ANY($1) ORDER BY display_name,username")
        .bind(at_limit.iter().chain(&near_limit).copied().collect::<Vec<_>>())
        .fetch_all(pool)
        .await?;
    let limited: Vec<Value> = named
        .into_iter()
        .map(|(id, username, display_name)| json!({"id": id, "username": username, "displayName": display_name, "atLimit": at_limit.contains(&id), "today": today_of.get(&id)}))
        .collect();
    let mix = crate::insights::usage_mix(pool, start, query.group, model.as_deref()).await?;
    Ok(Json(json!({
        "days": days, "group": query.group, "model": model, "timezone": "UTC", "quotaBasis": QUOTA_BASIS,
        "start": start, "groups": groups.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        "models": choices, "totals": totals, "previous": previous, "series": series, "byModel": models,
        "byPurpose": purposes, "people": people, "hasMore": has_more,
        "limits": resolved(defaults, json!({}))?, "overrides": overrides.len(),
        "atLimit": at_limit.len(), "nearLimit": near_limit.len(), "limited": limited, "mix": mix,
    })))
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
pub(crate) async fn admin_transaction(
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
    // The newest migration the database has, and the newest this build knows: during a
    // rollout an older replica can find the database ahead of it.
    let migration: Option<(i64, String)> = sqlx::query_as(
        "SELECT version, description FROM _sqlx_migrations WHERE success ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(&state.store.pool)
    .await
    .ok()
    .flatten();
    let about = json!({"version":env!("CARGO_PKG_VERSION"),"repository":env!("CARGO_PKG_REPOSITORY"),"migration":migration.map(|(version, description)| json!({"version":version,"description":description})),"buildMigration":crate::MIGRATOR.iter().map(|m| m.version).max(),"replica":std::env::var("HOSTNAME").ok(),"startedAt":*STARTED,"images":{"workspace":state.sandboxes.as_ref().map(|sb| sb.config().image.clone()),"browser":state.browsers.as_ref().map(|b| b.config().image.clone())}});
    Ok(Json(
        json!({"application":"obleth-chat","version":env!("CARGO_PKG_VERSION"),"about":about,"database":{"ready":db,"poolSize":state.store.pool.size(),"idleConnections":state.store.pool.num_idle()},"gateway":{"configured":state.gateway.is_some()},"oidc":{"configured":state.auth.oidc_enabled()},"client":client,"streams":{"activeOnReplica":state.metrics.streams.get(),"availableOnReplica":state.slots.available_permits()},"workspaces":{"enabled":state.sandboxes.is_some(),"namespace":state.sandboxes.as_ref().map(|sb| sb.config().namespace.clone()),"browser":state.browsers.is_some(),"off":state.workspaces_off},"shuttingDown":state.shutdown.is_cancelled(),"metricsPath":"/metrics","readinessPath":"/readyz","scope":"Current application replica; gateway readiness is checked when discovering models."}),
    ))
}
