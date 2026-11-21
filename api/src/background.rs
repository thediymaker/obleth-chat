//! What the passes that read chats on their own share: memory, insights, Auto mode
//! and knowledge.

use crate::auth::Identity;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// The account a pass works for, while it is enabled. It runs without a sign-in.
pub async fn identity_of(pool: &PgPool, id: Uuid) -> Result<Option<Identity>, sqlx::Error> {
    sqlx::query_as("SELECT id, username, display_name, role FROM users WHERE id = $1 AND enabled")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Why a pass over one chat did not finish.
pub enum Failure {
    Database(sqlx::Error),
    /// The gateway or the model gave no usable answer; the chat is tried again later.
    Answer(String),
}

impl From<sqlx::Error> for Failure {
    fn from(error: sqlx::Error) -> Self {
        Failure::Database(error)
    }
}

impl Failure {
    /// A database error is passed on; an answer that failed is the chat's own
    /// business and ends here.
    pub fn passed_on(self) -> Result<(), sqlx::Error> {
        match self {
            Failure::Database(error) => Err(error),
            Failure::Answer(_) => Ok(()),
        }
    }
}

/// A table of per-chat claims, keyed by `conversation_id`, with `claimed_until`,
/// `failed_at` and `attempts`.
pub struct Claims {
    pub table: &'static str,
    /// The SQL for the next attempt count, which sets how long the retry waits.
    pub attempts: &'static str,
    /// What a failed answer is logged as.
    pub failed: &'static str,
}

/// Ends a pass over one chat: a failure lets go of the claim and is counted, so the
/// chat is tried again later.
pub async fn settle(
    pool: &PgPool,
    claims: &Claims,
    id: Uuid,
    done: Result<(), Failure>,
) -> Result<(), sqlx::Error> {
    let Err(failure) = done else {
        return Ok(());
    };
    if let Failure::Answer(why) = &failure {
        tracing::info!(conversation = %id, "{}: {why}", claims.failed);
    }
    sqlx::query(&format!(
        "UPDATE {} SET claimed_until = NULL, failed_at = NOW(), attempts = {} WHERE conversation_id = $1",
        claims.table, claims.attempts
    ))
    .bind(id)
    .execute(pool)
    .await?;
    failure.passed_on()
}

/// A model's answer after its thinking, when it wrote that inline.
pub fn after_thinking(text: &str) -> &str {
    match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    }
}

/// The JSON from the first `open` to the last `close` of an answer, after thinking.
pub fn json_after_thinking(text: &str, open: char, close: char) -> Option<Value> {
    let text = after_thinking(text);
    let (start, end) = (text.find(open)?, text.rfind(close)?);
    if end < start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

/// A person's messages as a model reads them: the newest that fit in `total`
/// characters, in order, each cut to `per_message`. With `keep_first` the first
/// message is kept too, ahead of them.
pub fn newest_within(
    texts: &[String],
    per_message: usize,
    total: usize,
    keep_first: bool,
) -> Vec<String> {
    let cut = |text: &str, most: usize| text.chars().take(most).collect::<String>();
    let mut texts = texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty());
    let first = keep_first
        .then(|| texts.next().map(|first| cut(first, per_message.min(total))))
        .flatten();
    let mut left = total - first.as_ref().map_or(0, |f| f.chars().count());
    let mut newest = Vec::new();
    for text in texts.rev() {
        if left == 0 {
            break;
        }
        let part = cut(text, per_message.min(left));
        left -= part.chars().count();
        newest.push(part);
    }
    newest.reverse();
    first.into_iter().chain(newest).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thinking_is_left_out() {
        assert_eq!(
            after_thinking("<think>a</think>b</think> answer"),
            " answer"
        );
        assert_eq!(after_thinking("plain"), "plain");
        assert_eq!(
            json_after_thinking(
                "<think>{\"no\": 1}</think>Sure: {\"yes\": 2} done",
                '{',
                '}'
            ),
            Some(json!({"yes": 2}))
        );
        assert_eq!(json_after_thinking("x] then [", '[', ']'), None);
        assert_eq!(json_after_thinking("no json", '{', '}'), None);
    }

    #[test]
    fn the_newest_messages_fit_with_or_without_the_first() {
        let texts: Vec<String> = ["  ", "aaaa", "bbbb", "cccc", "dd"]
            .iter()
            .map(|t| t.to_string())
            .collect();
        assert_eq!(newest_within(&texts, 3, 5, false), ["ccc", "dd"]);
        assert_eq!(newest_within(&texts, 3, 7, false), ["bb", "ccc", "dd"]);
        assert_eq!(newest_within(&texts, 3, 7, true), ["aaa", "cc", "dd"]);
        assert!(newest_within(&texts[..1], 3, 7, true).is_empty());
        assert_eq!(newest_within(&["é".repeat(4)], 10, 10, true), ["éééé"]);
    }
}
