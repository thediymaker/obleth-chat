//! Thumbs up or down on replies. Only a chat's owner rates its replies, and a rating
//! stays in Chat: nothing is sent to the gateway. Administrators see ratings per model,
//! the reasons given and recent comments, but never the conversation or who rated it.
use crate::{error::ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Why a reply was not good, as the thumbs-down form offers them.
pub const REASONS: [&str; 6] = [
    "wrong",
    "instructions",
    "too_long",
    "too_short",
    "harmful",
    "other",
];
pub const MAX_COMMENT_CHARS: usize = 2_000;
/// Comments shown to administrators, newest first.
const RECENT_COMMENTS: i64 = 50;

/// A person's rating of one reply, as the owner reads it back with the chat.
#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Feedback {
    #[serde(skip)]
    pub message_id: Uuid,
    pub rating: i16,
    pub reasons: Vec<String>,
    pub comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rate {
    rating: i16,
    #[serde(default)]
    reasons: Vec<String>,
    comment: Option<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/conversations/{id}/messages/{message}/feedback",
            put(rate).delete(clear),
        )
        .route("/api/admin/feedback", get(summary))
        .route("/api/admin/feedback/models.csv", get(models_csv))
        .with_state(state)
}

/// The owner's ratings of these replies.
pub async fn for_messages(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    messages: &[Uuid],
) -> Result<Vec<Feedback>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT message_id,rating,reasons,comment FROM message_feedback WHERE user_id=$1 AND message_id=ANY($2)",
    )
    .bind(owner)
    .bind(messages)
    .fetch_all(tx)
    .await?)
}

fn clean(body: Rate) -> Result<(i16, Vec<String>, Option<String>), ApiError> {
    if !matches!(body.rating, 1 | -1) {
        return Err(ApiError::bad("Rate a reply up (1) or down (-1)"));
    }
    let mut reasons = Vec::new();
    for reason in body.reasons {
        if !REASONS.contains(&reason.as_str()) {
            return Err(ApiError::bad("Unknown reason"));
        }
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    if body.rating == 1 && !reasons.is_empty() {
        return Err(ApiError::bad("Reasons are for a thumbs down"));
    }
    let comment = body
        .comment
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty());
    if comment
        .as_ref()
        .is_some_and(|c| c.chars().count() > MAX_COMMENT_CHARS || c.contains('\0'))
    {
        return Err(ApiError::bad("Comments are up to 2,000 characters"));
    }
    Ok((body.rating, reasons, comment))
}

async fn rate(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, message)): Path<(Uuid, Uuid)>,
    Json(body): Json<Rate>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let (rating, reasons, comment) = clean(body)?;
    // A finished reply in one of the person's own chats; the model it came from is
    // what Auto picked, when it picked one.
    let model: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE(NULLIF(m.settings->>'resolvedModel',''),NULLIF(m.settings->>'model',''),c.model)
         FROM messages m JOIN conversations c ON c.id=m.conversation_id
         WHERE m.id=$1 AND c.id=$2 AND c.owner_id=$3 AND m.role='assistant' AND m.status<>'pending'",
    )
    .bind(message)
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.store.pool)
    .await?;
    let model = model.ok_or_else(ApiError::not_found)?;
    let model: String = model.chars().take(200).collect();
    let feedback: Feedback = sqlx::query_as(
        "INSERT INTO message_feedback(message_id,user_id,rating,reasons,comment,model) VALUES($1,$2,$3,$4,$5,$6)
         ON CONFLICT (message_id,user_id) DO UPDATE SET rating=EXCLUDED.rating,reasons=EXCLUDED.reasons,
           comment=EXCLUDED.comment,model=EXCLUDED.model,updated_at=NOW()
         RETURNING message_id,rating,reasons,comment",
    )
    .bind(message)
    .bind(user.id)
    .bind(rating)
    .bind(&reasons)
    .bind(&comment)
    .bind(&model)
    .fetch_one(&s.store.pool)
    .await?;
    Ok(Json(json!({ "feedback": feedback })))
}

async fn clear(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, message)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    sqlx::query(
        "DELETE FROM message_feedback f USING messages m, conversations c
         WHERE f.message_id=$1 AND f.user_id=$3 AND m.id=f.message_id AND c.id=m.conversation_id AND c.id=$2 AND c.owner_id=$3",
    )
    .bind(message)
    .bind(id)
    .bind(user.id)
    .execute(&s.store.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Period {
    days: Option<i32>,
}
impl Period {
    fn days(&self) -> Result<i32, ApiError> {
        let days = self.days.unwrap_or(30);
        if !(1..=365).contains(&days) {
            return Err(ApiError::bad("Choose a period of 1 to 365 days"));
        }
        Ok(days)
    }
}

/// A comment as administrators see it: no chat, reply or person.
#[derive(Serialize, FromRow)]
struct Comment {
    model: String,
    rating: i16,
    reasons: Vec<String>,
    comment: String,
    at: DateTime<Utc>,
}

#[derive(FromRow)]
struct ModelRow {
    model: String,
    up: i64,
    down: i64,
}
impl ModelRow {
    /// The share of ratings that were up, from 0 to 1.
    fn up_share(&self) -> f64 {
        self.up as f64 / (self.up + self.down).max(1) as f64
    }
}

async fn by_model(pool: &PgPool, days: i32) -> Result<Vec<ModelRow>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT model,COUNT(*) FILTER (WHERE rating=1) AS up,COUNT(*) FILTER (WHERE rating=-1) AS down
         FROM message_feedback WHERE updated_at>=NOW()-make_interval(days=>$1)
         GROUP BY model ORDER BY COUNT(*) DESC,model LIMIT 500",
    )
    .bind(days)
    .fetch_all(pool)
    .await?)
}

/// Ratings per model, the reasons given for thumbs down and recent comments. Only the
/// rating, reasons, comment, model and date leave the table: no chat, reply or person.
async fn summary(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(period): Query<Period>,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    let days = period.days()?;
    let pool = &s.store.pool;
    let models = by_model(pool, days).await?;
    let reasons: Vec<(String, i64)> = sqlx::query_as(
        "SELECT reason,COUNT(*) FROM message_feedback,unnest(reasons) AS reason
         WHERE rating=-1 AND updated_at>=NOW()-make_interval(days=>$1) GROUP BY reason ORDER BY COUNT(*) DESC,reason",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let comments: Vec<Comment> = sqlx::query_as(
        "SELECT model,rating,reasons,comment,updated_at AS at FROM message_feedback
         WHERE comment IS NOT NULL AND updated_at>=NOW()-make_interval(days=>$1) ORDER BY updated_at DESC LIMIT $2",
    )
    .bind(days)
    .bind(RECENT_COMMENTS)
    .fetch_all(pool)
    .await?;
    Ok(Json(json!({
        "days": days,
        "models": models.iter().map(|m| json!({"model":m.model,"up":m.up,"down":m.down,"upShare":m.up_share()})).collect::<Vec<_>>(),
        "reasons": reasons.into_iter().map(|(reason,count)| json!({"reason":reason,"count":count})).collect::<Vec<_>>(),
        "comments": comments,
    })))
}

/// One CSV field. Quotes when needed, and a leading formula character is escaped so a
/// spreadsheet shows a model name as text.
fn csv_field(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

async fn models_csv(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(period): Query<Period>,
) -> Result<Response, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    let days = period.days()?;
    let mut csv = String::from("model,up,down,up_percent\r\n");
    for m in by_model(&s.store.pool, days).await? {
        csv.push_str(&format!(
            "{},{},{},{:.1}\r\n",
            csv_field(&m.model),
            m.up,
            m.down,
            m.up_share() * 100.0
        ));
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"feedback-{days}-days.csv\""),
            ),
        ],
        csv,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_quoted_and_never_formulas() {
        assert_eq!(csv_field("org/model-a"), "org/model-a");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("=SUM(A1)"), "'=SUM(A1)");
    }

    #[test]
    fn ratings_take_known_reasons_once() {
        let rate = |rating, reasons: &[&str], comment: Option<&str>| {
            clean(Rate {
                rating,
                reasons: reasons.iter().map(|r| r.to_string()).collect(),
                comment: comment.map(str::to_owned),
            })
        };
        assert_eq!(
            rate(-1, &["wrong", "wrong", "other"], Some("  ")).unwrap(),
            (-1, vec!["wrong".into(), "other".into()], None)
        );
        assert!(rate(0, &[], None).is_err());
        assert!(rate(1, &["wrong"], None).is_err());
        assert!(rate(-1, &["boring"], None).is_err());
        assert!(rate(-1, &[], Some(&"x".repeat(2_001))).is_err());
        assert!(rate(-1, &[], Some(&"x".repeat(2_000))).is_ok());
    }
}
