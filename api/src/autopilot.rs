//! Auto mode's watchdog. A chat in Auto mode runs unattended, often overnight, so
//! when its turn stops without finishing (the pod restarted, the gateway failed,
//! the model stalled or reached a size limit) or ends while research runs are
//! still going, Chat sends the model a short note to carry on. Nudges back off,
//! and stop after MAX_NUDGES in a row that made no progress, until the person
//! writes again.

use std::time::Duration;

use serde_json::Value;
use sqlx::Row;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::auth::{Identity, Role};
use crate::{launch_turn, models, AppState, Launch};

/// Nudges in a row before Chat leaves the chat for the person.
pub const MAX_NUDGES: i32 = 10;
/// A turn with at least this many steps made progress, so the count starts over.
const PROGRESS_STEPS: i32 = 10;
/// Only chats active this recently are nudged, so an old failure stays put.
const RECENT_HOURS: i32 = 12;
const PAUSED: &str = "Say \"continue\" to pick up where it left off._";

/// What the watchdog tells the model, and why.
#[derive(Debug, PartialEq)]
pub enum Nudge {
    /// The reply stopped before finishing, with the reason it stopped.
    Stopped(String),
    /// The turn paused at a limit.
    Paused,
    /// The reply finished while research runs were still going.
    RunsGoing,
}

impl Nudge {
    pub fn message(&self) -> String {
        match self {
            Nudge::Stopped(reason) => format!("[Auto mode] Your last reply stopped before finishing ({}). Continue where you left off: check the workspace and research_status first rather than redoing work.", reason.trim_end_matches('.')),
            Nudge::Paused => "[Auto mode] Continue where you left off.".into(),
            Nudge::RunsGoing => "[Auto mode] Research runs are still going. Wait for them with experiment_wait, then carry on with the research.".into(),
        }
    }
}

/// Decides whether a chat's last reply needs a nudge. A reply the person stopped
/// never does.
pub fn nudge_for(
    status: &str,
    error: Option<&str>,
    content: &str,
    runs_going: bool,
) -> Option<Nudge> {
    match status {
        "interrupted" | "error" if error != Some("Response stopped") => Some(Nudge::Stopped(
            error.unwrap_or("it was interrupted").to_owned(),
        )),
        "completed" if content.trim_end().ends_with(PAUSED) => Some(Nudge::Paused),
        "completed" if runs_going => Some(Nudge::RunsGoing),
        _ => None,
    }
}

pub async fn worker(s: AppState, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            _ = shutdown.cancelled() => break,
        }
        if let Err(error) = tick(&s).await {
            tracing::warn!("auto mode watchdog: {error}");
        }
    }
}

/// One pass of the watchdog; the worker runs it every minute.
pub async fn tick(s: &AppState) -> Result<(), sqlx::Error> {
    let Some(gateway) = s.gateway.clone() else {
        return Ok(());
    };
    if !s
        .connections
        .as_ref()
        .is_some_and(|c| c.auto_mode_allowed())
    {
        return Ok(());
    }
    // A worker that died leaves an expired lease; reading the chat recovers it.
    let expired: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, owner_id FROM conversations WHERE auto_approve AND active_generation IS NOT NULL \
         AND generation_expires <= NOW() LIMIT 20",
    )
    .fetch_all(&s.store.pool)
    .await?;
    for (id, owner) in expired {
        let _ = s.store.get(owner, id).await;
    }
    // Each nudge waits longer after the chat last changed: 1, 2, 4, 8, 16, then 30 minutes.
    let rows = sqlx::query(
        "SELECT c.id, c.owner_id, c.model, c.settings, c.auto_nudges, c.auto_nudged_at, \
                m.status, m.error_message, m.content, \
                jsonb_array_length(COALESCE(m.activity, '[]'::jsonb)) AS steps, \
                EXISTS(SELECT 1 FROM research r JOIN research_runs x ON x.research_id = r.id \
                       WHERE r.conversation_id = c.id AND x.status = 'running') AS runs_going \
         FROM conversations c \
         JOIN users u ON u.id = c.owner_id AND u.enabled \
         JOIN messages m ON m.id = c.selected_leaf_id AND m.role = 'assistant' \
         WHERE c.auto_approve AND c.active_generation IS NULL AND c.auto_nudges < $1 \
           AND c.updated_at > NOW() - make_interval(hours => $2) \
           AND GREATEST(c.updated_at, COALESCE(c.auto_nudged_at, c.updated_at)) \
               < NOW() - make_interval(mins => LEAST(30, power(2, LEAST(c.auto_nudges, 5))::int)) \
         ORDER BY c.updated_at LIMIT 5",
    )
    .bind(MAX_NUDGES)
    .bind(RECENT_HOURS)
    .fetch_all(&s.store.pool)
    .await?;
    for row in rows {
        if s.shutdown.is_cancelled() {
            break;
        }
        let id: Uuid = row.get("id");
        let status: String = row.get("status");
        let error: Option<String> = row.get("error_message");
        let content: String = row.get("content");
        let Some(nudge) = nudge_for(&status, error.as_deref(), &content, row.get("runs_going"))
        else {
            continue;
        };
        // A reply that did real work earns a fresh count.
        let steps: i32 = row.get("steps");
        let count = if steps >= PROGRESS_STEPS {
            1
        } else {
            row.get::<i32, _>("auto_nudges") + 1
        };
        // One replica claims each nudge.
        let claimed = sqlx::query(
            "UPDATE conversations SET auto_nudges = $2, auto_nudged_at = NOW() \
             WHERE id = $1 AND active_generation IS NULL AND auto_nudged_at IS NOT DISTINCT FROM $3",
        )
        .bind(id)
        .bind(count)
        .bind(row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("auto_nudged_at"))
        .execute(&s.store.pool)
        .await?
        .rows_affected()
            == 1;
        if !claimed {
            continue;
        }
        let owner: Uuid = row.get("owner_id");
        let Some(user) = identity(s, owner).await? else {
            continue;
        };
        let settings: Value = row.get("settings");
        let launch = Launch {
            content: nudge.message(),
            model: row.get("model"),
            attachment_ids: vec![],
            overrides: serde_json::from_value::<models::ParameterOverrides>(settings)
                .unwrap_or_default(),
            variant: None,
        };
        match launch_turn(s.clone(), gateway.clone(), user, None, id, launch).await {
            Ok(_) => {
                tracing::info!(conversation = %id, nudge = count, "auto mode nudged a stopped turn")
            }
            Err(error) => {
                tracing::warn!(conversation = %id, "auto mode could not nudge: {}", error.1)
            }
        }
    }
    Ok(())
}

async fn identity(s: &AppState, user: Uuid) -> Result<Option<Identity>, sqlx::Error> {
    let row: Option<(Uuid, String, String, Role)> = sqlx::query_as(
        "SELECT id, username, display_name, role FROM users WHERE id = $1 AND enabled",
    )
    .bind(user)
    .fetch_optional(&s.store.pool)
    .await?;
    Ok(row.map(|(id, username, display_name, role)| Identity {
        id,
        username,
        display_name,
        role,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nudges_only_what_stopped_without_the_person() {
        assert_eq!(
            nudge_for("interrupted", Some("Sign-in session ended"), "", false),
            Some(Nudge::Stopped("Sign-in session ended".into()))
        );
        assert_eq!(
            nudge_for(
                "error",
                Some("Gateway stream ended before completion"),
                "x",
                false
            ),
            Some(Nudge::Stopped(
                "Gateway stream ended before completion".into()
            ))
        );
        assert_eq!(
            nudge_for("interrupted", Some("Response stopped"), "", true),
            None,
            "the person pressed Stop"
        );
        assert_eq!(
            nudge_for(
                "completed",
                None,
                "Done.\n\n_Paused after 100 workspace steps. Say \"continue\" to pick up where it left off._",
                false
            ),
            Some(Nudge::Paused)
        );
        assert_eq!(
            nudge_for("completed", None, "Started runs 4 and 5.", true),
            Some(Nudge::RunsGoing)
        );
        assert_eq!(nudge_for("completed", None, "All done.", false), None);
        assert!(Nudge::Stopped("It broke.".into())
            .message()
            .contains("(It broke)"));
    }
}
