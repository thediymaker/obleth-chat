//! Private conversation persistence. Every lookup is scoped to its authenticated owner.
//!
//! User-row locks serialize only short database transactions; generation runs outside
//! transactions. This keeps the per-user concurrent-response limit (operations::Limits)
//! correct across replicas. A selected transcript holds at most 200 messages, and a
//! conversation stores at most 1,000 including versions. Inference context is capped
//! at 100 KiB (including the system prompt), and is rejected explicitly rather than truncated.
//! Answer and reasoning text, including failed and interrupted replies, share a
//! 4 MiB storage budget; a new turn reserves its prompt and a combined 1 MiB reply.
//! Provider reasoning stays out of subsequent inference context.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::attachments::{self, Attachment, AttachmentMeta};
use crate::error::ApiError;
use std::collections::HashMap;

mod variants;
mod workflows;
pub use variants::VariantContext;
pub use workflows::{ChatArchive, ConversationPatch, ForkResult, HistoryPage, HistoryQuery};

pub const MAX_HISTORY: usize = 200;
pub const MAX_USER_MESSAGE_CHARS: usize = 12_000;
pub const MAX_SYSTEM_PROMPT_BYTES: usize = 8 * 1024;
pub const MAX_STORED_MESSAGES: usize = 1000;
pub const MAX_CONTEXT_BYTES: usize = 100 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Sum of UTF-8 answer and reasoning bytes, including every terminal status.
/// Before a turn starts, its user message and MAX_RESPONSE_BYTES must fit here.
pub const MAX_CONVERSATION_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CONVERSATIONS_PER_USER: i64 = 1_000;
/// The background worker renews this lease while it is alive; a failed worker
/// becomes recoverable on the next read within 45 seconds.
pub const GENERATION_LEASE_SECONDS: i32 = 45;

/// Chat and reply settings that other people may see and take into a fork: the
/// model and how it samples. Instructions such as the system prompt stay private
/// to the chat's owner.
const SHAREABLE_SETTINGS: [&str; 7] = [
    "model",
    "resolvedModel",
    "auto",
    "temperature",
    "topP",
    "maxTokens",
    "reasoningEffort",
];

/// The part of a chat's or reply's settings that is shown to, or copied for,
/// someone other than its owner.
pub fn shareable_settings(settings: &Value) -> Value {
    Value::Object(
        settings
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| SHAREABLE_SETTINGS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Low,
    #[default]
    Medium,
    High,
}
impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
    /// Where attachment and generated image bytes go; Postgres unless a bucket is set.
    pub files: crate::files::Files,
}

#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub reasoning_content: String,
    pub status: String,
    pub settings: Option<Value>,
    #[sqlx(skip)]
    pub attachments: Vec<AttachmentMeta>,
    pub finish_reason: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip)]
    pub parent_message_id: Option<Uuid>,
    #[sqlx(skip)]
    pub versions: Vec<MessageVersion>,
    #[sqlx(skip)]
    pub version_index: usize,
    pub follow_ups: Value,
    pub follow_ups_status: String,
    pub activity: Value,
}

#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MessageVersion {
    pub id: Uuid,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: Uuid,
    pub title: String,
    pub model: String,
    pub reasoning_effort: String,
    pub updated_at: DateTime<Utc>,
    pub settings: Value,
    pub archived: bool,
    pub pinned: bool,
    pub parent_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: Uuid,
    pub title: String,
    pub model: String,
    pub reasoning_effort: String,
    pub updated_at: DateTime<Utc>,
    pub settings: Value,
    pub archived: bool,
    pub pinned: bool,
    pub parent_id: Option<Uuid>,
    pub messages: Vec<Message>,
    pub active_generation_id: Option<Uuid>,
    pub generation_cancel_requested: bool,
    /// Who else may read it: none, group or signed_in.
    pub share_scope: String,
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub attachments: HashMap<Uuid, Vec<Attachment>>,
    pub generation_id: Uuid,
    pub assistant_id: Uuid,
    pub reasoning_effort: String,
    /// Completed messages, followed by the newly persisted user message.
    /// Failed, interrupted, and pending assistant messages never enter inference.
    pub messages: Vec<Message>,
}

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self {
            files: crate::files::Files::new(pool.clone(), None),
            pool,
        }
    }

    /// Keeps new files' bytes in object storage instead of Postgres.
    pub fn with_files(mut self, files: crate::files::Files) -> Self {
        self.files = files;
        self
    }

    pub async fn list(&self, owner: Uuid) -> Result<Vec<ConversationSummary>, ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let rows = sqlx::query_as::<_, ConversationSummary>(
            "SELECT id, title, model, reasoning_effort, updated_at, settings, archived, pinned, parent_id FROM conversations
             WHERE owner_id = $1 ORDER BY updated_at DESC, id",
        )
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn create(
        &self,
        owner: Uuid,
        title: &str,
        model: &str,
    ) -> Result<Conversation, ApiError> {
        self.create_with_effort(owner, title, model, ReasoningEffort::default())
            .await
    }

    pub async fn create_with_effort(
        &self,
        owner: Uuid,
        title: &str,
        model: &str,
        reasoning_effort: ReasoningEffort,
    ) -> Result<Conversation, ApiError> {
        validate_title(title)?;
        validate_model(model)?;
        let mut tx = self.owner_transaction(owner).await?;
        require_conversation_capacity(&mut tx, owner).await?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO conversations (id, owner_id, title, model, reasoning_effort, settings) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(id)
            .bind(owner)
            .bind(title.trim())
            .bind(model)
            .bind(reasoning_effort.as_str())
            .bind(json!({"reasoningEffort":reasoning_effort}))
            .execute(&mut *tx)
            .await?;
        let conversation = read_conversation(&mut tx, owner, id).await?;
        tx.commit().await?;
        Ok(conversation)
    }

    pub async fn get(&self, owner: Uuid, id: Uuid) -> Result<Conversation, ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let conversation = read_conversation(&mut tx, owner, id).await?;
        tx.commit().await?;
        Ok(conversation)
    }

    /// Saved parameter defaults for a new turn. Unlocked: start_turn_inner re-checks
    /// ownership and admission under the conversation lock.
    pub async fn saved_settings(&self, owner: Uuid, id: Uuid) -> Result<Value, ApiError> {
        sqlx::query_scalar("SELECT settings FROM conversations WHERE id=$1 AND owner_id=$2")
            .bind(id)
            .bind(owner)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(ApiError::not_found)
    }

    pub async fn rename(
        &self,
        owner: Uuid,
        id: Uuid,
        title: &str,
    ) -> Result<Conversation, ApiError> {
        self.update(owner, id, Some(title), None).await
    }

    pub async fn update(
        &self,
        owner: Uuid,
        id: Uuid,
        title: Option<&str>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Conversation, ApiError> {
        if title.is_none() && reasoning_effort.is_none() {
            return Err(ApiError::bad("Provide a title or reasoning effort"));
        }
        if let Some(title) = title {
            validate_title(title)?;
        }
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let active: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT active_generation FROM conversations WHERE id = $1 AND owner_id = $2 FOR UPDATE",
        ).bind(id).bind(owner).fetch_optional(&mut *tx).await?;
        match active {
            None => return Err(ApiError::not_found()),
            Some(Some(_)) if reasoning_effort.is_some() => {
                return Err(conflict(
                    "Stop the response before changing reasoning effort",
                ));
            }
            _ => {}
        }
        let changed = sqlx::query(
            "UPDATE conversations SET title = COALESCE($3, title), reasoning_effort = COALESCE($4, reasoning_effort), updated_at = clock_timestamp()
             WHERE id = $1 AND owner_id = $2",
        )
        .bind(id)
        .bind(owner)
        .bind(title.map(str::trim))
        .bind(reasoning_effort.map(ReasoningEffort::as_str))
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        let conversation = read_conversation(&mut tx, owner, id).await?;
        tx.commit().await?;
        Ok(conversation)
    }

    pub async fn delete(&self, owner: Uuid, id: Uuid) -> Result<(), ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let active: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT active_generation FROM conversations WHERE id = $1 AND owner_id = $2 FOR UPDATE",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        match active {
            None => return Err(ApiError::not_found()),
            Some(Some(_)) => {
                return Err(conflict(
                    "Stop the response before deleting this conversation",
                ))
            }
            Some(None) => {}
        }
        let changed = sqlx::query("DELETE FROM conversations WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        if changed.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        tx.commit().await?;
        Ok(())
    }

    /// A chat as someone it is shared with reads it: unlocked, and without recovering
    /// the owner's expired generations (the owner's own reads do that).
    pub async fn read_shared(&self, owner: Uuid, id: Uuid) -> Result<Conversation, ApiError> {
        let mut tx = self.pool.begin().await?;
        let conversation = read_conversation(&mut tx, owner, id).await?;
        tx.commit().await?;
        Ok(conversation)
    }

    /// Copies someone else's shared chat into a new chat of `viewer`'s, with its
    /// attachments and generated images, so the copy keeps working if the original goes.
    /// Only the shareable settings come along: the owner's instructions would
    /// otherwise steer the viewer's chat, and their tools, unseen.
    pub async fn fork_shared(
        &self,
        viewer: Uuid,
        owner: Uuid,
        id: Uuid,
    ) -> Result<Conversation, ApiError> {
        let mut tx = self.owner_transaction(viewer).await?;
        let original = read_conversation(&mut tx, owner, id).await?;
        require_conversation_capacity(&mut tx, viewer).await?;
        // A reply still being written is left out; the rest of the chat is copied.
        let selected = original
            .messages
            .iter()
            .take_while(|m| m.status != "pending")
            .map(|m| m.id)
            .collect::<Vec<_>>();
        let fork = Uuid::new_v4();
        let title = format!(
            "{} · fork",
            original.title.chars().take(153).collect::<String>()
        );
        sqlx::query("INSERT INTO conversations (id, owner_id, title, model, reasoning_effort, settings) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(fork)
            .bind(viewer)
            .bind(title)
            .bind(&original.model)
            .bind(&original.reasoning_effort)
            .bind(shareable_settings(&original.settings))
            .execute(&mut *tx)
            .await?;
        copy_selected_messages(&mut tx, id, fork, &selected, Some(&SHAREABLE_SETTINGS[..])).await?;
        // The copy's attachments become the viewer's own.
        let linked: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT ma.attachment_id FROM message_attachments ma JOIN messages m ON m.id = ma.message_id WHERE m.conversation_id = $1",
        )
        .bind(fork)
        .fetch_all(&mut *tx)
        .await?;
        for old in linked {
            let new = Uuid::new_v4();
            let (mime, data, key): (String, Option<Vec<u8>>, Option<String>) =
                sqlx::query_as("SELECT mime_type, data, object_key FROM attachments WHERE id = $1")
                    .bind(old)
                    .fetch_one(&mut *tx)
                    .await?;
            let placed = self
                .files
                .copy(
                    data,
                    key,
                    crate::files::attachment_key(viewer, new),
                    &mime,
                    attachments::MAX_IMAGE_BYTES,
                )
                .await?;
            sqlx::query("INSERT INTO attachments (id, owner_id, name, mime_type, kind, size, data, object_key, sha256) SELECT $1, $2, name, mime_type, kind, size, $4, $5, sha256 FROM attachments WHERE id = $3")
                .bind(new)
                .bind(viewer)
                .bind(old)
                .bind(&placed.data)
                .bind(&placed.key)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE message_attachments ma SET attachment_id = $1 FROM messages m WHERE ma.attachment_id = $2 AND m.id = ma.message_id AND m.conversation_id = $3")
                .bind(new)
                .bind(old)
                .bind(fork)
                .execute(&mut *tx)
                .await?;
        }
        // Generated images too, with the copied replies pointing at the new ones.
        let images: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM generated_images WHERE conversation_id = $1")
                .bind(id)
                .fetch_all(&mut *tx)
                .await?;
        for old in images {
            let new = Uuid::new_v4();
            let (mime, data, key): (String, Option<Vec<u8>>, Option<String>) = sqlx::query_as(
                "SELECT mime_type, data, object_key FROM generated_images WHERE id = $1",
            )
            .bind(old)
            .fetch_one(&mut *tx)
            .await?;
            let placed = self
                .files
                .copy(
                    data,
                    key,
                    crate::files::image_key(viewer, new),
                    &mime,
                    crate::images::MAX_IMAGE_BYTES,
                )
                .await?;
            sqlx::query("INSERT INTO generated_images (id, owner_id, conversation_id, mime_type, size, data, object_key) SELECT $1, $2, $3, mime_type, size, $5, $6 FROM generated_images WHERE id = $4")
                .bind(new)
                .bind(viewer)
                .bind(fork)
                .bind(old)
                .bind(&placed.data)
                .bind(&placed.key)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE messages SET content = REPLACE(content, $2, $3) WHERE conversation_id = $1 AND content LIKE '%' || $2 || '%'")
                .bind(fork)
                .bind(format!("/api/images/{old}"))
                .bind(format!("/api/images/{new}"))
                .execute(&mut *tx)
                .await?;
        }
        let conversation = read_conversation(&mut tx, viewer, fork).await?;
        tx.commit().await?;
        Ok(conversation)
    }

    pub async fn branch(
        &self,
        owner: Uuid,
        id: Uuid,
        through_message_id: Uuid,
    ) -> Result<Conversation, ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let original = read_conversation(&mut tx, owner, id).await?;
        require_conversation_capacity(&mut tx, owner).await?;
        let through = original
            .messages
            .iter()
            .position(|message| message.id == through_message_id)
            .ok_or_else(ApiError::not_found)?;
        if original.messages[..=through]
            .iter()
            .any(|message| message.status == "pending")
        {
            return Err(conflict(
                "Wait for the response to finish before branching it",
            ));
        }
        let branch_id = Uuid::new_v4();
        let title = format!(
            "{} · branch",
            original.title.chars().take(151).collect::<String>()
        );
        sqlx::query("INSERT INTO conversations (id, owner_id, title, model, reasoning_effort, settings, parent_id, branch_point_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(branch_id)
            .bind(owner)
            .bind(title)
            .bind(&original.model)
            .bind(&original.reasoning_effort)
            .bind(&original.settings)
            .bind(original.parent_id.unwrap_or(id))
            .bind(through_message_id)
            .execute(&mut *tx)
            .await?;
        let selected = original.messages[..=through]
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>();
        copy_selected_messages(&mut tx, id, branch_id, &selected, None).await?;
        let conversation = read_conversation(&mut tx, owner, branch_id).await?;
        tx.commit().await?;
        Ok(conversation)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
    ) -> Result<Turn, ApiError> {
        self.start_turn_with_effort(
            owner,
            id,
            content,
            model,
            system_prompt,
            temperature,
            max_tokens,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn_with_effort(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Turn, ApiError> {
        self.start_turn_configured(
            owner,
            id,
            content,
            model,
            system_prompt,
            temperature,
            max_tokens,
            reasoning_effort,
            1.0,
            model,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn_configured(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
        top_p: f64,
        requested_model: &str,
    ) -> Result<Turn, ApiError> {
        self.start_turn_attached(
            owner,
            id,
            content,
            model,
            system_prompt,
            temperature,
            max_tokens,
            reasoning_effort,
            top_p,
            requested_model,
            &[],
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn_attached(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
        top_p: f64,
        requested_model: &str,
        attachment_ids: &[Uuid],
        vision: bool,
    ) -> Result<Turn, ApiError> {
        self.start_turn_inner(
            owner,
            id,
            content,
            model,
            system_prompt,
            temperature,
            max_tokens,
            reasoning_effort,
            top_p,
            requested_model,
            attachment_ids,
            vision,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_turn_inner(
        &self,
        owner: Uuid,
        id: Uuid,
        content: &str,
        model: &str,
        system_prompt: &str,
        temperature: f64,
        max_tokens: i32,
        reasoning_effort: Option<ReasoningEffort>,
        top_p: f64,
        requested_model: &str,
        attachment_ids: &[Uuid],
        vision: bool,
        variant: Option<(Uuid, &str)>,
    ) -> Result<Turn, ApiError> {
        validate_model(model)?;
        validate_model(requested_model)?;
        if system_prompt.len() > MAX_SYSTEM_PROMPT_BYTES
            || !top_p.is_finite()
            || !(0.0..=1.0).contains(&top_p)
        {
            return Err(ApiError::bad("Invalid system prompt or top-p"));
        }
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err(ApiError::bad("Temperature must be between 0 and 2"));
        }
        if !(0..=crate::models::MAX_OUTPUT_TOKENS).contains(&max_tokens) {
            return Err(ApiError::bad(
                "The output limit must be 0 (the model's own) or up to 262,144 tokens",
            ));
        }
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let active: Option<(Option<Uuid>, String)> = sqlx::query_as(
            "SELECT active_generation, reasoning_effort FROM conversations WHERE id = $1 AND owner_id = $2 FOR UPDATE",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        let saved_effort = match active {
            None => return Err(ApiError::not_found()),
            Some((Some(_), _)) => {
                return Err(conflict(
                    "This conversation already has a response in progress",
                ))
            }
            Some((None, saved_effort)) => saved_effort,
        };
        let reasoning_effort = reasoning_effort
            .map(|effort| effort.as_str().to_owned())
            .unwrap_or(saved_effort);
        // Inference needs neither version metadata nor the conversation's other branches.
        let mut messages = read_path(&mut tx, owner, id).await?;
        let source = if let Some((message, mode)) = variant {
            let path = messages
                .iter()
                .map(|m| (m.id, m.role.as_str()))
                .collect::<Vec<_>>();
            let index = variants::source_index(&path, message, mode)?;
            let source = messages[index].clone();
            messages.truncate(index);
            Some(source)
        } else {
            None
        };
        let regenerate = variant.is_some_and(|(_, mode)| mode == "regenerate");
        let source_files = source
            .as_ref()
            .map(|m| m.attachments.iter().map(|a| a.id).collect::<Vec<_>>());
        let attachment_ids = source_files.as_deref().unwrap_or(attachment_ids);
        let content = if regenerate {
            source.as_ref().unwrap().content.as_str()
        } else {
            content
        };
        // Variants retain the source files even when a caller omits attachment
        // IDs. Validate the effective message after resolving that saved source.
        if (content.trim().is_empty() && attachment_ids.is_empty())
            || content.chars().count() > MAX_USER_MESSAGE_CHARS
            || content.contains('\0')
        {
            return Err(ApiError::bad(
                "Messages must contain 1 to 12,000 characters",
            ));
        }
        let parent = messages.last().map(|m| m.id);
        if messages.len() + 2 > MAX_HISTORY {
            return Err(ApiError::bad("This conversation has reached 200 messages. Start a new conversation or branch an earlier message"));
        }
        let (ordinal,stored_bytes):(i64,i64)=sqlx::query_as("SELECT COALESCE(MAX(ordinal)+1,0)::bigint,COALESCE(SUM(OCTET_LENGTH(content)+OCTET_LENGTH(reasoning_content)),0)::bigint FROM messages WHERE conversation_id=$1").bind(id).fetch_one(&mut *tx).await?;
        let added = if regenerate { 1 } else { 2 };
        if ordinal as usize + added > MAX_STORED_MESSAGES {
            return Err(ApiError::bad("This conversation has reached its 1,000 saved-message/version limit. Start a new conversation or explicitly branch the selected path."));
        }
        if stored_bytes as usize + if regenerate { 0 } else { content.len() } + MAX_RESPONSE_BYTES
            > MAX_CONVERSATION_BYTES
        {
            return Err(ApiError::bad("This conversation cannot fit another response within its 4 MiB stored-message limit. Start a new conversation or branch an earlier message"));
        }
        // A partial answer is not replayed, but a workspace reply that stopped early
        // still changed the workspace: it is replayed with its steps so "continue"
        // picks up where it left off.
        messages.retain(|message| {
            let worked = message.activity.as_array().is_some_and(|steps| {
                steps.iter().any(|s| {
                    s.get("kind")
                        .and_then(Value::as_str)
                        .is_none_or(|k| k != "thought" && k != "sources")
                })
            });
            let replayed = message.status == "completed"
                || (message.role == "assistant"
                    && worked
                    && matches!(message.status.as_str(), "error" | "interrupted"));
            replayed && !(message.role == "assistant" && replay_content(message).is_empty())
        });
        let files =
            attachments::load_owner(&mut tx, &self.files, owner, attachment_ids, vision).await?;
        let message_ids = messages.iter().map(|m| m.id).collect::<Vec<_>>();
        let mut attached =
            attachments::load_messages(&mut tx, &self.files, owner, &message_ids, vision).await?;
        let context_files = attached.values().flatten().chain(files.iter());
        let mut text_bytes = 0usize;
        let mut image_bytes = 0usize;
        for file in context_files {
            if file.meta.kind == "text" {
                text_bytes += file.context_bytes();
            } else {
                image_bytes += file.context_bytes();
            }
        }
        if image_bytes > attachments::MAX_IMAGE_CONTEXT_BYTES {
            return Err(ApiError::bad("Images exceed the 20 MiB request context limit. Start a new conversation or branch an earlier message."));
        }
        let context_size = messages
            .iter()
            .map(|message| replay_content(message).len())
            .sum::<usize>()
            + content.len()
            + system_prompt.len()
            + text_bytes;
        if context_size > MAX_CONTEXT_BYTES {
            return Err(ApiError::bad("This conversation exceeds the 100 KiB context limit. Start a new conversation or branch an earlier message"));
        }
        let generation_id = Uuid::new_v4();
        crate::operations::reserve_in(
            &mut tx,
            owner,
            generation_id,
            model,
            max_tokens,
            context_size,
        )
        .await?;
        let user_id = if regenerate {
            source.as_ref().unwrap().id
        } else {
            Uuid::new_v4()
        };
        let assistant_id = Uuid::new_v4();
        if !regenerate {
            sqlx::query(
            "INSERT INTO messages (id, conversation_id, ordinal, role, content, status, parent_message_id)
             VALUES ($1, $2, $3, 'user', $4, 'completed',$5)",
        )
        .bind(user_id)
        .bind(id)
        .bind(ordinal as i32)
        .bind(content)
        .bind(parent)
        .execute(&mut *tx)
        .await?;
            attachments::link_message(&mut tx, owner, user_id, attachment_ids).await?;
            if let Some(parent) = parent {
                sqlx::query(
                    "UPDATE messages SET selected_child_id=$2 WHERE id=$1 AND conversation_id=$3",
                )
                .bind(parent)
                .bind(user_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, ordinal, role, content, status, generation_id, settings,parent_message_id)
             VALUES ($1, $2, $3, 'assistant', '', 'pending', $4, $5,$6)",
        )
        .bind(assistant_id)
        .bind(id)
        .bind((ordinal + if regenerate{0}else{1}) as i32)
        .bind(generation_id)
        .bind(json!({"model": model, "systemPrompt": system_prompt, "temperature": temperature, "maxTokens": max_tokens, "reasoningEffort": reasoning_effort, "topP": top_p}))
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE messages SET selected_child_id=$2 WHERE id=$1 AND conversation_id=$3")
            .bind(user_id)
            .bind(assistant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE conversations SET model = $3, active_generation = $4,
             generation_expires = clock_timestamp() + make_interval(secs => $5),
            reasoning_effort = $6, settings = $7, selected_leaf_id=$8,generation_cancel_requested=FALSE,archived = FALSE, updated_at = clock_timestamp() WHERE id = $1 AND owner_id = $2",
        )
        .bind(id)
        .bind(owner)
        .bind(requested_model)
        .bind(generation_id)
        .bind(f64::from(GENERATION_LEASE_SECONDS))
        .bind(&reasoning_effort)
        .bind(json!({"systemPrompt":system_prompt,"temperature":temperature,"topP":top_p,"maxTokens":max_tokens,"reasoningEffort":reasoning_effort}))
        .bind(assistant_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        messages.push(Message {
            id: user_id,
            role: "user".into(),
            content: content.into(),
            reasoning_content: String::new(),
            status: "completed".into(),
            settings: None,
            attachments: files.iter().map(|f| f.meta.clone()).collect(),
            finish_reason: None,
            error_message: None,
            created_at: Utc::now(),
            usage: None,
            request_id: None,
            parent_message_id: parent,
            versions: vec![],
            version_index: 1,
            follow_ups: json!([]),
            follow_ups_status: "idle".into(),
            activity: json!([]),
        });
        attached.insert(user_id, files);
        Ok(Turn {
            attachments: attached,
            generation_id,
            assistant_id,
            reasoning_effort,
            messages,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        content: &str,
        status: &str,
        usage: Option<Value>,
        request_id: Option<&str>,
    ) -> Result<(), ApiError> {
        self.finish_turn_with_reasoning(
            owner,
            id,
            generation_id,
            content,
            "",
            status,
            usage,
            request_id,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_turn_with_reasoning(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        content: &str,
        reasoning_content: &str,
        status: &str,
        usage: Option<Value>,
        request_id: Option<&str>,
    ) -> Result<(), ApiError> {
        self.finish_turn_detailed(
            owner,
            id,
            generation_id,
            content,
            reasoning_content,
            status,
            usage,
            request_id,
            None,
            None,
        )
        .await
    }

    /// Records the model the gateway's Auto router chose for a reply.
    pub async fn set_served_model(&self, message: Uuid, model: &str) -> Result<(), ApiError> {
        validate_model(model)?;
        sqlx::query(
            "WITH served AS (UPDATE messages SET settings = settings || jsonb_build_object('model', $2::text, 'auto', true) WHERE id = $1 RETURNING generation_id)
             UPDATE generation_usage SET model = $2 WHERE generation_id IN (SELECT generation_id FROM served)",
        )
        .bind(message)
        .bind(model)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_turn_detailed(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        content: &str,
        reasoning_content: &str,
        status: &str,
        usage: Option<Value>,
        request_id: Option<&str>,
        finish_reason: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), ApiError> {
        if finish_reason.is_some_and(|s| s.len() > 80)
            || error_message.is_some_and(|s| s.chars().count() > 1000)
        {
            return Err(ApiError::bad("Invalid response result"));
        }
        if !matches!(status, "completed" | "interrupted" | "error") {
            return Err(ApiError::bad("Invalid response status"));
        }
        if content.len() + reasoning_content.len() > MAX_RESPONSE_BYTES {
            return Err(ApiError::bad(
                "The response exceeds the 1 MiB storage limit",
            ));
        }
        // Internal completion must still persist interruption after an administrator
        // disables an account. No HTTP route exposes this operation directly.
        let mut tx = self.lock_owner(owner, true).await?;
        recover_expired_in(&mut tx, owner).await?;
        let active: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT active_generation FROM conversations WHERE id = $1 AND owner_id = $2 FOR UPDATE",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        match active {
            None => return Err(ApiError::not_found()),
            Some(Some(active_id)) if active_id == generation_id => {}
            _ => {
                // Preserve expiry recovery even when rejecting a late writer.
                tx.commit().await?;
                return Err(conflict("This response is no longer active"));
            }
        }
        crate::operations::finish_with_output_in(
            &mut tx,
            generation_id,
            usage.as_ref(),
            status,
            Some(content.len() + reasoning_content.len()),
        )
        .await?;
        sqlx::query(
            "UPDATE messages SET content = $4, status = $5, usage = $6, request_id = $7, reasoning_content = $8, finish_reason = $9, error_message = $10
             WHERE conversation_id = $1 AND generation_id = $3 AND status = 'pending'
             AND EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND owner_id = $2)",
        )
        .bind(id)
        .bind(owner)
        .bind(generation_id)
        .bind(content)
        .bind(status)
        .bind(usage)
        .bind(request_id)
        .bind(reasoning_content)
        .bind(finish_reason)
        .bind(error_message)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE conversations SET active_generation = NULL, generation_expires = NULL,generation_cancel_requested=FALSE,
             updated_at = clock_timestamp() WHERE id = $1 AND owner_id = $2 AND active_generation = $3",
        )
        .bind(id)
        .bind(owner)
        .bind(generation_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn interrupt_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        generation_id: Uuid,
        partial_content: &str,
    ) -> Result<(), ApiError> {
        self.finish_turn(
            owner,
            id,
            generation_id,
            partial_content,
            "interrupted",
            None,
            None,
        )
        .await
    }

    pub async fn recover_expired(&self, owner: Uuid) -> Result<u64, ApiError> {
        let mut tx = self.lock_owner(owner, true).await?;
        let recovered = recover_expired_in(&mut tx, owner).await?;
        tx.commit().await?;
        Ok(recovered)
    }

    async fn owner_transaction(&self, owner: Uuid) -> Result<Transaction<'_, Postgres>, ApiError> {
        self.lock_owner(owner, false).await
    }

    async fn lock_owner(
        &self,
        owner: Uuid,
        allow_disabled: bool,
    ) -> Result<Transaction<'_, Postgres>, ApiError> {
        let mut tx = self.pool.begin().await?;
        // Consistent user -> conversation lock order prevents cross-operation deadlocks.
        let exists: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id = $1 AND (enabled = TRUE OR $2) FOR UPDATE",
        )
        .bind(owner)
        .bind(allow_disabled)
        .fetch_optional(&mut *tx)
        .await?;
        if exists.is_none() {
            return Err(ApiError::unauthorized());
        }
        Ok(tx)
    }
}

async fn recover_expired_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
) -> Result<u64, ApiError> {
    let result = sqlx::query(
        "WITH expired AS (
           UPDATE conversations SET active_generation = NULL, generation_expires = NULL,generation_cancel_requested=FALSE,
                  updated_at = clock_timestamp()
           WHERE owner_id = $1 AND active_generation IS NOT NULL AND generation_expires <= clock_timestamp()
           RETURNING id
         )
         UPDATE messages SET status = 'interrupted', error_message = 'The response was interrupted when its generation lease expired. You can retry it.' FROM expired
         WHERE messages.conversation_id = expired.id AND messages.status = 'pending'",
    )
    .bind(owner)
    .execute(&mut **tx)
    .await?;
    crate::operations::recover_in(tx, owner).await?;
    Ok(result.rows_affected())
}

async fn require_conversation_capacity(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
) -> Result<(), ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE owner_id = $1")
        .bind(owner)
        .fetch_one(&mut **tx)
        .await?;
    if count >= MAX_CONVERSATIONS_PER_USER {
        return Err(ApiError::bad("You have reached 1,000 conversations. Delete an older conversation before creating or branching another"));
    }
    Ok(())
}

async fn read_conversation(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    id: Uuid,
) -> Result<Conversation, ApiError> {
    let row = sqlx::query_as::<_, ConversationSummary>(
        "SELECT id, title, model, reasoning_effort, updated_at, settings, archived, pinned, parent_id FROM conversations WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let messages = read_messages(tx, owner, id).await?;
    let (active_generation_id,generation_cancel_requested,share_scope):(Option<Uuid>,bool,String)=sqlx::query_as("SELECT active_generation,generation_cancel_requested,share_scope FROM conversations WHERE id=$1 AND owner_id=$2").bind(id).bind(owner).fetch_one(&mut **tx).await?;
    Ok(Conversation {
        id: row.id,
        title: row.title,
        model: row.model,
        reasoning_effort: row.reasoning_effort,
        updated_at: row.updated_at,
        settings: row.settings,
        archived: row.archived,
        pinned: row.pinned,
        parent_id: row.parent_id,
        messages,
        active_generation_id,
        generation_cancel_requested,
        share_scope,
    })
}

async fn read_messages(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    id: Uuid,
) -> Result<Vec<Message>, ApiError> {
    let mut messages = read_path(tx, owner, id).await?;
    variants::populate_versions(tx, id, &mut messages).await?;
    Ok(messages)
}

/// The selected path with attachment metadata; versions stay empty.
async fn read_path(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    id: Uuid,
) -> Result<Vec<Message>, ApiError> {
    variants::initialize_path_in(tx, id).await?;
    let mut messages = sqlx::query_as::<_, Message>(
        "WITH RECURSIVE path AS (
           SELECT m.*,0 AS depth FROM messages m JOIN conversations c ON c.selected_leaf_id=m.id
             WHERE c.id=$1 AND c.owner_id=$2 AND m.conversation_id=c.id
           UNION ALL SELECT m.*,p.depth+1 FROM messages m JOIN path p ON m.id=p.parent_message_id
             WHERE m.conversation_id=$1 AND p.depth<200
         ) SELECT id,role,content,reasoning_content,status,usage,request_id,settings,finish_reason,error_message,created_at,parent_message_id,follow_ups,
           CASE WHEN follow_ups_status='pending' AND (follow_ups_expires_at IS NULL OR follow_ups_expires_at<=NOW()) THEN 'error' ELSE follow_ups_status END AS follow_ups_status,activity
           FROM path ORDER BY depth DESC",
    )
    .bind(id)
    .bind(owner)
    .fetch_all(&mut **tx)
    .await?;
    let ids = messages.iter().map(|m| m.id).collect::<Vec<_>>();
    let mut metadata = attachments::metadata_for_messages(tx, owner, &ids).await?;
    for message in &mut messages {
        message.attachments = metadata.remove(&message.id).unwrap_or_default();
    }
    Ok(messages)
}

fn validate_title(title: &str) -> Result<(), ApiError> {
    if title.trim().is_empty() || title.trim().chars().count() > 160 || title.contains('\0') {
        return Err(ApiError::bad("Titles must contain 1 to 160 characters"));
    }
    Ok(())
}

fn validate_model(model: &str) -> Result<(), ApiError> {
    if model.trim().is_empty() || model.chars().count() > 200 || model.chars().any(char::is_control)
    {
        return Err(ApiError::bad("Choose a valid model"));
    }
    Ok(())
}

fn conflict(message: &str) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

/// Copies messages into another chat. With `settings`, each reply keeps only those
/// settings keys, for a copy made for someone else.
async fn copy_selected_messages(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    destination: Uuid,
    ids: &[Uuid],
    settings: Option<&[&str]>,
) -> Result<(), ApiError> {
    let copies = ids.iter().map(|_| Uuid::new_v4()).collect::<Vec<_>>();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,reasoning_content,status,generation_id,settings,usage,request_id,finish_reason,error_message,created_at,follow_ups,follow_ups_status,activity)
      SELECT mapping.new_id,$2,(mapping.position-1)::integer,m.role,m.content,m.reasoning_content,m.status,CASE WHEN m.role='assistant' THEN gen_random_uuid() ELSE NULL END,
        CASE WHEN $5::text[] IS NULL THEN m.settings WHEN jsonb_typeof(m.settings)='object' THEN (SELECT COALESCE(jsonb_object_agg(e.key,e.value),'{}'::jsonb) FROM jsonb_each(m.settings) e WHERE e.key=ANY($5)) END,
        m.usage,m.request_id,m.finish_reason,m.error_message,m.created_at,m.follow_ups,CASE WHEN m.follow_ups_status='pending' THEN 'idle' ELSE m.follow_ups_status END,m.activity
      FROM unnest($3::uuid[],$4::uuid[]) WITH ORDINALITY mapping(old_id,new_id,position) JOIN messages m ON m.id=mapping.old_id WHERE m.conversation_id=$1")
        .bind(source).bind(destination).bind(ids).bind(&copies).bind(settings).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO message_attachments(message_id,attachment_id,position) SELECT mapping.new_id,ma.attachment_id,ma.position FROM unnest($1::uuid[],$2::uuid[]) mapping(old_id,new_id) JOIN message_attachments ma ON ma.message_id=mapping.old_id")
        .bind(ids).bind(&copies).execute(&mut **tx).await?;
    Ok(())
}

/// Steps recalled per replayed reply, and the most text they may add.
const REPLAY_STEPS: usize = 30;
const REPLAY_STEP_BYTES: usize = 6 * 1024;

/// What the model sees of an earlier message. An assistant reply also recalls the
/// tool steps it took and how it ended, since tool calls are not replayed.
/// Starts the note replay adds to a reply. The model sometimes copies it into a new
/// reply, so finished replies are cleaned with `strip_records`.
pub const RECORD: &str = "[Chat's record, not part of the reply: tool steps";

/// Removes copies of replay notes, current or older wording, that the model wrote itself.
pub fn strip_records(content: &str) -> String {
    let markers = [
        // Also catches a model's own variations, such as "...: tool call ...".
        "[Chat's record, not part of the reply",
        "[Tool steps in this reply",
        "[This reply stopped before finishing",
    ];
    let mut text = content.to_owned();
    while let Some(start) = markers.iter().filter_map(|m| text.find(m)).min() {
        // A note ends with a bracket at the end of a line.
        let rest = &text[start..];
        let end = rest
            .match_indices(']')
            .map(|(i, _)| i + 1)
            .find(|&i| rest[i..].starts_with('\n') || rest[i..].trim().is_empty())
            .unwrap_or(rest.len());
        text.replace_range(start..start + end, "");
    }
    if text.len() == content.len() {
        return text;
    }
    let mut cleaned = String::new();
    for line in text.trim().lines() {
        if line.trim().is_empty() && cleaned.ends_with("\n\n") {
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    cleaned.trim_end().to_owned()
}

pub fn replay_content(message: &Message) -> String {
    if message.role != "assistant" {
        return message.content.clone();
    }
    // Replies saved before cleaning existed may hold copied notes.
    let content = strip_records(&message.content);
    // Thinking and checkpoint entries are for the person; notes they sent are replayed.
    let steps: Vec<&Value> = message
        .activity
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| {
            !matches!(
                s.get("kind").and_then(Value::as_str),
                Some("thought" | "checkpoint" | "sources")
            )
        })
        .collect();
    let stopped = message.status != "completed";
    if steps.is_empty() && !stopped {
        return content;
    }
    let mut notes = String::new();
    if !steps.is_empty() {
        let skipped = steps.len().saturating_sub(REPLAY_STEPS);
        notes.push_str(RECORD);
        if skipped > 0 {
            notes.push_str(&format!(", the last {REPLAY_STEPS} of {}", steps.len()));
        }
        notes.push_str(":\n");
        for step in &steps[skipped..] {
            let field = |key: &str| step.get(key).and_then(Value::as_str).unwrap_or("");
            if field("kind") == "note" {
                let line = format!(
                    "- The person added a note: {}\n",
                    field("summary").chars().take(300).collect::<String>()
                );
                if notes.len() + line.len() > REPLAY_STEP_BYTES {
                    break;
                }
                notes.push_str(&line);
                continue;
            }
            let detail: String = field("detail")
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .chars()
                .take(160)
                .collect();
            let line = format!(
                "- {} ({}){}{}\n",
                field("summary"),
                field("status"),
                if detail.is_empty() { "" } else { ": " },
                detail.trim()
            );
            if notes.len() + line.len() > REPLAY_STEP_BYTES {
                notes.push_str("- …\n");
                break;
            }
            notes.push_str(&line);
        }
        notes.push(']');
    }
    if stopped {
        if !notes.is_empty() {
            notes.push('\n');
        }
        let reason = message
            .error_message
            .as_deref()
            .unwrap_or("it was interrupted");
        notes.push_str(&format!("[This reply stopped before finishing: {reason}]"));
    }
    match (content.trim().is_empty(), steps.is_empty()) {
        // A failed reply with nothing to show is not worth replaying.
        (true, true) => String::new(),
        (true, false) => notes,
        _ => format!("{}\n\n{notes}", content.trim_end()),
    }
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    #[test]
    fn copied_replay_notes_are_removed_from_replies() {
        let echoed = "Restarted the server. Open the preview.\n\n[Tool steps in this reply:\n- Started server on port 3003 (completed): 0.1s\n]\n\n[Tool steps in this reply:\n- Started the preview [port 3003] (completed): ready\n]";
        assert_eq!(
            strip_records(echoed),
            "Restarted the server. Open the preview."
        );
        let current = format!("{RECORD}:\n- Wrote a.ts (completed)\n]\nDone.");
        assert_eq!(strip_records(&current), "Done.");
        let plain = "Use arr[0] here.\n\n- one\n- two";
        assert_eq!(strip_records(plain), plain);
    }

    fn reply(content: &str, status: &str, activity: Value, error: Option<&str>) -> Message {
        Message {
            id: Uuid::nil(),
            role: "assistant".into(),
            content: content.into(),
            reasoning_content: String::new(),
            status: status.into(),
            settings: None,
            attachments: vec![],
            finish_reason: None,
            error_message: error.map(str::to_owned),
            created_at: Utc::now(),
            usage: None,
            request_id: None,
            parent_message_id: None,
            versions: vec![],
            version_index: 0,
            follow_ups: json!([]),
            follow_ups_status: "idle".into(),
            activity,
        }
    }

    #[test]
    fn plain_replies_replay_unchanged() {
        assert_eq!(
            replay_content(&reply("Hi", "completed", json!([]), None)),
            "Hi"
        );
    }

    #[test]
    fn stopped_workspace_replies_recall_their_steps() {
        let steps = json!([
            {"summary":"Ran cargo build","status":"error","detail":"exit 101\nerror[E0433]: failed to resolve"},
            {"summary":"Wrote src/lib.rs","status":"completed","detail":"40 lines"}
        ]);
        let text = replay_content(&reply(
            "Building it now.",
            "error",
            steps,
            Some("Generation reached the 15-minute time limit"),
        ));
        assert!(text.starts_with(&format!("Building it now.\n\n{RECORD}:")));
        assert!(text.contains("- Ran cargo build (error): exit 101\n"));
        assert!(text.contains("- Wrote src/lib.rs (completed): 40 lines\n"));
        assert!(text.ends_with(
            "[This reply stopped before finishing: Generation reached the 15-minute time limit]"
        ));
    }

    #[test]
    fn long_step_lists_keep_the_latest_steps() {
        let steps: Vec<Value> = (0..50)
            .map(|i| json!({"summary":format!("Step {i}"),"status":"completed","detail":""}))
            .collect();
        let text = replay_content(&reply("", "completed", json!(steps), None));
        assert!(text.contains("the last 30 of 50"));
        assert!(!text.contains("Step 19 "));
        assert!(text.contains("Step 49 (completed)"));
    }

    #[test]
    fn thoughts_and_checkpoints_are_not_replayed_but_notes_are() {
        let steps = json!([
            {"kind":"thought","from":0,"to":10,"status":"completed"},
            {"tool":"write_file","summary":"Wrote a.js","status":"completed"},
            {"kind":"checkpoint","summary":"Checkpoint 1","status":"completed"},
            {"kind":"note","summary":"make it blue","status":"completed"},
        ]);
        let text = replay_content(&reply("Done.", "completed", steps, None));
        assert!(text.contains("- Wrote a.js (completed)"), "{text}");
        assert!(
            text.contains("- The person added a note: make it blue"),
            "{text}"
        );
        assert!(
            !text.contains("Checkpoint") && !text.contains("thought"),
            "{text}"
        );
        // A reply that only thought before answering replays as plain text.
        let thought_only = json!([{"kind":"thought","from":0,"to":10,"status":"completed"}]);
        assert_eq!(
            replay_content(&reply("Answer.", "completed", thought_only, None)),
            "Answer."
        );
    }

    #[test]
    fn empty_failures_are_not_replayed() {
        assert_eq!(
            replay_content(&reply("", "error", json!([]), Some("Gateway error"))),
            ""
        );
    }
}
