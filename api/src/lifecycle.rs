//! Background generation ownership and explicit cancellation across replicas.
//! Browser connections are subscribers; the generation lease owns the work.
use crate::{error::ApiError, store, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Stop {
    generation_id: Uuid,
}

pub async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Stop>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    // Recover an already-expired lease before comparing the requested generation.
    state.store.get(user.id, id).await?;
    let mut tx = state.store.pool.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let (active,): (Option<Uuid>,) = sqlx::query_as(
        "SELECT active_generation FROM conversations WHERE id=$1 AND owner_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    if let Some(active) = active {
        if active != body.generation_id {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "A different response is now running. Refresh before stopping it.".into(),
            ));
        }
        sqlx::query("UPDATE conversations SET generation_cancel_requested=TRUE WHERE id=$1 AND owner_id=$2 AND active_generation=$3")
            .bind(id).bind(user.id).bind(active).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"conversation":state.store.get(user.id,id).await?}),
    ))
}

/// Workers renew their lease this often and run the read-only check in between;
/// renewals rewrite the conversation row, and the 1 s poll only has to notice Stop.
pub const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(10);
/// Workers tolerate database errors until their last renewal is this old, which
/// still leaves time to save output before the 45-second lease expires.
pub const LEASE_GRACE: Duration = Duration::from_secs(30);

/// Renew only a currently valid lease. An expired worker cannot revive itself
/// and overwrite a replacement response. None means keep working.
pub async fn heartbeat(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    generation: Uuid,
) -> Result<Option<&'static str>, ApiError> {
    let updated = sqlx::query("UPDATE conversations c SET generation_expires=clock_timestamp()+make_interval(secs=>$4) WHERE id=$1 AND owner_id=$2 AND active_generation=$3 AND generation_expires>clock_timestamp() AND NOT generation_cancel_requested AND EXISTS(SELECT 1 FROM users u WHERE u.id=c.owner_id AND u.enabled)")
        .bind(id).bind(owner).bind(generation).bind(store::GENERATION_LEASE_SECONDS as f64).execute(pool).await?.rows_affected();
    if updated == 1 {
        return Ok(None);
    }
    ended(pool, owner, id, generation).await.map(Some)
}

/// Apply the heartbeat conditions without renewing the lease.
pub async fn check(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    generation: Uuid,
) -> Result<Option<&'static str>, ApiError> {
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations c WHERE id=$1 AND owner_id=$2 AND active_generation=$3 AND generation_expires>clock_timestamp() AND NOT generation_cancel_requested AND EXISTS(SELECT 1 FROM users u WHERE u.id=c.owner_id AND u.enabled))")
        .bind(id).bind(owner).bind(generation).fetch_one(pool).await?;
    if valid {
        return Ok(None);
    }
    ended(pool, owner, id, generation).await.map(Some)
}

async fn ended(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    generation: Uuid,
) -> Result<&'static str, ApiError> {
    let cancelled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1 AND owner_id=$2 AND active_generation=$3 AND generation_cancel_requested)")
        .bind(id).bind(owner).bind(generation).fetch_one(pool).await?;
    Ok(if cancelled {
        "Response stopped"
    } else {
        "Response is no longer active"
    })
}

/// Long responses checkpoint less often so total checkpoint writes stay near
/// linear in response size: the first 32 KiB save on every tick, then only
/// after 8 KiB of new text or 3 seconds.
pub fn checkpoint_due(total: usize, saved: usize, since_save: Duration) -> bool {
    total != saved
        && (total <= 32 * 1024
            || total.saturating_sub(saved) >= 8 * 1024
            || since_save >= Duration::from_secs(3))
}

/// Save partial output without holding a transaction while waiting for a model.
/// The conditional update is the lease check and takes no user-row lock, so a
/// long transaction elsewhere for the same user cannot stall a stream. CONFLICT
/// means the lease is gone; an internal error is transient and can be retried.
/// Saves workspace tool steps for a response that is still in progress.
pub async fn save_activity(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    generation: Uuid,
    activity: &[Value],
) -> Result<(), ApiError> {
    sqlx::query("UPDATE messages m SET activity=$4 FROM conversations c WHERE m.conversation_id=c.id AND c.id=$1 AND c.owner_id=$2 AND c.active_generation=$3 AND m.generation_id=$3 AND m.status='pending'")
        .bind(id).bind(owner).bind(generation).bind(Value::Array(activity.to_vec())).execute(pool).await?;
    Ok(())
}
pub async fn checkpoint(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    generation: Uuid,
    content: &str,
    reasoning: &str,
    request_id: Option<&str>,
) -> Result<(), ApiError> {
    if content.len() + reasoning.len() > store::MAX_RESPONSE_BYTES {
        return Err(ApiError::bad("The response exceeded the storage limit"));
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='500ms'")
        .execute(&mut *tx)
        .await?;
    let changed=sqlx::query("UPDATE messages m SET content=$4,reasoning_content=$5,request_id=$6 FROM conversations c WHERE m.conversation_id=c.id AND c.id=$1 AND c.owner_id=$2 AND c.active_generation=$3 AND c.generation_expires>clock_timestamp() AND m.generation_id=$3 AND m.status='pending'")
        .bind(id).bind(owner).bind(generation).bind(content).bind(reasoning).bind(request_id).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Response is no longer active".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}
