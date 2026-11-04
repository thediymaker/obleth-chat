//! Usage insights: what kind of work people do in Chat, without keeping what they said.
//! When a chat's first reply finishes, and again once a longer chat has been quiet for a
//! while, Chat asks a model about it as its owner: the kind of use, the activity, the
//! field, what the person wanted out and a few flags, in the verdicts format, and one
//! generic line naming the task. Only those labels are kept, never the messages, and a
//! chat keeps its labels when it is deleted, so short-lived chats still count. Personal
//! chats, chats flagged explicit or self-harm, and deleted chats keep no topic line.
//! Temporary chats are never read. The same pass can give a new chat a better title.
//!
//! Administrators see counts for a period and a group, and can have a model group the
//! topic lines into workstreams. A group with fewer than five people shows totals only.
use crate::{
    auth::{Identity, Role},
    error::ApiError,
    gateway::{valid_model, Gateway},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How long a chat that kept going stays quiet before it is labelled again.
const QUIET_SECS: f64 = 30.0 * 60.0;
/// Chats last active longer ago than this are left alone.
const LOOKBACK_DAYS: i32 = 30;
/// Chats one replica labels each minute, and at once.
const BATCH: i64 = 12;
const AT_ONCE: usize = 4;
const CLAIM_SECS: f64 = 600.0;
/// What the model reads: the person's first and latest messages, each one and all
/// together capped.
const MESSAGE_CHARS: usize = 1_500;
const CHAT_CHARS: usize = 8_000;
const TOPIC_CHARS: usize = 120;
const TITLE_CHARS: usize = 80;
/// A flag counts from this probability.
const FLAG_AT: f64 = 0.7;
/// A group with fewer people shows totals only, so nothing singles anyone out.
pub const MIN_PEOPLE: i64 = 5;
/// Topic lines the workstreams model reads, newest first, and topic lines listed per
/// workstream.
const REPORT_TOPICS: usize = 1_500;
const REPORT_ROWS: usize = 200;
const REPORT_AT_ONCE: usize = 8;
/// The workstream list is found again after this many days, or sooner when more than
/// `STREAMS_OTHER_SHARE` of recent chats fit none of it.
const STREAMS_EVERY_DAYS: i32 = 7;
const STREAMS_OTHER_SHARE: f64 = 0.15;
/// Distinct topic lines needed before the first list.
const STREAMS_MIN_TOPICS: i64 = 20;
/// Days of topic lines the list is found from and kept placed for.
const STREAMS_DAYS: i32 = 90;
/// Finding the list still running after this long died with its replica.
const STREAMS_CLAIM_MINUTES: i32 = 20;
/// Topic lines placed per pass.
const PLACE_BATCH: i64 = 40;

pub const KINDS: [&str; 6] = [
    "research",
    "learning",
    "work",
    "personal",
    "play",
    "trying_it_out",
];
pub const ACTIVITIES: [&str; 9] = [
    "coding",
    "data_analysis",
    "writing",
    "research",
    "explaining",
    "computing_help",
    "planning_admin",
    "creative",
    "other",
];
pub const FIELDS: [&str; 10] = [
    "computing",
    "engineering",
    "physical_sciences",
    "life_health",
    "social_sciences",
    "business",
    "humanities_arts",
    "education",
    "law_policy",
    "general",
];
pub const OUTPUTS: [&str; 7] = [
    "code", "document", "analysis", "answer", "creative", "plan", "none",
];
/// Flags the model answers, plus `secrets`, which Chat finds itself.
pub const FLAGS: [&str; 6] = [
    "seeks_harm",
    "explicit",
    "harassment_hate",
    "jailbreak",
    "integrity",
    "self_harm",
];
/// Chats with these flags keep no topic line.
const PRIVATE_FLAGS: [&str; 2] = ["explicit", "self_harm"];

/// The questions asked about every chat, in the verdicts format.
const QUESTIONS: &str = r#"{
 "use": {
  "type": "choice",
  "instructions": "What kind of use is this conversation, judging only from the messages?",
  "criteria": {
   "research": "Scholarly work: experiments, data analysis, papers, theses, grants, lab code or methods",
   "learning": "Coursework, studying or understanding a topic: homework, exam prep, explanations",
   "work": "Job tasks: administration, IT, teaching preparation, business, marketing, running a lab or unit",
   "personal": "Everyday life: shopping, health, fitness, travel, hobbies, advice, personal messages",
   "play": "Entertainment: stories, fan fiction, roleplay, jokes, games, idle chat",
   "trying_it_out": "Testing the assistant itself: greetings, asking what model it is, probing its limits"
  }
 },
 "activity": {
  "type": "choice",
  "instructions": "What is the main activity in these messages? Pick the activity, not the subject area.",
  "criteria": {
   "coding": "Writing, debugging, reviewing or explaining software, scripts, SQL or configuration",
   "data_analysis": "Statistics, plots, cleaning, modeling or interpreting a dataset or results",
   "writing": "Drafting, editing, translating or giving feedback on prose: papers, essays, email, reports, grants",
   "research": "Finding, reading, summarizing or comparing literature, sources or facts",
   "explaining": "Understanding a concept, a worked example or a topic being taught",
   "computing_help": "Using the cluster, Slurm, software installs, environments, storage, accounts or other IT help",
   "planning_admin": "Planning, scheduling, organizing, policies, forms, budgets or other office tasks",
   "creative": "Fiction, poetry, games, art prompts, brainstorming names or other creative play",
   "other": "Small talk, personal advice or anything that fits none of the above"
  }
 },
 "field": {
  "type": "choice",
  "instructions": "Which academic field does the subject matter belong to? Choose general when the messages have no particular field. Fiction, fan fiction, roleplay and games are general unless the person is studying literature or art.",
  "criteria": {
   "computing": "Computer science, software, data science, IT",
   "engineering": "Engineering of any kind",
   "physical_sciences": "Physics, chemistry, earth and space science, mathematics, statistics",
   "life_health": "Biology, medicine, nursing, public health, psychology research",
   "social_sciences": "Economics, sociology, political science, anthropology, communication",
   "business": "Business, management, finance, marketing, accounting",
   "humanities_arts": "History, literature, languages, philosophy, music, art, design",
   "education": "Education and teaching as a field of study",
   "law_policy": "Law, public policy, ethics and compliance",
   "general": "No particular academic field"
  }
 },
 "output_type": {
  "type": "choice",
  "instructions": "What is the person mainly trying to get out of the conversation?",
  "criteria": {
   "code": "Code, scripts, queries or configuration",
   "document": "Prose to use: an essay, email, report, article, abstract, post or translation",
   "analysis": "An analysis of data or results, a chart or a model",
   "answer": "An answer or explanation to understand something",
   "creative": "A story, poem, script, roleplay or other creative piece",
   "plan": "A plan, list, outline, schedule or set of ideas",
   "none": "Nothing in particular: greetings, chat or tests"
  }
 },
 "seeks_harm": {
  "type": "boolean",
  "instructions": "Is the person asking for help to harm others or break the law: malware, breaking into accounts or systems they don't own, weapons, drugs, fraud, stalking or harassment? Studying, defending against or discussing these topics is not seeking harm.",
  "criteria": {
   "true": "Asks for help causing harm or committing a crime",
   "false": "Ordinary, educational, defensive or authorized work"
  }
 },
 "explicit": {
  "type": "boolean",
  "instructions": "Is the person asking for sexual or sexually explicit content, including fetish content? Romance without sexual detail does not count.",
  "criteria": {
   "true": "Asks for sexual or explicit content",
   "false": "No sexual or explicit content requested"
  }
 },
 "harassment_hate": {
  "type": "boolean",
  "instructions": "Is the person asking for content that harasses, demeans or threatens a real person or a group because of who they are, such as hate speech or slurs?",
  "criteria": {
   "true": "Asks for harassing, hateful or threatening content",
   "false": "No such request"
  }
 },
 "jailbreak": {
  "type": "boolean",
  "instructions": "Is the person trying to get the assistant to drop its rules: DAN-style prompts, 'ignore previous instructions', or telling it to act as a model with no restrictions? Ordinary roleplay or persona requests do not count.",
  "criteria": {
   "true": "Tries to remove the assistant's rules",
   "false": "No attempt to remove its rules"
  }
 },
 "integrity": {
  "type": "boolean",
  "instructions": "Is the person asking the model to do graded coursework for a class so they can hand it in as their own: answering exam, quiz or homework questions, or writing a class assignment? Not this: editing or feedback on their own writing, explanations, hints, practice, and writing that is not a class assignment, such as theses, papers, grant proposals, applications, personal statements or work documents.",
  "criteria": {
   "true": "Wants class work done for them to submit",
   "false": "Anything else"
  }
 },
 "self_harm": {
  "type": "boolean",
  "instructions": "Does the person say they are thinking about hurting or killing themselves, or ask how to? Fiction and discussing the topic do not count.",
  "criteria": {
   "true": "Personal thoughts of or plans for self-harm",
   "false": "No personal self-harm"
  }
 }
}"#;

const TOPIC_PROMPT: &str = "You label AI-assistant conversations for an anonymous usage report. Reply with one line of at most 10 words that names the task as a workstream, like 'Debugging a Django data model migration' or 'Writing fan fiction crossover stories'. Be specific about the subject area and the kind of work, but never include names of people, organizations, places or products, any personal details, or quotes from the text. Reply with the line only.";

const NAMES_PROMPT: &str = "You name AI-assistant conversations. Reply with JSON only: {\"title\": \"...\", \"topic\": \"...\"}. title: a short, specific title for the person's own list of chats, 3 to 8 words, in the language the person wrote in, naming what they are working on; it may name the specific thing, such as a library, a dataset or a book. topic: at most 10 words naming the task as a workstream for an anonymous usage report, like 'Debugging a Django data model migration' or 'Writing fan fiction crossover stories'; specific about the subject area and the kind of work, but with no names of people, organizations, places or products, no personal details and no quotes from the text.";

const WORKSTREAMS_PROMPT: &str = "You design the workstream list for a usage report of an AI assistant. Given one-line descriptions of conversations, propose up to {max} workstreams that together cover nearly all of them. Each workstream gets a specific, plain name of 5 to 9 words in the style 'Migrating a tracking service off a legacy scheduler' or 'Writing fan fiction and crossover stories', and a one-sentence description of what belongs in it. Prefer groupings by the actual subject and kind of work over generic buckets. Reply with JSON only: [{\"name\": \"...\", \"description\": \"...\"}]";

const KEEP_PROMPT: &str = " The user message starts with the current list. Keep each current name that still fits, word for word, so the report stays comparable over time. Rename, merge or drop a workstream only when it no longer fits, and add new ones for work the list misses. Reply with the whole new list.";

pub fn questions() -> Value {
    serde_json::from_str(QUESTIONS).expect("the insight questions are valid JSON")
}

#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    /// Whether new chats get a title from the model.
    pub titles: bool,
    pub model: Option<String>,
    pub report_model: Option<String>,
}

pub async fn settings(pool: &PgPool) -> Result<Settings, sqlx::Error> {
    sqlx::query_as("SELECT enabled, titles, model, report_model FROM insights WHERE id")
        .fetch_one(pool)
        .await
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/insights", get(summary))
        .route("/api/admin/insights/settings", put(save))
        .route("/api/admin/insights/workstreams", post(refresh))
        .with_state(state)
}

/// Labels chats that went quiet after more messages, every minute while insights are on.
/// Replicas share the work by claiming each chat before asking about it.
pub async fn worker(s: AppState, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            _ = shutdown.cancelled() => break,
        }
        if let Err(error) = tick(&s).await {
            tracing::warn!("insights: {error}");
        }
        if let Err(error) = workstreams_tick(&s).await {
            tracing::warn!("insights workstreams: {error}");
        }
    }
}

/// One pass of the worker: the number of chats it labelled.
pub async fn tick(s: &AppState) -> Result<usize, sqlx::Error> {
    let Some(gateway) = s.gateway.clone() else {
        return Ok(0);
    };
    let settings = settings(&s.store.pool).await?;
    let (true, Some(model)) = (settings.enabled, settings.model) else {
        return Ok(0);
    };
    let pool = &s.store.pool;
    // A chat whose last message is newer than its labels, quiet for a while, not running
    // a reply and not temporary. A failed chat waits an hour more for each failure.
    let due: Vec<(Uuid, Uuid, DateTime<Utc>)> = sqlx::query_as(
        "SELECT c.id, c.owner_id, last.at FROM conversations c
         CROSS JOIN LATERAL (SELECT MAX(m.created_at) AS at FROM messages m WHERE m.conversation_id = c.id) last
         LEFT JOIN conversation_labels l ON l.conversation_id = c.id
         WHERE NOT c.temporary AND c.active_generation IS NULL
           AND c.updated_at > NOW() - make_interval(days => $2)
           AND last.at < NOW() - make_interval(secs => $1)
           AND (l.conversation_id IS NULL OR (
             (l.labeled_through IS NULL OR l.labeled_through < last.at)
             AND (l.claimed_until IS NULL OR l.claimed_until < NOW())
             AND (l.failed_at IS NULL OR l.failed_at < NOW() - make_interval(hours => LEAST(l.attempts, 24)))))
         ORDER BY last.at LIMIT $3",
    )
    .bind(QUIET_SECS)
    .bind(LOOKBACK_DAYS)
    .bind(BATCH)
    .fetch_all(pool)
    .await?;
    let mut claimed = Vec::new();
    for (id, owner, last) in due {
        if claim(pool, id, owner).await? {
            claimed.push((id, owner, last));
        }
    }
    let results: Vec<Result<(), sqlx::Error>> = futures_util::stream::iter(claimed)
        .map(|(id, owner, last)| {
            let gateway = gateway.clone();
            let model = model.clone();
            async move {
                let Some(user) = identity(pool, owner).await? else {
                    // An account that is gone: the chat is passed over until it has a new
                    // message.
                    sqlx::query("UPDATE conversation_labels SET claimed_until = NULL, labeled_through = $2 WHERE conversation_id = $1")
                        .bind(id)
                        .bind(last)
                        .execute(pool)
                        .await?;
                    return Ok(());
                };
                let work = Work { label: true, rename: None };
                settle(pool, id, describe(pool, &gateway, &model, id, &user, work).await).await
            }
        })
        .buffer_unordered(AT_ONCE)
        .collect()
        .await;
    let mut labelled = 0;
    for result in results {
        result?;
        labelled += 1;
    }
    Ok(labelled)
}

/// Claims a chat's labels for this replica; false when another one holds them.
async fn claim(pool: &PgPool, id: Uuid, owner: Uuid) -> Result<bool, sqlx::Error> {
    let mine: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO conversation_labels (conversation_id, owner_id, claimed_until)
         VALUES ($1, $2, NOW() + make_interval(secs => $3))
         ON CONFLICT (conversation_id) DO UPDATE SET claimed_until = EXCLUDED.claimed_until
         WHERE conversation_labels.claimed_until IS NULL OR conversation_labels.claimed_until < NOW()
         RETURNING conversation_id",
    )
    .bind(id)
    .bind(owner)
    .bind(CLAIM_SECS)
    .fetch_optional(pool)
    .await?;
    Ok(mine.is_some())
}

/// Records a failed answer, so the chat is tried again later.
async fn settle(pool: &PgPool, id: Uuid, done: Result<(), Failure>) -> Result<(), sqlx::Error> {
    match done {
        Ok(()) => Ok(()),
        Err(Failure::Database(error)) => Err(error),
        Err(Failure::Answer(why)) => {
            tracing::info!(conversation = %id, "insights could not label a chat: {why}");
            sqlx::query(
                "UPDATE conversation_labels SET claimed_until = NULL, failed_at = NOW(), attempts = attempts + 1
                 WHERE conversation_id = $1",
            )
            .bind(id)
            .execute(pool)
            .await
            .map(|_| ())
        }
    }
}

/// When a reply finishes: labels a chat that has none yet, and names a chat whose first
/// reply this is and whose title is still the start of its first message. Starts the work
/// and returns at once; true when a new title is on its way.
pub async fn after_reply(s: &AppState, user: &Identity, conversation: Uuid) -> bool {
    match tokio::time::timeout(Duration::from_secs(2), start(s, user, conversation)).await {
        Ok(Ok(pending)) => pending,
        Ok(Err(error)) => {
            tracing::warn!(conversation = %conversation, "insights after a reply: {error}");
            false
        }
        Err(_) => false,
    }
}

async fn start(s: &AppState, user: &Identity, conversation: Uuid) -> Result<bool, sqlx::Error> {
    let Some(gateway) = s.gateway.clone() else {
        return Ok(false);
    };
    let pool = &s.store.pool;
    let settings = settings(pool).await?;
    let (Some(model), true) = (settings.model.clone(), settings.enabled || settings.titles) else {
        return Ok(false);
    };
    let row: Option<(bool, String, Option<String>, bool, i64)> = sqlx::query_as(
        "SELECT c.temporary, c.title,
           (SELECT m.content FROM messages m WHERE m.conversation_id = c.id AND m.role = 'user' ORDER BY m.ordinal LIMIT 1),
           EXISTS (SELECT 1 FROM conversation_labels l WHERE l.conversation_id = c.id),
           (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id AND m.role = 'assistant' AND m.status = 'completed')
         FROM conversations c WHERE c.id = $1 AND c.owner_id = $2",
    )
    .bind(conversation)
    .bind(user.id)
    .fetch_optional(pool)
    .await?;
    let Some((temporary, title, first, labelled, replies)) = row else {
        return Ok(false);
    };
    if temporary {
        return Ok(false);
    }
    let rename = (settings.titles
        && replies == 1
        && first
            .as_deref()
            .is_some_and(|first| auto_title(&title, first)))
    .then_some(title);
    let label = settings.enabled && !labelled && claim(pool, conversation, user.id).await?;
    if !label && rename.is_none() {
        return Ok(false);
    }
    let pending = rename.is_some();
    let pool = pool.clone();
    let user = user.clone();
    tokio::spawn(async move {
        let work = Work { label, rename };
        let done = describe(&pool, &gateway, &model, conversation, &user, work).await;
        let settled = if label {
            settle(&pool, conversation, done).await
        } else {
            done.or_else(|f| match f {
                Failure::Database(error) => Err(error),
                Failure::Answer(_) => Ok(()),
            })
        };
        if let Err(error) = settled {
            tracing::warn!(conversation = %conversation, "insights after a reply: {error}");
        }
    });
    Ok(pending)
}

/// Whether a title is still the one Chat gave the chat: the start of its first message.
pub fn auto_title(title: &str, first: &str) -> bool {
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (title, first) = (words(title), words(first));
    !title.is_empty() && first.starts_with(&title)
}

/// What one pass does for a chat: label it, and name it in place of `rename`, the title
/// it had when the pass started.
struct Work {
    label: bool,
    rename: Option<String>,
}

enum Failure {
    Database(sqlx::Error),
    /// The gateway or the model gave no usable answer; the chat is tried again later.
    Answer(String),
}
impl From<sqlx::Error> for Failure {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

/// The labels for one chat, read from a verdicts answer.
#[derive(Debug, PartialEq)]
pub struct Labels {
    pub kind: String,
    pub activity: Option<String>,
    pub field: Option<String>,
    pub output: Option<String>,
    pub flags: Vec<String>,
}
impl Labels {
    /// None when the model gave no kind of use, the one label every chat needs.
    pub fn read(answer: &Value) -> Option<Self> {
        let picked = |id: &str, allowed: &[&str]| {
            answer["verdicts"][id]["value"]
                .as_str()
                .filter(|v| allowed.contains(v))
                .map(str::to_owned)
        };
        Some(Self {
            kind: picked("use", &KINDS)?,
            activity: picked("activity", &ACTIVITIES),
            field: picked("field", &FIELDS),
            output: picked("output_type", &OUTPUTS),
            flags: FLAGS
                .iter()
                .filter(|id| {
                    answer["verdicts"][**id]["probabilities"]["true"]
                        .as_f64()
                        .is_some_and(|p| p >= FLAG_AT)
                })
                .map(|id| id.to_string())
                .collect(),
        })
    }
    /// Whether the chat keeps no topic line: the person's own life, or flagged so.
    pub fn private(&self) -> bool {
        self.kind == "personal"
            || self
                .flags
                .iter()
                .any(|f| PRIVATE_FLAGS.contains(&f.as_str()))
    }
}

async fn describe(
    pool: &PgPool,
    gateway: &Gateway,
    model: &str,
    id: Uuid,
    user: &Identity,
    work: Work,
) -> Result<(), Failure> {
    let texts: Vec<String> = sqlx::query_scalar(
        "SELECT content FROM messages WHERE conversation_id = $1 AND role = 'user' ORDER BY ordinal",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let (last, tokens, chat_model): (Option<DateTime<Utc>>, i64, Option<String>) = sqlx::query_as(
        "SELECT MAX(created_at), COALESCE(SUM(CASE WHEN jsonb_typeof(usage->'total_tokens') = 'number'
           THEN (usage->>'total_tokens')::numeric ELSE 0 END), 0)::bigint,
           (SELECT g.model FROM messages m JOIN generation_usage g ON g.generation_id = m.generation_id
            WHERE m.conversation_id = $1 AND g.model IS NOT NULL
            GROUP BY g.model ORDER BY COALESCE(SUM(g.total_tokens), 0) DESC, COUNT(*) DESC, g.model LIMIT 1)
         FROM messages WHERE conversation_id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    let read = capped(&texts);
    if read.is_empty() {
        // Nothing to read: the chat is passed over until it has a new message.
        if work.label {
            sqlx::query("UPDATE conversation_labels SET claimed_until = NULL, labeled_through = $2 WHERE conversation_id = $1")
                .bind(id)
                .bind(last)
                .execute(pool)
                .await?;
        }
        return Ok(());
    }
    let labels = if work.label {
        let body = json!({
            "state": {"user_messages": read},
            "questions": questions(),
            "model": model,
            "on_no_answer": "low_confidence",
        });
        let answer = gateway
            .verdicts(user, &body, Duration::from_secs(120))
            .await
            .map_err(|e| Failure::Answer(e.1))?;
        let mut labels = Labels::read(&answer)
            .ok_or_else(|| Failure::Answer("the model gave no answer".into()))?;
        if texts.iter().any(|t| has_secret(t)) {
            labels.flags.push("secrets".into());
        }
        Some(labels)
    } else {
        None
    };
    let want_topic = labels.as_ref().is_some_and(|l| !l.private());
    let (title, topic) = if work.rename.is_some() || want_topic {
        let prompt = if work.rename.is_some() {
            NAMES_PROMPT
        } else {
            TOPIC_PROMPT
        };
        let body = json!({
            "model": model,
            "temperature": 0,
            "max_tokens": 120,
            "stream": false,
            "messages": [
                {"role": "system", "content": prompt},
                {"role": "user", "content": read.join("\n\n---\n\n")},
            ],
        });
        let answer = gateway
            .complete(user, &body, Duration::from_secs(120))
            .await
            .map_err(|e| Failure::Answer(e.1))?;
        if work.rename.is_some() {
            names(&answer)
        } else {
            (None, topic_line(&answer))
        }
    } else {
        (None, None)
    };
    if let (Some(old), Some(title)) = (&work.rename, &title) {
        // Only while the title is still the one the pass started from: a rename wins.
        sqlx::query("UPDATE conversations SET title = $2 WHERE id = $1 AND title = $3")
            .bind(id)
            .bind(title)
            .bind(old)
            .execute(pool)
            .await?;
    }
    let Some(labels) = labels else {
        return Ok(());
    };
    let topic = if want_topic { topic } else { None };
    sqlx::query(
        "UPDATE conversation_labels SET owner_id = $2, kind = $3, activity = $4, field = $5, output = $6,
           topic = $7, flags = $8, turns = $9, tokens = $10, model = $11, labeled_through = $12,
           chat_model = $13, labeled_at = NOW(), claimed_until = NULL, attempts = 0, failed_at = NULL,
           placed_at = NULL
         WHERE conversation_id = $1",
    )
    .bind(id)
    .bind(user.id)
    .bind(&labels.kind)
    .bind(&labels.activity)
    .bind(&labels.field)
    .bind(&labels.output)
    .bind(&topic)
    .bind(&labels.flags)
    .bind(texts.len() as i32)
    .bind(tokens)
    .bind(model.chars().take(200).collect::<String>())
    .bind(last)
    .bind(chat_model)
    .execute(pool)
    .await?;
    Ok(())
}

async fn identity(pool: &PgPool, user: Uuid) -> Result<Option<Identity>, sqlx::Error> {
    let row: Option<(Uuid, String, String, Role)> = sqlx::query_as(
        "SELECT id, username, display_name, role FROM users WHERE id = $1 AND enabled",
    )
    .bind(user)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, username, display_name, role)| Identity {
        id,
        username,
        display_name,
        role,
        // Insights run on their own, without a sign-in.
        session: None,
    }))
}

/// The person's messages as the model reads them: the first one and as many of the latest
/// as fit, in order, each one and all of them together cut to a few thousand characters.
pub fn capped(texts: &[String]) -> Vec<String> {
    let texts: Vec<&str> = texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    let Some((first, rest)) = texts.split_first() else {
        return Vec::new();
    };
    let cut = |text: &str, most: usize| text.chars().take(most).collect::<String>();
    let first = cut(first, MESSAGE_CHARS.min(CHAT_CHARS));
    let mut left = CHAT_CHARS - first.chars().count();
    let mut latest = Vec::new();
    for text in rest.iter().rev() {
        if left == 0 {
            break;
        }
        let part = cut(text, MESSAGE_CHARS.min(left));
        left -= part.chars().count();
        latest.push(part);
    }
    latest.reverse();
    std::iter::once(first).chain(latest).collect()
}

/// The title and topic line from a naming answer. A title is one line of a few words.
pub fn names(answer: &Value) -> (Option<String>, Option<String>) {
    let text = answer["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    let text = match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    };
    let (Some(open), Some(close)) = (text.find('{'), text.rfind('}')) else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_str::<Value>(&text[open..=close.max(open)]) else {
        return (None, None);
    };
    let clean = |key: &str, most: usize| {
        let line = value[key]
            .as_str()?
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())?;
        let line = line.trim_matches(|c: char| {
            c == '"' || c == '\'' || c == '*' || c == '`' || c == '.' || c.is_whitespace()
        });
        let line: String = line.chars().take(most).collect();
        (line.chars().count() >= 2).then_some(line)
    };
    (clean("title", TITLE_CHARS), clean("topic", TOPIC_CHARS))
}

/// The first line of a completion, without thinking, quotes or a final full stop.
pub fn topic_line(answer: &Value) -> Option<String> {
    let text = answer["choices"][0]["message"]["content"].as_str()?;
    let text = match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    };
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_matches(|c: char| {
        c == '"' || c == '\'' || c == '*' || c == '`' || c == '.' || c.is_whitespace()
    });
    let line: String = line.chars().take(TOPIC_CHARS).collect();
    (!line.is_empty()).then_some(line)
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Whether text holds what looks like a live secret: a provider key with a known prefix,
/// a private key block, a 32-byte hex key, or a long value given to a key, token,
/// secret or password.
pub fn has_secret(text: &str) -> bool {
    const PREFIXES: [&str; 10] = [
        "hf_",
        "sk-",
        "ghp_",
        "gho_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "glpat-",
        "AKIA",
    ];
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return true;
    }
    for prefix in PREFIXES {
        for (at, _) in text.match_indices(prefix) {
            let before_ok = text[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !token_char(c));
            let rest = text[at + prefix.len()..]
                .chars()
                .take_while(|c| token_char(*c))
                .count();
            if before_ok && rest >= 16 {
                return true;
            }
        }
    }
    for (at, _) in text.match_indices("0x") {
        let hex = text[at + 2..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        if hex == 64 {
            return true;
        }
    }
    let lower = text.to_ascii_lowercase();
    for name in [
        "api_key", "apikey", "api-key", "secret", "token", "password", "passwd",
    ] {
        for (at, _) in lower.match_indices(name) {
            let rest = &text[at + name.len()..];
            let mut chars = rest.chars().peekable();
            // A few name characters and spaces, then `:` or `=`.
            let mut skipped = 0;
            while let Some(c) = chars.peek() {
                if (c.is_ascii_alphanumeric() || *c == '_' || *c == ' ' || *c == '"' || *c == '\'')
                    && skipped < 12
                {
                    chars.next();
                    skipped += 1;
                } else {
                    break;
                }
            }
            match chars.next() {
                Some(':' | '=') => {}
                // `"X-Api-Key", "value"`: a comma counts only before a quoted value.
                Some(',') => {
                    while chars.peek() == Some(&' ') {
                        chars.next();
                    }
                    if !matches!(chars.peek(), Some('"' | '\'')) {
                        continue;
                    }
                }
                _ => continue,
            }
            let value: String = chars
                .skip_while(|c| *c == ' ' || *c == '"' || *c == '\'' || *c == '‘' || *c == '’')
                .take_while(|c| {
                    !c.is_whitespace() && !matches!(c, '"' | '\'' | '‘' | '’' | ',' | ';' | ')')
                })
                .collect();
            let looks_like_code = value.contains('(')
                || value.contains('.')
                    && value.chars().any(char::is_lowercase)
                    && !value.chars().any(|c| c.is_ascii_digit());
            if value.chars().count() >= 12 && !looks_like_code {
                return true;
            }
        }
    }
    false
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    days: Option<i32>,
    group: Option<Uuid>,
}
fn period(days: Option<i32>) -> Result<i32, ApiError> {
    let days = days.unwrap_or(28);
    if !(1..=365).contains(&days) {
        return Err(ApiError::bad("Choose a period of 1 to 365 days"));
    }
    Ok(days)
}

/// Labelled chats whose last message falls in the period, for everyone or a group's
/// members. $1 is the days, $2 the group.
const IN_SCOPE: &str = "l.labeled_at IS NOT NULL AND l.kind IS NOT NULL
    AND l.labeled_through >= NOW() - make_interval(days => $1)
    AND ($2::uuid IS NULL OR l.owner_id IN (SELECT user_id FROM user_group_members WHERE group_id = $2))";

async fn counts(
    pool: &PgPool,
    column: &str,
    days: i32,
    group: Option<Uuid>,
) -> Result<Vec<Value>, sqlx::Error> {
    let rows: Vec<(Option<String>, i64, i64)> = sqlx::query_as(&format!(
        "SELECT l.{column}, COUNT(*), COALESCE(SUM(l.tokens), 0)::bigint FROM conversation_labels l
         WHERE {IN_SCOPE} GROUP BY l.{column} ORDER BY COUNT(*) DESC, l.{column}"
    ))
    .bind(days)
    .bind(group)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, sessions, tokens)| json!({"name": name.unwrap_or_else(|| "unknown".into()), "sessions": sessions, "tokens": tokens}))
        .collect())
}

/// Where finding the workstream list is: none yet, building, ready or failed.
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct Streams {
    status: String,
    building: bool,
    model: Option<String>,
    error: Option<String>,
    found_at: Option<DateTime<Utc>>,
    /// Whether an administrator runs finding and placing; without one, nothing runs.
    runs: bool,
}
const STREAMS_COLUMNS: &str = "streams_status AS status,
    (streams_status = 'building' AND streams_claimed_until > NOW()) AS building,
    streams_model AS model, streams_error AS error, streams_found_at AS found_at, run_as IS NOT NULL AS runs";

/// The shared workstream list counted for a period and group.
async fn streams_summary(
    pool: &PgPool,
    days: i32,
    group: Option<Uuid>,
) -> Result<Value, sqlx::Error> {
    let state: Streams =
        sqlx::query_as(&format!("SELECT {STREAMS_COLUMNS} FROM insights WHERE id"))
            .fetch_one(pool)
            .await?;
    let streams: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, name, description FROM workstreams WHERE retired_at IS NULL ORDER BY created_at, name",
    )
    .fetch_all(pool)
    .await?;
    let rows: Vec<Labelled> = sqlx::query_as(&format!(
        "SELECT l.topic, l.kind, l.output, l.field, l.turns, l.tokens, l.workstream_id, l.placed_at IS NOT NULL AS placed
         FROM conversation_labels l WHERE {IN_SCOPE} ORDER BY l.labeled_through DESC LIMIT 20000"
    ))
    .bind(days)
    .bind(group)
    .fetch_all(pool)
    .await?;
    let waiting = rows
        .iter()
        .filter(|r| r.topic.is_some() && !r.placed)
        .count();
    let list = if streams.is_empty() {
        json!([])
    } else {
        workstreams(&rows, &streams)
    };
    Ok(json!({"state": state, "waiting": waiting, "list": list}))
}

/// Counts for a period and a group: chats, people, tokens, each label and each flag, and
/// the latest workstreams report. Never a chat, a message or a person.
async fn summary(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(scope): Query<Scope>,
) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    let days = period(scope.days)?;
    let group = scope.group;
    let pool = &s.store.pool;
    let settings = settings(pool).await?;
    let groups: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM user_groups ORDER BY name")
            .fetch_all(pool)
            .await?;
    if group.is_some_and(|g| !groups.iter().any(|(id, _)| *id == g)) {
        return Err(ApiError::not_found());
    }
    let (sessions, people, tokens): (i64, i64, i64) = sqlx::query_as(&format!(
        "SELECT COUNT(*), COUNT(DISTINCT l.owner_id), COALESCE(SUM(l.tokens), 0)::bigint
         FROM conversation_labels l WHERE {IN_SCOPE}"
    ))
    .bind(days)
    .bind(group)
    .fetch_one(pool)
    .await?;
    let last_labeled: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT MAX(labeled_at) FROM conversation_labels")
            .fetch_one(pool)
            .await?;
    let mut out = json!({
        "settings": settings,
        "gateway": s.gateway.is_some(),
        "groups": groups.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        "days": days,
        "group": group,
        "sessions": sessions,
        "people": people,
        "tokens": tokens,
        "lastLabeledAt": last_labeled,
        "minPeople": MIN_PEOPLE,
        "tooFew": group.is_some() && people < MIN_PEOPLE,
    });
    if out["tooFew"] == json!(true) {
        return Ok(Json(out));
    }
    for (key, column) in [
        ("kinds", "kind"),
        ("activities", "activity"),
        ("fields", "field"),
        ("outputs", "output"),
    ] {
        out[key] = json!(counts(pool, column, days, group).await?);
    }
    let flags: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT f, COUNT(*) FROM conversation_labels l, unnest(l.flags) AS f WHERE {IN_SCOPE} GROUP BY f"
    ))
    .bind(days)
    .bind(group)
    .fetch_all(pool)
    .await?;
    out["flags"] = json!(flags.into_iter().collect::<BTreeMap<_, _>>());
    out["workstreams"] = streams_summary(pool, days, group).await?;
    // The period before, for the change in chats, people and tokens.
    let (sessions, people, tokens): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(DISTINCT l.owner_id), COALESCE(SUM(l.tokens), 0)::bigint FROM conversation_labels l
         WHERE l.labeled_at IS NOT NULL AND l.kind IS NOT NULL
           AND l.labeled_through >= NOW() - make_interval(days => 2 * $1) AND l.labeled_through < NOW() - make_interval(days => $1)
           AND ($2::uuid IS NULL OR l.owner_id IN (SELECT user_id FROM user_group_members WHERE group_id = $2))",
    )
    .bind(days)
    .bind(group)
    .fetch_one(pool)
    .await?;
    out["previous"] = json!({"sessions": sessions, "people": people, "tokens": tokens});
    // One chat can hold most of a period's tokens, which flattens every token share.
    let largest: Option<i64> = sqlx::query_scalar(&format!(
        "SELECT MAX(l.tokens) FROM conversation_labels l WHERE {IN_SCOPE}"
    ))
    .bind(days)
    .bind(group)
    .fetch_one(pool)
    .await?;
    out["largestChat"] = json!(largest.unwrap_or(0));
    // The kind of use by day for short periods, by week for longer ones.
    let unit = if days <= 14 { "day" } else { "week" };
    let trend: Vec<(String, String, i64)> = sqlx::query_as(&format!(
        "SELECT to_char(date_trunc($3, l.labeled_through AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS at, l.kind, COUNT(*)
         FROM conversation_labels l WHERE {IN_SCOPE} GROUP BY at, l.kind ORDER BY at"
    ))
    .bind(days)
    .bind(group)
    .bind(unit)
    .fetch_all(pool)
    .await?;
    let mut points: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for (at, kind, n) in trend {
        points.entry(at).or_default().insert(kind, n);
    }
    out["trend"] = json!({"unit": unit, "points": points.into_iter().map(|(at, kinds)| json!({"at": at, "kinds": kinds})).collect::<Vec<_>>()});
    Ok(Json(out))
}

/// Labelled chats since `start`, for a group and the model the chats mostly used.
const MIX_SCOPE: &str = "l.labeled_at IS NOT NULL AND l.kind IS NOT NULL AND l.labeled_through >= $1
    AND ($2::uuid IS NULL OR l.owner_id IN (SELECT user_id FROM user_group_members WHERE group_id = $2))
    AND ($3::text IS NULL OR l.chat_model = $3)";
/// Models shown in what each model's tokens went to.
const MIX_MODELS: usize = 8;

/// What usage went to, for the usage page: the kind of use by chats and tokens, split by
/// the model the chats mostly used, and for the people who used the most tokens against
/// everyone else. Counts only, and only for groups of at least `MIN_PEOPLE`.
pub async fn usage_mix(
    pool: &PgPool,
    start: DateTime<Utc>,
    group: Option<Uuid>,
    model: Option<&str>,
) -> Result<Value, sqlx::Error> {
    if !settings(pool).await?.enabled {
        return Ok(json!({"enabled": false}));
    }
    let (sessions, people, tokens): (i64, i64, i64) = sqlx::query_as(&format!(
        "SELECT COUNT(*), COUNT(DISTINCT l.owner_id), COALESCE(SUM(l.tokens), 0)::bigint
         FROM conversation_labels l WHERE {MIX_SCOPE}"
    ))
    .bind(start)
    .bind(group)
    .bind(model)
    .fetch_one(pool)
    .await?;
    let mut out = json!({"enabled": true, "sessions": sessions, "people": people, "tokens": tokens, "minPeople": MIN_PEOPLE});
    if group.is_some() && people < MIN_PEOPLE {
        out["tooFew"] = json!(true);
        return Ok(out);
    }
    let rows: Vec<(Option<String>, String, i64, i64)> = sqlx::query_as(&format!(
        "SELECT l.chat_model, l.kind, COUNT(*), COALESCE(SUM(l.tokens), 0)::bigint
         FROM conversation_labels l WHERE {MIX_SCOPE} GROUP BY l.chat_model, l.kind"
    ))
    .bind(start)
    .bind(group)
    .bind(model)
    .fetch_all(pool)
    .await?;
    // Chats and tokens per kind of use.
    type Kinds = BTreeMap<String, (i64, i64)>;
    let mut kinds = Kinds::new();
    let mut models: BTreeMap<Option<String>, (i64, i64, Kinds)> = BTreeMap::new();
    for (chat_model, kind, n, t) in rows {
        let total = kinds.entry(kind.clone()).or_default();
        *total = (total.0 + n, total.1 + t);
        let entry = models.entry(chat_model).or_default();
        entry.0 += n;
        entry.1 += t;
        entry.2.insert(kind, (n, t));
    }
    let split = |kinds: &Kinds| {
        kinds
            .iter()
            .map(|(kind, (n, t))| json!({"name": kind, "sessions": n, "tokens": t}))
            .collect::<Vec<_>>()
    };
    out["kinds"] = json!(split(&kinds));
    let mut models: Vec<_> = models.into_iter().collect();
    models.sort_by_key(|(name, (n, t, _))| {
        (std::cmp::Reverse(*t), std::cmp::Reverse(*n), name.clone())
    });
    out["byModel"] = json!(models
        .iter()
        .take(MIX_MODELS)
        .map(|(name, (n, t, kinds))| json!({"model": name, "sessions": n, "tokens": t, "kinds": split(kinds)}))
        .collect::<Vec<_>>());
    // The people who used the most: a tenth of everyone, and at least MIN_PEOPLE on each side.
    let heavy = (people as usize).div_ceil(10).max(MIN_PEOPLE as usize) as i64;
    if people - heavy >= MIN_PEOPLE {
        let sides: Vec<(bool, String, i64, i64, i64)> = sqlx::query_as(&format!(
            "WITH ranked AS (SELECT l.owner_id, ROW_NUMBER() OVER (ORDER BY SUM(l.tokens) DESC, l.owner_id) AS place
               FROM conversation_labels l WHERE {MIX_SCOPE} GROUP BY l.owner_id)
             SELECT r.place <= $4, l.kind, COUNT(*), COALESCE(SUM(l.tokens), 0)::bigint, COUNT(DISTINCT l.owner_id)
             FROM conversation_labels l JOIN ranked r ON r.owner_id = l.owner_id
             WHERE {MIX_SCOPE} GROUP BY 1, 2"
        ))
        .bind(start)
        .bind(group)
        .bind(model)
        .bind(heavy)
        .fetch_all(pool)
        .await?;
        let side = |top: bool| {
            let rows: Vec<_> = sides.iter().filter(|s| s.0 == top).collect();
            json!({
                "tokens": rows.iter().map(|s| s.3).sum::<i64>(),
                "kinds": rows.iter().map(|s| json!({"name": s.1, "sessions": s.2, "tokens": s.3})).collect::<Vec<_>>(),
            })
        };
        let (mut top, mut rest) = (side(true), side(false));
        top["people"] = json!(heavy);
        rest["people"] = json!(people - heavy);
        out["heavy"] = json!({"top": top, "rest": rest});
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Patch {
    enabled: bool,
    titles: bool,
    model: Option<String>,
    report_model: Option<String>,
}

fn model_name(value: Option<String>) -> Result<Option<String>, ApiError> {
    let value = value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    if value.as_deref().is_some_and(|v| !valid_model(v)) {
        return Err(ApiError::bad("Enter a model name of up to 160 characters"));
    }
    Ok(value)
}

async fn save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(patch): Json<Patch>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let model = model_name(patch.model)?;
    let report_model = model_name(patch.report_model)?;
    if (patch.enabled || patch.titles) && model.is_none() {
        return Err(ApiError::bad(
            "Choose the model that labels and names chats",
        ));
    }
    if (patch.enabled || patch.titles) && s.gateway.is_none() {
        return Err(ApiError::bad(
            "Insights ask models through the gateway; connect one first",
        ));
    }
    let mut tx = s.store.pool.begin().await?;
    let saved: Settings = sqlx::query_as(
        "UPDATE insights SET enabled = $1, titles = $4, model = $2, report_model = $3, run_as = $5 WHERE id
         RETURNING enabled, titles, model, report_model",
    )
    .bind(patch.enabled)
    .bind(&model)
    .bind(&report_model)
    .bind(patch.titles)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,metadata)VALUES($1,$2,'insights.update','site',$3)")
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(json!({"enabled": saved.enabled, "titles": saved.titles, "model": saved.model, "reportModel": saved.report_model}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({ "settings": saved })))
}

/// Asks for the workstream list to be found again now, as the administrator who asked.
async fn refresh(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let pool = &s.store.pool;
    if !settings(pool).await?.enabled {
        return Err(ApiError::bad("Turn insights on first"));
    }
    if s.gateway.is_none() {
        return Err(ApiError::bad(
            "Insights ask models through the gateway; connect one first",
        ));
    }
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE insights SET streams_requested = TRUE, run_as = $1 WHERE id")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,metadata)VALUES($1,$2,'insights.report','site','{}')")
        .bind(Uuid::new_v4())
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let started = s.clone();
    tokio::spawn(async move {
        if let Err(error) = workstreams_tick(&started).await {
            tracing::warn!("insights workstreams: {error}");
        }
    });
    let state: Streams =
        sqlx::query_as(&format!("SELECT {STREAMS_COLUMNS} FROM insights WHERE id"))
            .fetch_one(pool)
            .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "state": state }))))
}

#[derive(FromRow)]
struct Labelled {
    topic: Option<String>,
    kind: String,
    output: Option<String>,
    field: Option<String>,
    turns: i32,
    tokens: i64,
    workstream_id: Option<Uuid>,
    placed: bool,
}

/// Workstream names and descriptions from a model's answer.
pub fn read_workstreams(text: &str) -> Option<Vec<(String, String)>> {
    let text = match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    };
    let list: Value = serde_json::from_str(&text[text.find('[')?..=text.rfind(']')?]).ok()?;
    let mut out: Vec<(String, String)> = Vec::new();
    for item in list.as_array()? {
        let name: String = item["name"]
            .as_str()
            .unwrap_or("")
            .trim()
            .chars()
            .take(120)
            .collect();
        let description: String = item["description"]
            .as_str()
            .unwrap_or("")
            .trim()
            .chars()
            .take(300)
            .collect();
        if !name.is_empty() && !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            out.push((name, description));
        }
        if out.len() == 20 {
            break;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// What decides whether the workstream list is found again.
#[derive(Debug, Clone, Copy, FromRow)]
pub struct StreamsDue {
    pub requested: bool,
    pub building: bool,
    pub failed: bool,
    pub tried_at: Option<DateTime<Utc>>,
    pub found_at: Option<DateTime<Utc>>,
    /// Workstreams in the current list.
    pub current: i64,
    /// Distinct topic lines in the last `STREAMS_DAYS`.
    pub topics: i64,
    /// Chats placed in the last 30 days, and those of them that fit none.
    pub placed: i64,
    pub other: i64,
}

impl StreamsDue {
    /// Asked for; or, with enough topics, no list yet, a list a week old, or one that more
    /// than `STREAMS_OTHER_SHARE` of recent chats fit none of. A failed try waits six hours.
    pub fn due(&self, now: DateTime<Utc>) -> bool {
        let older = |at: Option<DateTime<Utc>>, hours: i64| {
            at.is_none_or(|at| now - at > chrono::Duration::hours(hours))
        };
        if self.building {
            return false;
        }
        if self.requested {
            return true;
        }
        if self.topics < STREAMS_MIN_TOPICS || (self.failed && !older(self.tried_at, 6)) {
            return false;
        }
        self.current == 0
            || older(self.found_at, 24 * i64::from(STREAMS_EVERY_DAYS))
            || (self.placed >= 2 * STREAMS_MIN_TOPICS
                && self.other as f64 > STREAMS_OTHER_SHARE * self.placed as f64
                && older(self.found_at, 24))
    }
}

/// Finds the workstream list when it is due, and places chats that wait for a place. Runs
/// every minute while insights are on; a claim on the settings row keeps replicas apart.
pub async fn workstreams_tick(s: &AppState) -> Result<(), sqlx::Error> {
    let Some(gateway) = s.gateway.clone() else {
        return Ok(());
    };
    let pool = &s.store.pool;
    let settings = settings(pool).await?;
    let (true, Some(model)) = (settings.enabled, settings.model.clone()) else {
        return Ok(());
    };
    let run_as: Option<Uuid> = sqlx::query_scalar("SELECT run_as FROM insights WHERE id")
        .fetch_one(pool)
        .await?;
    let Some(user) = (match run_as {
        Some(id) => identity(pool, id).await?,
        None => None,
    }) else {
        return Ok(());
    };
    let due: StreamsDue = sqlx::query_as(
        "SELECT i.streams_requested AS requested,
           (i.streams_status = 'building' AND i.streams_claimed_until > NOW()) AS building,
           i.streams_status = 'failed' AS failed, i.streams_tried_at AS tried_at, i.streams_found_at AS found_at,
           (SELECT COUNT(*) FROM workstreams WHERE retired_at IS NULL) AS current,
           (SELECT COUNT(DISTINCT topic) FROM conversation_labels
              WHERE topic IS NOT NULL AND labeled_through >= NOW() - make_interval(days => $1)) AS topics,
           (SELECT COUNT(*) FROM conversation_labels
              WHERE placed_at IS NOT NULL AND topic IS NOT NULL AND labeled_through >= NOW() - INTERVAL '30 days') AS placed,
           (SELECT COUNT(*) FROM conversation_labels
              WHERE placed_at IS NOT NULL AND topic IS NOT NULL AND workstream_id IS NULL
                AND labeled_through >= NOW() - INTERVAL '30 days') AS other
         FROM insights i WHERE i.id",
    )
    .bind(STREAMS_DAYS)
    .fetch_one(pool)
    .await?;
    if due.due(Utc::now()) {
        let finder = settings
            .report_model
            .clone()
            .unwrap_or_else(|| model.clone());
        let claimed = sqlx::query(
            "UPDATE insights SET streams_status = 'building', streams_requested = FALSE, streams_error = NULL,
               streams_claimed_until = NOW() + make_interval(mins => $1), streams_tried_at = NOW(), streams_model = $2
             WHERE id AND NOT (streams_status = 'building' AND streams_claimed_until > NOW())",
        )
        .bind(STREAMS_CLAIM_MINUTES)
        .bind(finder.chars().take(200).collect::<String>())
        .execute(pool)
        .await?
        .rows_affected()
            == 1;
        if claimed {
            let pool = pool.clone();
            let gateway = gateway.clone();
            let user = user.clone();
            tokio::spawn(async move {
                let found = find_workstreams(&pool, &gateway, &user, &finder).await;
                let saved = match found {
                    Ok(()) => Ok(()),
                    Err(error) => sqlx::query(
                        "UPDATE insights SET streams_status = 'failed', streams_error = $1, streams_claimed_until = NULL WHERE id",
                    )
                    .bind(error.chars().take(500).collect::<String>())
                    .execute(&pool)
                    .await
                    .map(|_| ()),
                };
                if let Err(error) = saved {
                    tracing::warn!("insights workstreams could not be saved: {error}");
                }
            });
            return Ok(());
        }
    }
    place(pool, &gateway, &user, &model).await
}

/// Asks the workstreams model for the list, keeping the current names that still fit, then
/// saves it and sends every recent chat to be placed again.
async fn find_workstreams(
    pool: &PgPool,
    gateway: &Gateway,
    user: &Identity,
    model: &str,
) -> Result<(), String> {
    let failed = |e: sqlx::Error| {
        tracing::warn!("insights workstreams: {e}");
        "The workstreams could not be saved".to_string()
    };
    let topics: Vec<String> = sqlx::query_scalar(
        "SELECT topic FROM conversation_labels WHERE topic IS NOT NULL AND labeled_through >= NOW() - make_interval(days => $1)
         GROUP BY topic ORDER BY MAX(labeled_through) DESC LIMIT $2",
    )
    .bind(STREAMS_DAYS)
    .bind(REPORT_TOPICS as i64)
    .fetch_all(pool)
    .await
    .map_err(failed)?;
    if topics.is_empty() {
        return Err("No chats have topic lines yet".into());
    }
    let current: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, description FROM workstreams WHERE retired_at IS NULL ORDER BY created_at, name",
    )
    .fetch_all(pool)
    .await
    .map_err(failed)?;
    let max = (topics.len() / 8).clamp(2, 18);
    let mut prompt = WORKSTREAMS_PROMPT.replace("{max}", &max.to_string());
    let mut content = String::new();
    if !current.is_empty() {
        prompt.push_str(KEEP_PROMPT);
        content.push_str("Current workstreams:\n");
        for (name, description) in &current {
            content.push_str(&format!("- {name}: {description}\n"));
        }
        content.push_str("\nConversations:\n");
    }
    content.push_str(
        &topics
            .iter()
            .map(|t| format!("- {t}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let body = json!({
        "model": model,
        "temperature": 0,
        "max_tokens": 16000,
        "stream": false,
        "messages": [{"role": "system", "content": prompt}, {"role": "user", "content": content}],
    });
    let answer = gateway
        .complete(user, &body, Duration::from_secs(900))
        .await
        .map_err(|e| e.1)?;
    let text = answer["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    let streams = read_workstreams(text).ok_or(
        "The model did not answer with a list of workstreams. Try again, or choose another workstreams model.",
    )?;
    let mut tx = pool.begin().await.map_err(failed)?;
    let mut kept: Vec<Uuid> = Vec::new();
    for (name, description) in &streams {
        let id: Option<Uuid> = sqlx::query_scalar(
            "UPDATE workstreams SET description = $2 WHERE retired_at IS NULL AND LOWER(name) = LOWER($1) RETURNING id",
        )
        .bind(name)
        .bind(description)
        .fetch_optional(&mut *tx)
        .await
        .map_err(failed)?;
        kept.push(match id {
            Some(id) => id,
            None => {
                let id = Uuid::new_v4();
                sqlx::query("INSERT INTO workstreams (id, name, description) VALUES ($1, $2, $3)")
                    .bind(id)
                    .bind(name)
                    .bind(description)
                    .execute(&mut *tx)
                    .await
                    .map_err(failed)?;
                id
            }
        });
    }
    sqlx::query(
        "UPDATE workstreams SET retired_at = NOW() WHERE retired_at IS NULL AND id <> ALL($1)",
    )
    .bind(&kept)
    .execute(&mut *tx)
    .await
    .map_err(failed)?;
    sqlx::query("UPDATE conversation_labels SET placed_at = NULL WHERE labeled_through >= NOW() - make_interval(days => $1)")
        .bind(STREAMS_DAYS)
        .execute(&mut *tx)
        .await
        .map_err(failed)?;
    sqlx::query(
        "UPDATE insights SET streams_status = 'ready', streams_found_at = NOW(), streams_claimed_until = NULL WHERE id",
    )
    .execute(&mut *tx)
    .await
    .map_err(failed)?;
    tx.commit().await.map_err(failed)
}

/// Places up to `PLACE_BATCH` topic lines that wait for a place, each in one workstream or
/// in none, asking the labelling model through verdicts.
async fn place(
    pool: &PgPool,
    gateway: &Gateway,
    user: &Identity,
    model: &str,
) -> Result<(), sqlx::Error> {
    let claimed = sqlx::query(
        "UPDATE insights SET placing_until = NOW() + INTERVAL '5 minutes'
         WHERE id AND (placing_until IS NULL OR placing_until < NOW())
           AND NOT (streams_status = 'building' AND streams_claimed_until > NOW())",
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(());
    }
    let placed = place_claimed(pool, gateway, user, model).await;
    sqlx::query("UPDATE insights SET placing_until = NULL WHERE id")
        .execute(pool)
        .await?;
    placed
}

async fn place_claimed(
    pool: &PgPool,
    gateway: &Gateway,
    user: &Identity,
    model: &str,
) -> Result<(), sqlx::Error> {
    let streams: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, name, description FROM workstreams WHERE retired_at IS NULL ORDER BY created_at, name",
    )
    .fetch_all(pool)
    .await?;
    if streams.is_empty() {
        return Ok(());
    }
    let topics: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT topic FROM conversation_labels
         WHERE placed_at IS NULL AND topic IS NOT NULL AND labeled_at IS NOT NULL
           AND labeled_through >= NOW() - make_interval(days => $1)
         LIMIT $2",
    )
    .bind(STREAMS_DAYS)
    .bind(PLACE_BATCH)
    .fetch_all(pool)
    .await?;
    let mut criteria = serde_json::Map::new();
    for (i, (_, name, description)) in streams.iter().enumerate() {
        criteria.insert(format!("w{i:02}"), json!(format!("{name}: {description}")));
    }
    criteria.insert("other".into(), json!("None of the above"));
    let questions = json!({"workstream": {"type": "choice", "instructions": "Which workstream does this chat belong to, judging from its one-line topic?", "criteria": criteria}});
    let mut answers = futures_util::stream::iter(topics)
        .map(|topic| {
            let body = json!({"state": {"topic": topic}, "questions": questions, "model": model, "on_no_answer": "low_confidence"});
            async move {
                let picked = gateway
                    .verdicts(user, &body, Duration::from_secs(120))
                    .await
                    .ok()
                    .and_then(|a| a["verdicts"]["workstream"]["value"].as_str().map(str::to_owned));
                (topic, picked)
            }
        })
        .buffer_unordered(REPORT_AT_ONCE);
    while let Some((topic, picked)) = answers.next().await {
        // No answer: the topic waits for the next pass.
        let Some(picked) = picked else { continue };
        let stream = picked
            .strip_prefix('w')
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|i| streams.get(i))
            .map(|s| s.0);
        sqlx::query("UPDATE conversation_labels SET workstream_id = $2, placed_at = NOW() WHERE topic = $1 AND placed_at IS NULL")
            .bind(&topic)
            .bind(stream)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Each workstream with its share of chats and tokens, the kinds of use and outputs in it,
/// and its topic lines. Chats in no workstream, or still waiting for one, are "Everything
/// else"; chats without a topic line and no workstream are counted together, never listed.
fn workstreams(rows: &[Labelled], streams: &[(Uuid, String, String)]) -> Value {
    #[derive(Default)]
    struct Bucket<'a> {
        sessions: i64,
        tokens: i64,
        kinds: BTreeMap<&'a str, i64>,
        outputs: BTreeMap<&'a str, i64>,
        rows: Vec<&'a Labelled>,
    }
    let other = streams.len();
    let unnamed = other + 1;
    let mut buckets: Vec<Bucket> = (0..=unnamed).map(|_| Bucket::default()).collect();
    for row in rows {
        let current = row
            .workstream_id
            .and_then(|id| streams.iter().position(|s| s.0 == id));
        let at = match (current, row.topic.is_some()) {
            (Some(i), _) => i,
            (None, true) => other,
            (None, false) => unnamed,
        };
        let b = &mut buckets[at];
        b.sessions += 1;
        b.tokens += row.tokens;
        *b.kinds.entry(&row.kind).or_default() += 1;
        *b.outputs
            .entry(row.output.as_deref().unwrap_or("unknown"))
            .or_default() += 1;
        if row.topic.is_some() {
            b.rows.push(row);
        }
    }
    let mut out: Vec<Value> = buckets
        .into_iter()
        .enumerate()
        .filter(|(_, b)| b.sessions > 0)
        .map(|(i, mut b)| {
            let (name, description, private) = if i < other {
                (streams[i].1.clone(), streams[i].2.clone(), false)
            } else if i == other {
                ("Everything else".to_string(), "Chats that fit none of the workstreams, or are still being placed".to_string(), false)
            } else {
                (
                    "Personal and unlisted chats".to_string(),
                    "Chats about the person's own life, flagged explicit or self-harm, deleted, or without a topic line. Counted, never listed.".to_string(),
                    true,
                )
            };
            b.rows.sort_by_key(|r| std::cmp::Reverse(r.tokens));
            json!({
                "name": name,
                "description": description,
                "private": private,
                "sessions": b.sessions,
                "tokens": b.tokens,
                "kinds": b.kinds,
                "outputs": b.outputs,
                "rows": b.rows.iter().take(REPORT_ROWS).map(|r| json!({
                    "topic": r.topic, "kind": r.kind, "output": r.output, "field": r.field, "turns": r.turns, "tokens": r.tokens,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b["sessions"]
            .as_i64()
            .cmp(&a["sessions"].as_i64())
            .then(b["tokens"].as_i64().cmp(&a["tokens"].as_i64()))
    });
    json!(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_cover_every_label_and_flag() {
        let q = questions();
        for id in ["use", "activity", "field", "output_type"]
            .iter()
            .chain(FLAGS.iter())
        {
            assert!(q[*id]["instructions"].is_string(), "{id}");
        }
        let options = |id: &str| {
            let mut keys: Vec<String> = q[id]["criteria"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        };
        let sorted = |list: &[&str]| {
            let mut v: Vec<String> = list.iter().map(|s| s.to_string()).collect();
            v.sort();
            v
        };
        assert_eq!(options("use"), sorted(&KINDS));
        assert_eq!(options("activity"), sorted(&ACTIVITIES));
        assert_eq!(options("field"), sorted(&FIELDS));
        assert_eq!(options("output_type"), sorted(&OUTPUTS));
    }

    #[test]
    fn labels_read_values_and_flags_from_probability() {
        let answer = json!({"verdicts": {
            "use": {"value": "personal"}, "activity": {"value": "writing"}, "field": {"value": "nonsense"},
            "output_type": {"value": "document"},
            "explicit": {"value": true, "probabilities": {"true": 0.71, "false": 0.29}},
            "integrity": {"value": true, "probabilities": {"true": 0.6, "false": 0.4}},
        }});
        let labels = Labels::read(&answer).unwrap();
        assert_eq!(labels.kind, "personal");
        assert_eq!(labels.activity.as_deref(), Some("writing"));
        assert_eq!(labels.field, None);
        assert_eq!(labels.flags, vec!["explicit"]);
        assert!(labels.private());
        assert!(Labels::read(&json!({"verdicts": {"use": {"value": "goofing"}}})).is_none());
        let work = Labels::read(&json!({"verdicts": {"use": {"value": "work"}}})).unwrap();
        assert!(!work.private());
    }

    #[test]
    fn the_model_reads_the_first_and_latest_messages() {
        let long = |c: &str| c.repeat(5_000);
        let texts = vec![
            "  ".into(),
            long("a"),
            long("b"),
            long("c"),
            long("d"),
            long("e"),
            long("f"),
            long("g"),
            long("h"),
            "late".into(),
        ];
        let read = capped(&texts);
        assert!(read.iter().all(|t| t.chars().count() <= MESSAGE_CHARS));
        assert_eq!(
            read.iter().map(|t| t.chars().count()).sum::<usize>(),
            CHAT_CHARS
        );
        assert!(read[0].starts_with('a'));
        assert_eq!(read.last().unwrap(), "late");
        assert!(
            !read
                .iter()
                .any(|t| t.starts_with('b') || t.starts_with('c')),
            "the middle is left out"
        );
        assert_eq!(capped(&["é".repeat(10)]), vec!["é".repeat(10)]);
        assert!(capped(&[" ".into()]).is_empty());
    }

    #[test]
    fn names_come_from_a_json_answer() {
        let answer = |text: &str| json!({"choices": [{"message": {"content": text}}]});
        assert_eq!(
            names(&answer("<think>x</think>```json\n{\"title\": \"Fix the VEHICULE reverse accessor clash.\", \"topic\": \"Debugging a Django data model migration\"}\n```")),
            (Some("Fix the VEHICULE reverse accessor clash".into()), Some("Debugging a Django data model migration".into()))
        );
        assert_eq!(names(&answer("{\"title\": \"x\"}")), (None, None));
        assert_eq!(names(&answer("no json")), (None, None));
        assert_eq!(
            names(&answer(&format!("{{\"title\": \"{}\"}}", "w ".repeat(100))))
                .0
                .unwrap()
                .chars()
                .count(),
            TITLE_CHARS
        );
    }

    #[test]
    fn only_the_title_chat_gave_is_replaced() {
        let first = "getting this error in make migrations in django ,  how can i fix it\ndata_model.VEHICULE";
        assert!(auto_title(
            "getting this error in make migrations in django , how can i fix it",
            first
        ));
        assert!(!auto_title("Django migration error", first));
        assert!(!auto_title("", first));
        assert!(!auto_title("report.pdf", "Summarize this"));
    }

    #[test]
    fn topic_lines_are_one_clean_line() {
        let answer = |text: &str| json!({"choices": [{"message": {"content": text}}]});
        assert_eq!(
            topic_line(&answer(
                "<think>hmm</think>\n\n\"Debugging a Django migration.\"\nmore"
            ))
            .as_deref(),
            Some("Debugging a Django migration")
        );
        assert_eq!(
            topic_line(&answer("**Writing fan fiction**")).as_deref(),
            Some("Writing fan fiction")
        );
        assert_eq!(topic_line(&answer("  \n ")), None);
        assert_eq!(topic_line(&json!({})), None);
        assert_eq!(
            topic_line(&answer(&"a".repeat(400))).unwrap().len(),
            TOPIC_CHARS
        );
    }

    #[test]
    fn secrets_are_found_and_ordinary_code_is_not() {
        for text in [
            "const modelToken = 'hf_kRdAbCdEfGhIjKlMnOpQrStUvWxYz012345';",
            "export OPENAI_API_KEY=sk-proj-abcdefghijklmnop1234",
            "API_TOKEN = '7089341234:AAF2bkd93kdl20dkKdk3ldk20dkdl3kdl'",
            "email_password = 'brwgeakdlqpzmxna'",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----",
            "const WALLET_SECRET = '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';",
            ".addHeader(\"X-RapidAPI-Key\", \"f07b38abcd1234ef5678abcd\")",
        ] {
            assert!(has_secret(text), "{text}");
        }
        for text in [
            "password = input('Enter password: ')",
            "token = response.json()",
            "const apiKeyInput = document.getElementById('api_key');",
            "the secret to good bread is time",
            "if (password.length < 12) return",
            "use the sk-learn package",
            "token: string",
        ] {
            assert!(!has_secret(text), "{text}");
        }
    }

    #[test]
    fn workstreams_come_from_a_json_list() {
        let text = "<think>x</think>Here: [{\"name\": \"Writing fan fiction\", \"description\": \"Stories\"}, {\"name\": \"writing fan fiction\"}, {\"name\": \"\"}, {\"name\": \"Debugging code\", \"description\": \"Fixes\"}] done";
        assert_eq!(
            read_workstreams(text).unwrap(),
            vec![
                ("Writing fan fiction".to_string(), "Stories".to_string()),
                ("Debugging code".to_string(), "Fixes".to_string())
            ]
        );
        assert!(read_workstreams("no list").is_none());
        assert!(read_workstreams("[]").is_none());
    }

    #[test]
    fn workstreams_count_unlisted_chats_without_their_topics() {
        let code = Uuid::new_v4();
        let retired = Uuid::new_v4();
        let row = |topic: Option<&str>, kind: &str, tokens: i64, stream: Option<Uuid>| Labelled {
            topic: topic.map(str::to_owned),
            kind: kind.into(),
            output: Some("code".into()),
            field: Some("computing".into()),
            turns: 2,
            tokens,
            workstream_id: stream,
            placed: stream.is_some(),
        };
        let rows = vec![
            row(Some("Debugging"), "work", 10, Some(code)),
            row(Some("Debugging"), "work", 30, Some(code)),
            // A deleted chat keeps its workstream without its topic.
            row(None, "work", 7, Some(code)),
            row(Some("Odd one"), "play", 5, None),
            // Placed in a workstream that has since gone: everything else until placed again.
            row(Some("Old"), "play", 5, Some(retired)),
            row(None, "personal", 100, None),
        ];
        let streams = vec![(code, "Code".to_string(), "Fixing code".to_string())];
        let out = workstreams(&rows, &streams);
        let list = out.as_array().unwrap();
        assert_eq!(list[0]["name"], "Code");
        assert_eq!(list[0]["sessions"], 3);
        assert_eq!(list[0]["rows"].as_array().unwrap().len(), 2);
        assert_eq!(list[0]["rows"][0]["tokens"], 30);
        assert_eq!(list[1]["name"], "Everything else");
        assert_eq!(list[1]["sessions"], 2);
        assert_eq!(list[2]["name"], "Personal and unlisted chats");
        assert_eq!(list[2]["private"], true);
        assert_eq!(list[2]["rows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn the_workstream_list_is_found_when_asked_new_old_or_missing_much() {
        let now = Utc::now();
        let days = |n: i64| Some(now - chrono::Duration::days(n));
        let fresh = StreamsDue {
            requested: false,
            building: false,
            failed: false,
            tried_at: days(2),
            found_at: days(2),
            current: 8,
            topics: 300,
            placed: 200,
            other: 10,
        };
        assert!(!fresh.due(now));
        assert!(StreamsDue {
            requested: true,
            ..fresh
        }
        .due(now));
        assert!(
            !StreamsDue {
                requested: true,
                building: true,
                ..fresh
            }
            .due(now),
            "never twice at once"
        );
        assert!(
            StreamsDue {
                found_at: days(8),
                ..fresh
            }
            .due(now),
            "a week old"
        );
        assert!(
            StreamsDue { other: 40, ..fresh }.due(now),
            "a fifth fit none"
        );
        assert!(
            !StreamsDue {
                other: 40,
                found_at: Some(now - chrono::Duration::hours(3)),
                ..fresh
            }
            .due(now),
            "not twice a day"
        );
        assert!(
            StreamsDue {
                current: 0,
                found_at: None,
                ..fresh
            }
            .due(now),
            "no list yet"
        );
        assert!(
            !StreamsDue {
                current: 0,
                found_at: None,
                topics: 12,
                ..fresh
            }
            .due(now),
            "too few topics"
        );
        let failed = StreamsDue {
            current: 0,
            found_at: None,
            failed: true,
            tried_at: Some(now - chrono::Duration::hours(1)),
            ..fresh
        };
        assert!(!failed.due(now), "a failure waits");
        assert!(StreamsDue {
            tried_at: Some(now - chrono::Duration::hours(7)),
            ..failed
        }
        .due(now));
    }
}
