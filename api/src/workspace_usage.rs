//! Workspaces and browsers for administrators: the pods open now with what they use, and
//! over a period their pod hours, the busiest moment against the slots, starts that failed
//! and starts turned away because every slot was in use. Shows who and when, never what ran
//! in a workspace or which pages a browser had open.
use crate::{auth::Identity, error::ApiError, AppState};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, DurationRound, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

const PERIODS: [i32; 4] = [1, 7, 28, 90];

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/workspaces", get(overview))
        .route("/api/admin/workspaces/stop", post(stop))
        .with_state(state)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Period {
    days: Option<i32>,
}

/// A workspace or browser pod that is starting, running or failed.
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Open {
    kind: String,
    owner_id: Uuid,
    username: String,
    display_name: String,
    #[serde(skip)]
    slug: String,
    status: String,
    error_message: Option<String>,
    started_at: Option<DateTime<Utc>>,
    last_used_at: DateTime<Utc>,
    disk_bytes: Option<i64>,
    /// Chats using the workspace, or chats with a tab in the browser.
    chats: i64,
    previews: i64,
}

/// One pod's run, from the log the pod tables keep.
#[derive(Clone, Debug, FromRow)]
pub struct Session {
    pub kind: String,
    pub owner_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub ready_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub outcome: Option<String>,
}

/// Pod hours, the most pods at once, starts, failures and refusals for each day (or hour,
/// for one day) from `start` to `now`, and for the whole period.
pub fn timeline(
    sessions: &[Session],
    start: DateTime<Utc>,
    now: DateTime<Utc>,
    step: Duration,
) -> Value {
    let count = ((now - start).num_seconds() as f64 / step.num_seconds() as f64)
        .ceil()
        .max(1.0) as usize;
    let mut buckets: Vec<Value> = Vec::with_capacity(count);
    let ran: Vec<&Session> = sessions
        .iter()
        .filter(|s| s.outcome.as_deref() != Some("full"))
        .collect();
    let hours = |kind: &str, from: DateTime<Utc>, to: DateTime<Utc>| {
        ran.iter()
            .filter(|s| s.kind == kind)
            .map(|s| {
                let (a, b) = (s.started_at.max(from), s.stopped_at.unwrap_or(now).min(to));
                ((b - a).num_seconds().max(0)) as f64 / 3600.0
            })
            .sum::<f64>()
    };
    // The most pods at once: walk the starts and stops in order.
    let mut edges: Vec<(DateTime<Utc>, i32)> = ran
        .iter()
        .flat_map(|s| [(s.started_at, 1), (s.stopped_at.unwrap_or(now), -1)])
        .collect();
    edges.sort_by_key(|(at, delta)| (*at, *delta));
    let mut open = 0;
    let mut at_edge = 0;
    let (mut peak, mut peak_at) = (0, None);
    for i in 0..count {
        let from = start + step * i as i32;
        let to = (from + step).min(now);
        // Pods still open when the bucket starts.
        while at_edge < edges.len() && edges[at_edge].0 < from {
            open += edges[at_edge].1;
            at_edge += 1;
        }
        let mut most = open;
        while at_edge < edges.len() && edges[at_edge].0 < to {
            open += edges[at_edge].1;
            at_edge += 1;
            most = most.max(open);
        }
        if most > peak {
            (peak, peak_at) = (most, Some(from));
        }
        let within = |s: &Session| s.started_at >= from && s.started_at < to;
        buckets.push(json!({
            "at": from,
            "workspaceHours": hours("workspace", from, to),
            "browserHours": hours("browser", from, to),
            "peak": most,
            "starts": ran.iter().filter(|s| within(s)).count(),
            "failed": sessions.iter().filter(|s| within(s)).filter(|s| s.outcome.as_deref() == Some("error")).count(),
            "refused": sessions.iter().filter(|s| within(s)).filter(|s| s.outcome.as_deref() == Some("full")).count(),
        }));
    }
    let mut waits: Vec<i64> = ran
        .iter()
        .filter(|s| s.started_at >= start)
        .filter_map(|s| s.ready_at.map(|r| (r - s.started_at).num_seconds()))
        .collect();
    waits.sort_unstable();
    let in_period = |s: &Session| s.started_at >= start;
    let mut people: Vec<Uuid> = ran
        .iter()
        .filter(|s| s.stopped_at.is_none_or(|t| t >= start))
        .filter_map(|s| s.owner_id)
        .collect();
    people.sort_unstable();
    people.dedup();
    json!({
        "series": buckets,
        "totals": {
            "workspaceHours": hours("workspace", start, now),
            "browserHours": hours("browser", start, now),
            "peak": peak,
            "peakAt": peak_at,
            "starts": ran.iter().filter(|s| in_period(s)).count(),
            "failed": sessions.iter().filter(|s| in_period(s)).filter(|s| s.outcome.as_deref() == Some("error")).count(),
            "refused": sessions.iter().filter(|s| in_period(s)).filter(|s| s.outcome.as_deref() == Some("full")).count(),
            "people": people.len(),
            "medianStartSeconds": waits.get(waits.len() / 2),
        },
    })
}

async fn overview(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(period): Query<Period>,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    let days = period.days.unwrap_or(7);
    if !PERIODS.contains(&days) {
        return Err(ApiError::bad("Choose 1, 7, 28 or 90 days"));
    }
    let pool = &s.store.pool;
    let Some(sandboxes) = s.sandboxes.as_ref() else {
        return Ok(Json(
            json!({"enabled": false, "off": s.workspaces_off, "days": days}),
        ));
    };
    let config = sandboxes.config();
    let browser_idle = s.browsers.as_ref().map(|b| b.config().idle);
    let mut open: Vec<Open> = sqlx::query_as(
        "SELECT 'workspace' AS kind, p.owner_id, u.username, u.display_name, p.slug, p.status, p.error_message,
           (SELECT MAX(w.started_at) FROM workspace_sessions w WHERE w.kind = 'workspace' AND w.owner_id = p.owner_id AND w.stopped_at IS NULL) AS started_at,
           p.last_used_at, p.disk_bytes,
           (SELECT COUNT(*) FROM sandboxes x WHERE x.pod_id = p.id) AS chats,
           (SELECT COUNT(*) FROM sandboxes x WHERE x.pod_id = p.id AND x.preview_port IS NOT NULL) AS previews
         FROM sandbox_pods p JOIN users u ON u.id = p.owner_id WHERE p.status IN ('starting', 'running', 'error')
         UNION ALL
         SELECT 'browser', b.owner_id, u.username, u.display_name, b.slug, b.status, b.error_message,
           (SELECT MAX(w.started_at) FROM workspace_sessions w WHERE w.kind = 'browser' AND w.owner_id = b.owner_id AND w.stopped_at IS NULL),
           b.last_used_at, NULL::bigint,
           (SELECT COUNT(*) FROM browser_tabs t WHERE t.browser_id = b.id), 0::bigint
         FROM browsers b JOIN users u ON u.id = b.owner_id WHERE b.status IN ('starting', 'running', 'error')
         ORDER BY last_used_at DESC",
    )
    .fetch_all(pool)
    .await?;
    open.sort_by_key(|o| (o.status == "error", std::cmp::Reverse(o.last_used_at)));
    let usage = sandboxes.pod_usage().await;
    let now = Utc::now();
    let rows: Vec<Value> = open
        .iter()
        .map(|o| {
            let (name, idle) = if o.kind == "browser" {
                (
                    crate::browser::pod_name(&o.slug),
                    browser_idle.unwrap_or(config.idle),
                )
            } else {
                (crate::sandbox::pod_name(&o.slug), config.idle)
            };
            let used = usage.as_ref().and_then(|u| u.get(&name));
            let mut row = json!(o);
            row["stopsAt"] = json!((o.status == "running")
                .then(|| o.last_used_at + Duration::from_std(idle).unwrap_or_default()));
            row["cpuMillis"] = json!(used.map(|u| u.0));
            row["memoryBytes"] = json!(used.map(|u| u.1));
            row
        })
        .collect();
    let in_use = open.iter().filter(|o| o.status != "error").count();
    let tally = |kind: &str, status: &str| {
        open.iter()
            .filter(|o| o.kind == kind && o.status == status)
            .count()
    };
    // The period runs to now from the start of its first UTC day (or this hour's day).
    let today = now.duration_trunc(Duration::days(1)).unwrap_or(now);
    let start = today - Duration::days(i64::from(days) - 1);
    let step = if days == 1 {
        Duration::hours(1)
    } else {
        Duration::days(1)
    };
    let sessions: Vec<Session> = sqlx::query_as(
        "SELECT kind, owner_id, started_at, ready_at, stopped_at, outcome FROM workspace_sessions
         WHERE started_at < NOW() AND (stopped_at IS NULL OR stopped_at >= $1) ORDER BY started_at",
    )
    .bind(start)
    .fetch_all(pool)
    .await?;
    let since: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT MIN(started_at) FROM workspace_sessions")
            .fetch_one(pool)
            .await?;
    Ok(Json(json!({
        "enabled": true,
        "days": days,
        "start": start,
        "slots": config.slots,
        "everyone": config.everyone,
        "browsers": s.browsers.is_some(),
        "idleMinutes": {"workspace": config.idle.as_secs() / 60, "browser": browser_idle.map(|d| d.as_secs() / 60)},
        "limits": {
            "cpu": config.cpu_limit, "memory": config.memory_limit, "disk": config.storage,
            "browserCpu": s.browsers.as_ref().map(|b| b.config().cpu_limit.clone()),
            "browserMemory": s.browsers.as_ref().map(|b| b.config().memory_limit.clone()),
        },
        "now": {
            "inUse": in_use,
            "workspaces": {"running": tally("workspace", "running"), "starting": tally("workspace", "starting"), "error": tally("workspace", "error")},
            "browsers": {"running": tally("browser", "running"), "starting": tally("browser", "starting"), "error": tally("browser", "error")},
            "previews": open.iter().map(|o| o.previews).sum::<i64>(),
            "cpuMillis": usage.as_ref().map(|u| rows.iter().filter_map(|r| r["cpuMillis"].as_i64()).sum::<i64>().min(u.values().map(|v| v.0).sum())),
            "memoryBytes": usage.as_ref().map(|_| rows.iter().filter_map(|r| r["memoryBytes"].as_i64()).sum::<i64>()),
        },
        "metrics": usage.is_some(),
        "open": rows,
        "history": timeline(&sessions, start, now, step),
        "recordedSince": since,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stop {
    kind: String,
    owner_id: Uuid,
}

/// Stops one person's workspace or browser pod now, as going idle would. Files and the
/// browser profile stay; the person's next use starts it again.
async fn stop(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Stop>,
) -> Result<Json<Value>, ApiError> {
    let admin: Identity = s.auth.require_admin(&headers, true).await?;
    let stopped = match body.kind.as_str() {
        "workspace" => {
            let sandboxes = s
                .sandboxes
                .as_ref()
                .ok_or_else(|| ApiError::bad("Workspaces are off"))?;
            sandboxes.stop_owner(body.owner_id).await
        }
        "browser" => {
            let browsers = s
                .browsers
                .as_ref()
                .ok_or_else(|| ApiError::bad("The browser is off"))?;
            browsers.stop_owner(body.owner_id).await
        }
        _ => return Err(ApiError::bad("Stop a workspace or a browser")),
    }
    .map_err(|error| {
        tracing::warn!(owner = %body.owner_id, "could not stop a {} pod: {error:#}", body.kind);
        ApiError::bad("The pod could not be stopped. Try again.")
    })?;
    if stopped {
        sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,target_id,metadata)VALUES($1,$2,'workspaces.stop','user',$3,$4)")
            .bind(Uuid::new_v4())
            .bind(admin.id)
            .bind(body.owner_id.to_string())
            .bind(json!({"kind": body.kind}))
            .execute(&s.store.pool)
            .await?;
    }
    Ok(Json(json!({ "stopped": stopped })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(
        kind: &str,
        start: i64,
        ready: Option<i64>,
        stop: Option<i64>,
        outcome: Option<&str>,
    ) -> Session {
        let t0 = DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        Session {
            kind: kind.into(),
            owner_id: Some(Uuid::from_u128(start as u128)),
            started_at: t0 + Duration::minutes(start),
            ready_at: ready.map(|m| t0 + Duration::minutes(m)),
            stopped_at: stop.map(|m| t0 + Duration::minutes(m)),
            outcome: outcome.map(str::to_owned),
        }
    }

    #[test]
    fn cpu_quantities_read_as_millicores() {
        use crate::sandbox::kube::cpu_millis;
        assert_eq!(cpu_millis("250m"), Some(250));
        assert_eq!(cpu_millis("123456789n"), Some(123));
        assert_eq!(cpu_millis("1500u"), Some(2));
        assert_eq!(cpu_millis("2"), Some(2000));
        assert_eq!(cpu_millis("0.5"), Some(500));
        assert_eq!(cpu_millis("lots"), None);
    }

    #[test]
    fn pod_hours_peaks_and_refusals_come_from_the_log() {
        let t0 = DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let sessions = vec![
            // Started the day before, ran an hour into this one.
            session("workspace", -60, Some(-59), Some(60), Some("stopped")),
            session("browser", 30, Some(31), Some(90), Some("stopped")),
            session("workspace", 45, None, Some(46), Some("error")),
            session("workspace", 50, None, Some(50), Some("full")),
            // Still running at "now".
            session("workspace", 100, Some(101), None, None),
        ];
        let now = t0 + Duration::minutes(160);
        let out = timeline(&sessions, t0, now, Duration::hours(1));
        let series = out["series"].as_array().unwrap();
        assert_eq!(series.len(), 3);
        assert_eq!(
            series[0]["peak"], 3,
            "the old workspace, the browser and the failed start at 0:45"
        );
        assert_eq!(series[0]["refused"], 1);
        assert_eq!(series[0]["failed"], 1);
        assert_eq!(series[0]["starts"], 2);
        assert!((series[0]["workspaceHours"].as_f64().unwrap() - (1.0 + 1.0 / 60.0)).abs() < 1e-9);
        assert!((series[0]["browserHours"].as_f64().unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(series[2]["peak"], 1);
        let totals = &out["totals"];
        assert_eq!(totals["peak"], 3);
        assert_eq!(totals["refused"], 1);
        assert_eq!(totals["people"], 4, "the refused start ran nothing");
        assert_eq!(totals["medianStartSeconds"], 60);
    }
}
