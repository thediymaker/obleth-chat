//! Second opinions on a reply Auto chose. The gateway's Auto router answers with one
//! model and says which models it ranked next and how close they were. Now and then
//! Chat puts the same request to those models itself, in the background, and keeps
//! their answers as versions of the reply: the person reads them side by side and
//! keeps the one the chat should go on from. The gateway never answers twice; these
//! are Chat's own requests, by name, counted in the usage ledger like any other.
//!
//! An administrator chooses when (all off as installed): Auto's pick was a close call,
//! the request read as hard, or one Auto reply in so many. Only a reply that came
//! from one request to the model is compared, since the others are asked without
//! tools, and a person gets a set number in a day.
use crate::audit::{self, Action, Target};
use crate::store::{self, Require};
use crate::{
    auth::Identity,
    error::ApiError,
    gateway::{self, Decoder, Gateway},
    models::{self, ParameterOverrides},
    operations, AppState,
};
use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Models asked beside the one Auto chose.
const OTHERS: usize = 2;
/// How long one of them may take. Its reservation is held a little longer.
const TIMEOUT: Duration = Duration::from_secs(180);
const LEASE_SECONDS: i64 = 200;

/// When Chat asks for second opinions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// Auto's pick was close: the best model it did not choose scored within
    /// `close_gap` of the one it did.
    pub close_call: bool,
    pub close_gap: f64,
    /// The request read as hard.
    pub hard: bool,
    /// One Auto reply in this many, by chance; 0 is never.
    pub sample_every: i32,
    /// How many replies a person has compared in a day.
    pub daily_limit: i32,
}

impl Settings {
    /// Whether any reply could be compared.
    pub fn on(&self) -> bool {
        self.close_call || self.hard || self.sample_every > 0
    }

    fn check(self) -> Result<Self, ApiError> {
        if !(self.close_gap.is_finite() && (0.0..=1.0).contains(&self.close_gap)) {
            return Err(ApiError::bad(
                "How close a close call is must be between 0 and 1",
            ));
        }
        if !(0..=1000).contains(&self.sample_every) {
            return Err(ApiError::bad(
                "One reply in how many must be between 2 and 1000, or 0 for never",
            ));
        }
        if !(1..=100).contains(&self.daily_limit) {
            return Err(ApiError::bad(
                "The daily number for one person must be between 1 and 100",
            ));
        }
        Ok(self)
    }
}

const COLUMNS: &str = "close_call, close_gap, hard, sample_every, daily_limit";

async fn settings(pool: &PgPool) -> Result<Settings, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM compare_settings WHERE id"))
        .fetch_one(pool)
        .await
}

/// Whether a reply starting now could be compared, so its request is worth keeping.
pub async fn watching(pool: &PgPool) -> bool {
    settings(pool).await.is_ok_and(|s| s.on())
}

/// Why a reply is compared.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Trigger {
    Close,
    Hard,
    Sample,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Trigger::Close => "close",
            Trigger::Hard => "hard",
            Trigger::Sample => "sample",
        }
    }
}

/// Whether this reply gets second opinions, from what Auto said about its pick.
/// `draw` is a number from 0 up to 1, by chance.
fn trigger(settings: &Settings, why: Option<&Value>, draw: f64) -> Option<Trigger> {
    let gap = why
        .and_then(|w| w.pointer("/next/0/gap"))
        .and_then(Value::as_f64);
    if settings.close_call && gap.is_some_and(|gap| gap <= settings.close_gap) {
        return Some(Trigger::Close);
    }
    let difficulty = why
        .and_then(|w| w.get("difficulty"))
        .and_then(Value::as_u64);
    if settings.hard && difficulty.is_some_and(|level| level >= 3) {
        return Some(Trigger::Hard);
    }
    (settings.sample_every > 0 && draw < 1.0 / f64::from(settings.sample_every))
        .then_some(Trigger::Sample)
}

/// The models a `next` list names, without the one that answered.
fn named(next: Option<&Value>, served: &str) -> Vec<String> {
    let mut models: Vec<String> = Vec::with_capacity(OTHERS);
    for model in next
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|n| n.get("model").and_then(Value::as_str))
    {
        if model != served
            && model != "auto"
            && gateway::valid_model(model)
            && !models.iter().any(|m| m == model)
            && models.len() < OTHERS
        {
            models.push(model.to_owned());
        }
    }
    models
}

/// A chat that kept its model was not ranked again; the runners-up of its last
/// ranking stand in.
async fn remembered(pool: &PgPool, conversation: Uuid) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT settings->'autoWhy'->'next' FROM messages WHERE conversation_id = $1
         AND role = 'assistant' AND jsonb_typeof(settings->'autoWhy'->'next') = 'array'
         ORDER BY ordinal DESC LIMIT 1",
    )
    .bind(conversation)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// The request as another model takes it: the same messages and settings, by that
/// model's name, without tools. `None` when it cannot take it.
fn request_for(
    request: &Value,
    model: &str,
    capabilities: &gateway::ModelMetadata,
) -> Option<Value> {
    let mut payload = request.clone();
    let body = payload.as_object_mut()?;
    for key in ["tools", "tool_choice", "parallel_tool_calls"] {
        body.remove(key);
    }
    body.insert("model".into(), json!(model));
    body.insert("stream".into(), json!(true));
    body.insert("stream_options".into(), json!({"include_usage": true}));
    if capabilities.supports_reasoning == Some(false) {
        body.remove("reasoning_effort");
    }
    let messages = body.get("messages")?.as_array()?;
    // A tool call or result in the request means the reply was not one plain answer.
    if messages
        .iter()
        .any(|m| m.get("tool_calls").is_some() || m["role"] == "tool")
    {
        return None;
    }
    let pictures = messages.iter().any(|m| {
        m["content"].as_array().is_some_and(|parts| {
            parts
                .iter()
                .any(|p| p["type"] == "image_url" || p["type"] == "input_image")
        })
    });
    if pictures && capabilities.supports_vision != Some(true) {
        return None;
    }
    Some(payload)
}

/// After a reply Auto chose: decides whether it is compared and, if so, starts the
/// other models on `request`, which is what the reply itself was asked with. Returns
/// whether answers are on their way, so the page knows to look for them.
#[allow(clippy::too_many_arguments)]
pub async fn after_reply(
    state: &AppState,
    gateway: &Gateway,
    user: &Identity,
    conversation: Uuid,
    reply: Uuid,
    served: &str,
    why: Option<&Value>,
    request: Value,
) -> bool {
    if state.shutdown.is_cancelled() {
        return false;
    }
    let Ok(settings) = settings(&state.store.pool).await else {
        return false;
    };
    let Some(trigger) = trigger(&settings, why, rand::random::<f64>()) else {
        return false;
    };
    let mut models = named(why.and_then(|w| w.get("next")), served);
    if models.is_empty() {
        models = named(
            remembered(&state.store.pool, conversation).await.as_ref(),
            served,
        );
    }
    if models.is_empty() {
        return false;
    }
    // Optional work never takes the last room a person's own message would need.
    let Ok(mut spare) = state
        .slots
        .clone()
        .try_acquire_many_owned(models.len() as u32 + 1)
    else {
        return false;
    };
    let permits = spare
        .split(models.len())
        .expect("as many permits as models were acquired");
    drop(spare);
    let claimed = tokio::time::timeout(
        Duration::from_secs(2),
        claim(
            &state.store.pool,
            user.id,
            conversation,
            reply,
            &models,
            trigger,
            settings.daily_limit,
        ),
    )
    .await;
    if !matches!(claimed, Ok(Ok(true))) {
        return false;
    }
    let (state, gateway, user) = (state.clone(), gateway.clone(), user.clone());
    tokio::spawn(async move {
        let _permits = permits;
        let answers = futures_util::future::join_all(models.iter().map(|model| {
            one(
                &state,
                &gateway,
                &user,
                conversation,
                reply,
                model,
                &request,
            )
        }))
        .await;
        if !answers.contains(&true) {
            // Nothing came of it, so the reply says nothing about it.
            let _ = sqlx::query(
                "UPDATE messages SET settings = settings - 'compare' WHERE id = $1 AND conversation_id = $2",
            )
            .bind(reply)
            .bind(conversation)
            .execute(&state.store.pool)
            .await;
        }
    });
    true
}

/// Marks the reply as being compared, once, when it is still the chat's last reply
/// and its owner has not had the day's number yet.
async fn claim(
    pool: &PgPool,
    owner: Uuid,
    conversation: Uuid,
    reply: Uuid,
    models: &[String],
    trigger: Trigger,
    daily_limit: i32,
) -> Result<bool, sqlx::Error> {
    let note = json!({"models": models, "why": trigger.as_str(), "at": Utc::now()});
    let claimed = sqlx::query(
        "UPDATE messages m SET settings = m.settings || jsonb_build_object('compare', $4::jsonb)
         FROM conversations c
         WHERE m.id = $1 AND m.conversation_id = c.id AND c.id = $2 AND c.owner_id = $3
           AND NOT c.temporary AND c.selected_leaf_id = m.id
           AND m.role = 'assistant' AND m.status = 'completed' AND m.content <> ''
           AND NOT jsonb_exists(m.settings, 'compare')
           AND (SELECT COUNT(*) FROM generation_usage g WHERE g.user_id = $3 AND g.purpose = 'compare'
                AND g.created_at >= (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')) < $5",
    )
    .bind(reply)
    .bind(conversation)
    .bind(owner)
    .bind(note)
    .bind(i64::from(daily_limit) * OTHERS as i64)
    .execute(pool)
    .await?;
    Ok(claimed.rows_affected() == 1)
}

/// What one model answered.
struct Answer {
    content: String,
    reasoning: String,
    request_id: Option<String>,
    finish_reason: Option<String>,
}

/// One model's second opinion, start to finish. Returns whether its answer was kept.
async fn one(
    state: &AppState,
    gateway: &Gateway,
    user: &Identity,
    conversation: Uuid,
    reply: Uuid,
    model: &str,
    request: &Value,
) -> bool {
    let id = Uuid::new_v4();
    let mut usage = None;
    let work = answer(
        state,
        gateway,
        user,
        conversation,
        reply,
        id,
        model,
        request,
        &mut usage,
    );
    // Only fixed words reach the log: never a provider's error, the request or the answer.
    let result = tokio::select! {
        result = tokio::time::timeout(TIMEOUT, work) => match result {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => Err("failed"),
            Err(_) => Err("timeout"),
        },
        _ = state.shutdown.cancelled() => Err("shutdown"),
    };
    if let Err(reason) = &result {
        tracing::debug!(reason, "a second opinion did not complete");
    }
    for (key, kind) in [
        ("prompt_tokens", "prompt"),
        ("completion_tokens", "completion"),
    ] {
        if let Some(tokens) = usage
            .as_ref()
            .and_then(|u: &Value| u.get(key))
            .and_then(Value::as_u64)
            .filter(|n| *n <= 1_000_000_000)
        {
            state
                .metrics
                .tokens
                .with_label_values(&[kind])
                .inc_by(tokens);
        }
    }
    let kept = tokio::time::timeout(
        Duration::from_secs(5),
        settle(
            &state.store.pool,
            user.id,
            conversation,
            reply,
            id,
            model,
            result.ok(),
            usage.as_ref(),
        ),
    )
    .await;
    match kept {
        Ok(Ok(kept)) => kept,
        _ => {
            tracing::warn!("could not keep a second opinion");
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn answer(
    state: &AppState,
    gateway: &Gateway,
    user: &Identity,
    conversation: Uuid,
    reply: Uuid,
    id: Uuid,
    model: &str,
    request: &Value,
    usage: &mut Option<Value>,
) -> Result<Answer, ApiError> {
    // The person must be allowed this model now, as for a message of their own.
    let resolved = models::resolve_request(
        &state.store.pool,
        gateway,
        user,
        model,
        ParameterOverrides::default(),
    )
    .await?;
    if resolved.model == "auto" {
        return Err(ApiError::bad("A second opinion names its model"));
    }
    let payload = request_for(request, &resolved.model, &resolved.capabilities)
        .ok_or_else(|| ApiError::bad("This model cannot take the request"))?;
    let max_tokens = payload
        .get("max_tokens")
        .and_then(Value::as_i64)
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(0)
        .clamp(0, models::MAX_OUTPUT_TOKENS);
    let mut tx = state.store.pool.begin().await?;
    store::require_owner(&mut tx, user.id, Require::EnabledNoPasswordChange).await?;
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM messages m JOIN conversations c ON c.id = m.conversation_id
         WHERE m.id = $1 AND c.id = $2 AND c.owner_id = $3 AND jsonb_exists(m.settings, 'compare'))",
    )
    .bind(reply)
    .bind(conversation)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if !current {
        return Err(ApiError::not_found());
    }
    operations::reserve_compare_in(
        &mut tx,
        user.id,
        id,
        &resolved.model,
        max_tokens,
        payload["messages"].to_string().len(),
        LEASE_SECONDS,
    )
    .await?;
    tx.commit().await?;
    let asked = Instant::now();
    let response = gateway.chat(user, conversation, &payload).await?;
    let request_id = response
        .headers()
        .get("x-obleth-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| s.len() <= 200)
        .map(str::to_owned);
    let mut stream = response.bytes_stream();
    let mut decoder = Decoder::default();
    let (mut content, mut reasoning) = (String::new(), String::new());
    let mut finish_reason = None;
    let (mut wrote_first, mut wrote_last): (Option<Instant>, Option<Instant>) = (None, None);
    let mut finished = false;
    'body: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ApiError::bad("The stream was interrupted"))?;
        for event in decoder
            .events(&chunk)
            .map_err(|_| ApiError::bad("The stream could not be read"))?
        {
            let data = match event {
                gateway::Event::Done => {
                    finished = true;
                    break 'body;
                }
                gateway::Event::Failed { .. } => {
                    return Err(ApiError::bad("The provider reported an error"))
                }
                gateway::Event::Chunk(data) => data,
            };
            if let Some(reported) = gateway::usage_of(&data) {
                let mut clean = json!({});
                gateway::add_usage(&mut clean, reported);
                clean["steps"] = json!(1);
                *usage = Some(clean);
            }
            let Some(choice) = data.pointer("/choices/0") else {
                continue;
            };
            if choice.pointer("/delta/tool_calls").is_some() {
                return Err(ApiError::bad("A second opinion has no tools"));
            }
            for (channel, delta) in gateway::text_deltas(choice) {
                if content.len() + reasoning.len() + delta.len() > store::MAX_RESPONSE_BYTES {
                    return Err(ApiError::bad("The answer is too large to keep"));
                }
                let now = Instant::now();
                wrote_first.get_or_insert(now);
                wrote_last = Some(now);
                if channel == "delta" {
                    content.push_str(delta);
                } else {
                    reasoning.push_str(delta);
                }
            }
            if let Some(reason) = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .filter(|r| r.len() <= 80)
            {
                finish_reason = Some(reason.to_owned());
                finished = true;
            }
        }
    }
    if !finished {
        return Err(ApiError::bad("The stream did not finish"));
    }
    // How long it waited for the first token and how long it wrote, as a reply has them.
    if let (Some(from), Some(to), Some(usage)) = (wrote_first, wrote_last, usage.as_mut()) {
        usage["generation_ms"] = json!(to.duration_since(from).as_millis() as u64);
        usage["first_token_ms"] = json!(from.duration_since(asked).as_millis() as u64);
    }
    Ok(Answer {
        content: store::strip_records(&content),
        reasoning,
        request_id,
        finish_reason,
    })
}

/// Keeps an answer as a version of the reply, beside it and not in its place, and
/// settles what it cost. An answer that failed, came back empty, or whose reply is
/// gone is settled and not kept.
#[allow(clippy::too_many_arguments)]
async fn settle(
    pool: &PgPool,
    owner: Uuid,
    conversation: Uuid,
    reply: Uuid,
    id: Uuid,
    model: &str,
    answer: Option<Answer>,
    usage: Option<&Value>,
) -> Result<bool, ApiError> {
    let mut tx = pool.begin().await?;
    // Work already under way still settles after an account is disabled.
    store::lock_owner(&mut tx, owner, Require::Any).await?;
    let mut kept = false;
    let written = answer.as_ref().map(|a| a.content.len() + a.reasoning.len());
    if let Some(answer) = answer.filter(|a| !a.content.trim().is_empty()) {
        // The conversation's row is the lock every writer of its messages takes.
        let source: Option<(Option<Uuid>, Value)> = sqlx::query_as(
            "SELECT m.parent_message_id, m.settings FROM messages m JOIN conversations c ON c.id = m.conversation_id
             WHERE m.id = $1 AND c.id = $2 AND c.owner_id = $3 AND jsonb_exists(m.settings, 'compare') FOR UPDATE OF c",
        )
        .bind(reply)
        .bind(conversation)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((parent, mut settings)) = source {
            let ordinal: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(ordinal) + 1, 0)::bigint FROM messages WHERE conversation_id = $1",
            )
            .bind(conversation)
            .fetch_one(&mut *tx)
            .await?;
            if (ordinal as usize) < store::MAX_STORED_MESSAGES {
                // The reply's own parameters, by this model's name. What Auto said
                // about its pick stays with the reply Auto chose.
                if let Some(map) = settings.as_object_mut() {
                    map.retain(|key, _| models::PARAMETER_KEYS.contains(&key.as_str()));
                    map.insert("model".into(), json!(model));
                    map.insert("compareOf".into(), json!(reply));
                }
                sqlx::query(
                    "INSERT INTO messages (id, conversation_id, ordinal, role, content, reasoning_content, status,
                                           generation_id, settings, parent_message_id, usage, request_id, finish_reason)
                     VALUES ($1, $2, $3, 'assistant', $4, $5, 'completed', $1, $6, $7, $8, $9, $10)",
                )
                .bind(id)
                .bind(conversation)
                .bind(ordinal as i32)
                .bind(&answer.content)
                .bind(&answer.reasoning)
                .bind(settings)
                .bind(parent)
                .bind(usage)
                .bind(&answer.request_id)
                .bind(&answer.finish_reason)
                .execute(&mut *tx)
                .await?;
                kept = true;
            }
        }
    }
    // Without a reservation (the model was refused, or the day's allowance) this does nothing.
    operations::finish_with_output_in(
        &mut tx,
        id,
        usage,
        if kept { "completed" } else { "error" },
        written,
    )
    .await?;
    tx.commit().await?;
    Ok(kept)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/compare", get(read).patch(save))
        .with_state(state)
}

async fn document(pool: &PgPool) -> Result<Value, ApiError> {
    let settings = settings(pool).await?;
    let updated_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM compare_settings WHERE id")
            .fetch_one(pool)
            .await?;
    let mut document = serde_json::to_value(&settings).map_err(|_| ApiError::internal())?;
    document["updatedAt"] = json!(updated_at);
    Ok(document)
}

async fn read(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    Ok(Json(document(&s.store.pool).await?))
}

async fn save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let change: Settings = serde_json::from_value(body).map_err(|_| {
        ApiError::bad("Comparing takes closeCall, closeGap, hard, sampleEvery and dailyLimit")
    })?;
    let change = change.check()?;
    let mut tx = operations::admin_transaction(&s.store.pool, user.id).await?;
    sqlx::query(
        "UPDATE compare_settings SET close_call = $1, close_gap = $2, hard = $3, sample_every = $4,
         daily_limit = $5, updated_at = NOW() WHERE id",
    )
    .bind(change.close_call)
    .bind(change.close_gap)
    .bind(change.hard)
    .bind(change.sample_every)
    .bind(change.daily_limit)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        Action::COMPARE_UPDATE,
        Target::site(),
        serde_json::to_value(&change).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(document(&s.store.pool).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            close_call: false,
            close_gap: 0.05,
            hard: false,
            sample_every: 0,
            daily_limit: 3,
        }
    }

    #[test]
    fn nothing_is_compared_until_an_administrator_says_when() {
        let off = settings();
        assert!(!off.on());
        let why = json!({"difficulty": 3, "next": [{"model": "b", "gap": 0.0}]});
        assert_eq!(trigger(&off, Some(&why), 0.0), None);
    }

    #[test]
    fn a_close_call_a_hard_request_or_chance_each_compare_a_reply() {
        let close = Settings {
            close_call: true,
            ..settings()
        };
        assert!(close.on());
        let near = json!({"difficulty": 1, "next": [{"model": "b", "gap": 0.05}, {"model": "c", "gap": 0.0}]});
        let far = json!({"difficulty": 3, "next": [{"model": "b", "gap": 0.051}]});
        assert_eq!(trigger(&close, Some(&near), 0.9), Some(Trigger::Close));
        assert_eq!(trigger(&close, Some(&far), 0.9), None);
        // A chat that kept its model, or an older gateway, gives no distance.
        assert_eq!(trigger(&close, Some(&json!({"reason": "kept"})), 0.9), None);
        assert_eq!(
            trigger(&close, Some(&json!({"next": [{"model": "b"}]})), 0.9),
            None
        );
        assert_eq!(trigger(&close, None, 0.9), None);

        let hard = Settings {
            hard: true,
            ..settings()
        };
        assert_eq!(trigger(&hard, Some(&far), 0.9), Some(Trigger::Hard));
        assert_eq!(trigger(&hard, Some(&near), 0.9), None);

        let sample = Settings {
            sample_every: 10,
            ..settings()
        };
        assert_eq!(trigger(&sample, None, 0.09), Some(Trigger::Sample));
        assert_eq!(trigger(&sample, None, 0.1), None);

        // The first ground that holds is the one given.
        let all = Settings {
            close_call: true,
            hard: true,
            sample_every: 2,
            ..settings()
        };
        assert_eq!(trigger(&all, Some(&near), 0.0), Some(Trigger::Close));
        assert_eq!(trigger(&all, Some(&far), 0.0), Some(Trigger::Hard));
        assert_eq!(
            trigger(&all, Some(&json!({"difficulty": 2})), 0.0),
            Some(Trigger::Sample)
        );
    }

    #[test]
    fn the_others_are_the_named_models_without_the_one_that_answered() {
        let next = json!([{"model": "b", "gap": 0.1}, {"model": "a"}, {"model": "b"}, {"model": "auto"}, {"model": "c"}, {"model": "d"}, {"gap": 1}]);
        assert_eq!(named(Some(&next), "a"), ["b", "c"]);
        assert_eq!(named(Some(&next), "z"), ["b", "a"]);
        assert!(named(None, "a").is_empty());
        assert!(named(Some(&json!("b")), "a").is_empty());
    }

    #[test]
    fn another_model_gets_the_same_request_without_tools() {
        let request = json!({"model": "auto", "messages": [{"role": "system", "content": "s"}, {"role": "user", "content": "q"}],
            "temperature": 0.7, "top_p": 1.0, "max_tokens": 512, "reasoning_effort": "medium", "stream": true,
            "tools": [{"type": "function"}], "tool_choice": "auto"});
        let able = gateway::ModelMetadata::default();
        let payload = request_for(&request, "b", &able).unwrap();
        assert_eq!(payload["model"], "b");
        assert_eq!(payload["messages"], request["messages"]);
        assert_eq!(payload["max_tokens"], 512);
        assert_eq!(payload["reasoning_effort"], "medium");
        assert!(payload.get("tools").is_none() && payload.get("tool_choice").is_none());
        assert_eq!(payload["stream_options"]["include_usage"], true);

        let plain = gateway::ModelMetadata {
            supports_reasoning: Some(false),
            ..Default::default()
        };
        assert!(request_for(&request, "b", &plain)
            .unwrap()
            .get("reasoning_effort")
            .is_none());

        // A picture needs a model that reads pictures.
        let mut pictured = request.clone();
        pictured["messages"][1]["content"] = json!([{"type": "text", "text": "q"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}}]);
        assert!(request_for(&pictured, "b", &able).is_none());
        let seeing = gateway::ModelMetadata {
            supports_vision: Some(true),
            ..Default::default()
        };
        assert!(request_for(&pictured, "b", &seeing).is_some());

        // A request that already holds a tool round is not one plain answer.
        let mut worked = request.clone();
        worked["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role": "tool", "content": "x", "tool_call_id": "1"}));
        assert!(request_for(&worked, "b", &able).is_none());
    }

    #[test]
    fn settings_out_of_range_are_refused() {
        assert!(settings().check().is_ok());
        for bad in [
            Settings {
                close_gap: 1.5,
                ..settings()
            },
            Settings {
                close_gap: f64::NAN,
                ..settings()
            },
            Settings {
                sample_every: -1,
                ..settings()
            },
            Settings {
                sample_every: 1001,
                ..settings()
            },
            Settings {
                daily_limit: 0,
                ..settings()
            },
            Settings {
                daily_limit: 101,
                ..settings()
            },
        ] {
            assert!(bad.check().is_err());
        }
    }
}
