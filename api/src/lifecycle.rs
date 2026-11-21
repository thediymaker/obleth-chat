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
    state.store.recover_expired(user.id).await?;
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
    if active.is_some() {
        state.wakeups.send(&state.store.pool, &stop_key(id)).await;
    }
    Ok(Json(
        json!({"conversation":state.store.get(user.id,id).await?}),
    ))
}

/// The wake-up key Stop sends for a chat's running reply, on every replica.
pub fn stop_key(conversation: Uuid) -> String {
    format!("stop:{conversation}")
}

/// Workers renew their lease this often, and otherwise wait for Stop's wake-up, which
/// makes them check at once; renewals rewrite the conversation row and also notice a
/// Stop whose wake-up was missed.
pub const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(10);
/// Workers tolerate database errors until their last renewal is this old, which
/// still leaves time to save output before the 45-second lease expires.
pub const LEASE_GRACE: Duration = Duration::from_secs(30);

/// How a turn ended without finishing. The reply stores and shows `message()`; its
/// status comes from the variant, so rewording a message changes no behaviour.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnEnd {
    /// The person pressed Stop.
    Stopped,
    /// Another reply took the chat over, or the lease ran out.
    Superseded,
    ShuttingDown,
    /// The sign-in that started the turn ended.
    SessionEnded,
    /// The model sent nothing for STALL_SECS. A tool turn pauses instead.
    Stalled,
    Failed(String),
}

impl TurnEnd {
    pub fn message(&self) -> std::borrow::Cow<'_, str> {
        match self {
            TurnEnd::Stopped => "Response stopped".into(),
            TurnEnd::Superseded => "Response is no longer active".into(),
            TurnEnd::ShuttingDown => "Service shutting down".into(),
            TurnEnd::SessionEnded => "Sign-in session ended".into(),
            TurnEnd::Stalled => format!(
                "The model sent nothing for {} minutes",
                crate::STALL_SECS / 60
            )
            .into(),
            TurnEnd::Failed(message) => message.as_str().into(),
        }
    }

    /// The stored status: a turn something else ended is interrupted, one that went
    /// wrong is an error.
    pub fn status(&self) -> &'static str {
        match self {
            TurnEnd::Stopped
            | TurnEnd::Superseded
            | TurnEnd::ShuttingDown
            | TurnEnd::SessionEnded => "interrupted",
            TurnEnd::Stalled | TurnEnd::Failed(_) => "error",
        }
    }
}

impl From<String> for TurnEnd {
    fn from(message: String) -> Self {
        TurnEnd::Failed(message)
    }
}

impl From<&str> for TurnEnd {
    fn from(message: &str) -> Self {
        TurnEnd::Failed(message.to_owned())
    }
}

/// Renew only a currently valid lease. An expired worker cannot revive itself
/// and overwrite a replacement response. None means keep working.
pub async fn heartbeat(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    generation: Uuid,
) -> Result<Option<TurnEnd>, ApiError> {
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
) -> Result<Option<TurnEnd>, ApiError> {
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
) -> Result<TurnEnd, ApiError> {
    let cancelled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1 AND owner_id=$2 AND active_generation=$3 AND generation_cancel_requested)")
        .bind(id).bind(owner).bind(generation).fetch_one(pool).await?;
    Ok(if cancelled {
        TurnEnd::Stopped
    } else {
        TurnEnd::Superseded
    })
}

/// Long responses checkpoint less often so total checkpoint writes stay linear in
/// response size: the first 32 KiB save on every tick; after that, once the new text
/// is a quarter of what was saved, or after 3 seconds while under 128 KiB. A replica
/// that dies mid-reply so loses at most that quarter of a long reply's newest text.
pub fn checkpoint_due(total: usize, saved: usize, since_save: Duration) -> bool {
    let new = total.saturating_sub(saved);
    total != saved
        && (total <= 32 * 1024
            || new >= saved / 4
            || (total < 128 * 1024 && since_save >= Duration::from_secs(3)))
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
        .bind(id).bind(owner).bind(generation).bind(sqlx::types::Json(activity)).execute(pool).await?;
    Ok(())
}
pub async fn checkpoint(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
    generation: Uuid,
    content: &str,
    // None leaves the saved reasoning as it is: it has not grown since the last save.
    reasoning: Option<&str>,
    request_id: Option<&str>,
) -> Result<(), ApiError> {
    if content.len() + reasoning.map_or(0, str::len) > store::MAX_RESPONSE_BYTES {
        return Err(ApiError::bad("The response exceeded the storage limit"));
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT set_config('statement_timeout','5s',true), set_config('lock_timeout','500ms',true)",
    )
    .execute(&mut *tx)
    .await?;
    let changed=sqlx::query("UPDATE messages m SET content=$4,reasoning_content=COALESCE($5,m.reasoning_content),request_id=$6 FROM conversations c WHERE m.conversation_id=c.id AND c.id=$1 AND c.owner_id=$2 AND c.active_generation=$3 AND c.generation_expires>clock_timestamp() AND m.generation_id=$3 AND m.status='pending'")
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

#[cfg(test)]
mod tests {
    use super::TurnEnd;

    #[test]
    fn a_turn_end_s_status_follows_the_variant_not_its_words() {
        for end in [
            TurnEnd::Stopped,
            TurnEnd::Superseded,
            TurnEnd::ShuttingDown,
            TurnEnd::SessionEnded,
        ] {
            assert_eq!(end.status(), "interrupted", "{end:?}");
        }
        assert_eq!(TurnEnd::Stalled.status(), "error");
        // Any wording of a failure, even one that reads like a stop, stays an error.
        assert_eq!(TurnEnd::from("Response stopped").status(), "error");
        assert_eq!(
            TurnEnd::Stalled.message(),
            "The model sent nothing for 10 minutes"
        );
    }
}
