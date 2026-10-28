//! Optional retention, disabled until an administrator explicitly sets a policy.
//! Every cleanup is bounded, serialized across replicas, and uses the same owner
//! row locks as chat/file writes. An active writer is skipped rather than blocked.
use crate::{error::ApiError, store::Store, AppState};
use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const BATCH: i64 = 100;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Policy {
    conversation_days: Option<i32>,
    unused_file_days: Option<i32>,
    audit_days: Option<i32>,
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub at: DateTime<Utc>,
    pub conversations: u64,
    pub unused_files: u64,
    pub audit_events: u64,
    pub expired_sessions: u64,
    pub expired_access_tokens: u64,
}
fn validate(body: &Value) -> Result<(), ApiError> {
    let object = body
        .as_object()
        .ok_or_else(|| ApiError::bad("Retention policy must be an object"))?;
    for (key, value) in object {
        let minimum = match key.as_str() {
            "conversationDays" | "unusedFileDays" => 1,
            "auditDays" => 30,
            _ => return Err(ApiError::bad("Unknown retention setting")),
        };
        if !value.is_null()
            && !value
                .as_i64()
                .is_some_and(|days| (minimum..=3650).contains(&days))
        {
            return Err(ApiError::bad("Conversation/file retention must be null or 1–3,650 days; audit retention must be null or 30–3,650 days"));
        }
    }
    Ok(())
}
fn document(mut policy: Value, last_result: Option<Value>) -> Value {
    policy["enabled"] = json!(["conversationDays", "unusedFileDays", "auditDays"]
        .iter()
        .any(|key| !policy[key].is_null()));
    policy["lastRun"] = last_result.unwrap_or(Value::Null);
    policy
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/retention", get(read).patch(save))
        .with_state(state)
}
async fn read(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    state.auth.require_admin(&headers, false).await?;
    let (policy, last_result): (Value, Option<Value>) =
        sqlx::query_as("SELECT policy,last_result FROM retention_policy WHERE id=TRUE")
            .fetch_one(&state.store.pool)
            .await?;
    Ok(Json(document(policy, last_result)))
}
async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require_admin(&headers, true).await?;
    validate(&body)?;
    let mut tx = state.store.pool.begin().await?;
    // Same lock as role/account administration: a concurrent demotion cannot
    // authorize a new destructive policy after it has committed.
    sqlx::query("SELECT pg_advisory_xact_lock(7346281951102)")
        .execute(&mut *tx)
        .await?;
    let admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND enabled AND role='admin')",
    )
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if !admin {
        return Err(ApiError::forbidden());
    }
    let (mut policy, last_result): (Value, Option<Value>) =
        sqlx::query_as("SELECT policy,last_result FROM retention_policy WHERE id=TRUE FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
    for (key, value) in body.as_object().unwrap() {
        policy[key] = value.clone();
    }
    sqlx::query("UPDATE retention_policy SET policy=$1,updated_at=NOW() WHERE id=TRUE")
        .bind(&policy)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,metadata)VALUES($1,$2,'retention.update','site',$3)").bind(Uuid::new_v4()).bind(user.id).bind(&policy).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(document(policy, last_result)))
}

/// One batch at most per hour across all replicas. None means another replica
/// owns the job, or a completed run is still fresh. No user content is removed
/// with the default null policy; expired authentication artifacts always expire.
pub async fn run_once(pool: &PgPool) -> Result<Option<RunReport>, ApiError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *tx)
        .await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(7346281951103)")
        .fetch_one(&mut *tx)
        .await?;
    if !acquired {
        return Ok(None);
    }
    let Some(raw)=sqlx::query_scalar::<_,Value>("SELECT policy FROM retention_policy WHERE id=TRUE AND (last_run IS NULL OR last_run<=NOW()-INTERVAL '1 hour') FOR UPDATE").fetch_optional(&mut *tx).await? else { return Ok(None); };
    validate(&raw).map_err(|_| ApiError::internal())?;
    let policy: Policy = serde_json::from_value(raw).map_err(|_| ApiError::internal())?;
    let mut report = RunReport::default();
    // Sorting owner locks matches ordinary chat/file operations and prevents
    // deleting files while an upload/reference transaction is still in progress.
    let owners=sqlx::query_scalar::<_,Uuid>("SELECT u.id FROM users u WHERE
      ($1::int IS NOT NULL AND EXISTS(SELECT 1 FROM conversations c WHERE c.owner_id=u.id AND (c.active_generation IS NULL OR c.generation_expires<=NOW()) AND c.updated_at<NOW()-make_interval(days=>$1))) OR
      ($2::int IS NOT NULL AND EXISTS(SELECT 1 FROM attachments a WHERE a.owner_id=u.id AND a.created_at<NOW()-make_interval(days=>$2) AND NOT EXISTS(SELECT 1 FROM message_attachments ma WHERE ma.attachment_id=a.id)))
      ORDER BY u.id LIMIT 100 FOR UPDATE OF u SKIP LOCKED")
        .bind(policy.conversation_days).bind(policy.unused_file_days).fetch_all(&mut *tx).await?;
    for owner in owners {
        if let Some(days) = policy.conversation_days {
            report.conversations+=sqlx::query("DELETE FROM conversations WHERE id IN(SELECT id FROM conversations WHERE owner_id=$1 AND (active_generation IS NULL OR generation_expires<=NOW()) AND updated_at<NOW()-make_interval(days=>$2) ORDER BY updated_at,id LIMIT $3 FOR UPDATE SKIP LOCKED)")
                .bind(owner).bind(days).bind(BATCH-report.conversations as i64).execute(&mut *tx).await?.rows_affected();
        }
        if let Some(days) = policy.unused_file_days {
            report.unused_files+=sqlx::query("DELETE FROM attachments WHERE id IN(SELECT a.id FROM attachments a WHERE a.owner_id=$1 AND a.created_at<NOW()-make_interval(days=>$2) AND NOT EXISTS(SELECT 1 FROM message_attachments ma WHERE ma.attachment_id=a.id) ORDER BY a.created_at,a.id LIMIT $3 FOR UPDATE OF a SKIP LOCKED)")
                .bind(owner).bind(days).bind(BATCH-report.unused_files as i64).execute(&mut *tx).await?.rows_affected();
        }
    }
    if let Some(days) = policy.audit_days {
        report.audit_events=sqlx::query("DELETE FROM audit_events WHERE id IN(SELECT id FROM audit_events WHERE created_at<NOW()-make_interval(days=>$1) ORDER BY created_at,id LIMIT 100 FOR UPDATE SKIP LOCKED)")
            .bind(days).execute(&mut *tx).await?.rows_affected();
    }
    report.expired_sessions=sqlx::query("DELETE FROM auth_sessions WHERE token_hash IN(SELECT token_hash FROM auth_sessions WHERE expires_at<NOW() ORDER BY expires_at LIMIT 500 FOR UPDATE SKIP LOCKED)").execute(&mut *tx).await?.rows_affected();
    report.expired_access_tokens=sqlx::query("DELETE FROM account_access_tokens WHERE token_hash IN(SELECT token_hash FROM account_access_tokens WHERE expires_at<NOW() ORDER BY expires_at LIMIT 500 FOR UPDATE SKIP LOCKED)").execute(&mut *tx).await?.rows_affected();
    report.at = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *tx)
        .await?;
    let result = json!(report);
    sqlx::query("UPDATE retention_policy SET last_run=$1,last_result=$2 WHERE id=TRUE")
        .bind(report.at)
        .bind(&result)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO audit_events(id,action,target_type,metadata)VALUES($1,'retention.cleanup','site',$2)").bind(Uuid::new_v4()).bind(result).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(report))
}
/// Deletes temporary chats that were left or went quiet, every minute; see
/// Store::sweep_temporary. Replicas may sweep at once: each deletion re-checks under
/// the owner's lock.
pub async fn temporary_worker(store: Store, shutdown: CancellationToken) {
    let mut timer = tokio::time::interval(Duration::from_secs(60));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { _=shutdown.cancelled()=>break, _=timer.tick()=>{} }
        tokio::select! {
            _=shutdown.cancelled()=>break,
            result=store.sweep_temporary()=>if result.is_err() { tracing::warn!("Temporary chat cleanup failed; it will retry in a minute"); }
        }
    }
}
pub async fn worker(pool: PgPool, shutdown: CancellationToken) {
    let mut timer = tokio::time::interval(Duration::from_secs(3600));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { _=shutdown.cancelled()=>break, _=timer.tick()=>{} }
        tokio::select! {
            _=shutdown.cancelled()=>break,
            result=run_once(&pool)=>if result.is_err() { tracing::warn!("Retention cleanup failed; it will retry at the next hourly run"); }
        }
    }
}
