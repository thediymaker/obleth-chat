//! Saves an uploaded export: Chat's own through `import`, another app's history in
//! batches. Each batch is one transaction under the owner's lock, so a failure part
//! way leaves every earlier batch in place and the summary says how far it got.
use super::*;
use crate::imports::{self, ImportedChat, Upload};

/// Chats saved per transaction, and the text and messages one may carry.
const BATCH_CHATS: usize = 100;
const BATCH_BYTES: usize = 16 * 1024 * 1024;
const BATCH_MESSAGES: usize = 20_000;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    /// "obleth-chat", "open-webui", "chatgpt" or "claude".
    pub source: &'static str,
    pub imported: usize,
    /// Chats a previous import of this file already brought in.
    pub already_imported: usize,
    /// The oldest chats left out because the account's conversation limit was reached.
    pub over_limit: usize,
    /// Chats with no message to show.
    pub empty: usize,
    /// Alternative replies or edits, with their continuations, that did not fit.
    pub alternatives_left_out: usize,
    /// The earliest messages of chats longer than Chat keeps.
    pub messages_left_out: usize,
    /// Chats not saved because a batch failed, and why.
    pub failed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Chat's own exports return the new conversations, as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversations: Option<Vec<ConversationSummary>>,
}

impl Store {
    /// Imports a file as uploaded: Chat's export, or an Open WebUI, ChatGPT or Claude
    /// export, detected from its content.
    pub async fn import_upload(
        &self,
        owner: Uuid,
        body: impl AsRef<[u8]> + Send + 'static,
    ) -> Result<ImportSummary, ApiError> {
        if body.as_ref().len() > imports::MAX_IMPORT_BYTES {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Choose an export of 200 MB or less".into(),
            ));
        }
        // The body is dropped once read; only the shaped chats stay.
        let upload = tokio::task::spawn_blocking(move || imports::read(body.as_ref()))
            .await
            .map_err(|_| ApiError::internal())??;
        match upload {
            Upload::Archive(archive) => {
                let conversations = self.import(owner, archive).await?;
                Ok(ImportSummary {
                    source: "obleth-chat",
                    imported: conversations.len(),
                    conversations: Some(conversations),
                    ..Default::default()
                })
            }
            Upload::Chats(parsed) => {
                let mut summary = ImportSummary {
                    source: parsed.source.as_str(),
                    already_imported: parsed.repeated,
                    empty: parsed.empty,
                    alternatives_left_out: parsed.alternatives_left_out,
                    messages_left_out: parsed.messages_left_out,
                    ..Default::default()
                };
                self.save_imported(owner, parsed.chats, &mut summary)
                    .await?;
                Ok(summary)
            }
        }
    }

    async fn save_imported(
        &self,
        owner: Uuid,
        mut chats: Vec<ImportedChat>,
        summary: &mut ImportSummary,
    ) -> Result<(), ApiError> {
        // New replies use the person's default model; each imported reply still
        // records the model that wrote it.
        let preferences = crate::models::preferences(&self.pool, owner).await;
        let model = Some(preferences.default_model)
            .filter(|m| validate_model(m).is_ok())
            .unwrap_or_else(|| "auto".into());
        let effort = preferences.reasoning_effort;
        // When the conversation limit cuts an import short, the newest chats come over.
        chats.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        let mut chats = chats.into_iter().peekable();
        while chats.peek().is_some() {
            let mut batch = Vec::new();
            let (mut bytes, mut messages) = (0, 0);
            while let Some(chat) = chats.next_if(|c| {
                batch.is_empty()
                    || (batch.len() < BATCH_CHATS
                        && bytes + text_bytes(c) <= BATCH_BYTES
                        && messages + c.messages.len() <= BATCH_MESSAGES)
            }) {
                bytes += text_bytes(&chat);
                messages += chat.messages.len();
                batch.push(chat);
            }
            let size = batch.len();
            match self.save_batch(owner, &model, effort, batch, summary).await {
                Ok(true) => {}
                Ok(false) => {
                    summary.over_limit += chats.len();
                    break;
                }
                Err(error) if summary.imported == 0 => return Err(error),
                Err(error) => {
                    summary.failed = size + chats.len();
                    summary.error = Some(error.1);
                    break;
                }
            }
        }
        Ok(())
    }

    /// Saves one batch; false once the conversation limit is reached.
    async fn save_batch(
        &self,
        owner: Uuid,
        model: &str,
        effort: ReasoningEffort,
        mut batch: Vec<ImportedChat>,
        summary: &mut ImportSummary,
    ) -> Result<bool, ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        let keys: Vec<String> = batch.iter().filter_map(|c| c.key.clone()).collect();
        let existing: Vec<String> = sqlx::query_scalar(
            "SELECT import_key FROM conversations WHERE owner_id=$1 AND import_key=ANY($2)",
        )
        .bind(owner)
        .bind(&keys)
        .fetch_all(&mut *tx)
        .await?;
        let existing: std::collections::HashSet<String> = existing.into_iter().collect();
        let before = batch.len();
        batch.retain(|c| c.key.as_ref().is_none_or(|k| !existing.contains(k)));
        let already = before - batch.len();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE owner_id=$1")
            .bind(owner)
            .fetch_one(&mut *tx)
            .await?;
        let room = usize::try_from(MAX_CONVERSATIONS_PER_USER - count).unwrap_or(0);
        let over = batch.len().saturating_sub(room);
        batch.truncate(room);
        let ids: Vec<Uuid> = batch.iter().map(|_| Uuid::new_v4()).collect();
        sqlx::query("INSERT INTO conversations(id,owner_id,title,model,reasoning_effort,settings,archived,pinned,created_at,updated_at,import_key)
            SELECT u.id,$1,u.title,$2,$3,$4,u.archived,u.pinned,u.created_at,u.updated_at,u.import_key
            FROM unnest($5::uuid[],$6::text[],$7::boolean[],$8::boolean[],$9::timestamptz[],$10::timestamptz[],$11::text[]) AS u(id,title,archived,pinned,created_at,updated_at,import_key)")
            .bind(owner).bind(model).bind(effort.as_str()).bind(json!({"reasoningEffort":effort}))
            .bind(&ids)
            .bind(batch.iter().map(|c| c.title.as_str()).collect::<Vec<_>>())
            .bind(batch.iter().map(|c| c.archived).collect::<Vec<_>>())
            .bind(batch.iter().map(|c| c.pinned).collect::<Vec<_>>())
            .bind(batch.iter().map(|c| c.created_at).collect::<Vec<_>>())
            .bind(batch.iter().map(|c| c.updated_at).collect::<Vec<_>>())
            .bind(batch.iter().map(|c| c.key.as_deref()).collect::<Vec<_>>())
            .execute(&mut *tx).await?;
        let mut rows = MessageRows::default();
        let mut leaves = Vec::with_capacity(batch.len());
        for (chat, &conversation) in batch.iter_mut().zip(&ids) {
            let message_ids: Vec<Uuid> = chat.messages.iter().map(|_| Uuid::new_v4()).collect();
            leaves.push(message_ids[chat.leaf]);
            for (ordinal, m) in chat.messages.drain(..).enumerate() {
                rows.id.push(message_ids[ordinal]);
                rows.conversation.push(conversation);
                rows.ordinal.push(ordinal as i32);
                rows.role.push(m.role.as_str());
                rows.status.push(if m.error.is_some() {
                    "error"
                } else {
                    "completed"
                });
                rows.generation
                    .push((m.role == imports::Role::Assistant).then(Uuid::new_v4));
                rows.model.push(m.model);
                rows.error.push(m.error);
                rows.created.push(m.created_at);
                rows.parent.push(m.parent.map(|p| message_ids[p]));
                rows.child.push(m.selected_child.map(|c| message_ids[c]));
                rows.content.push(m.content);
                rows.reasoning.push(m.reasoning);
            }
        }
        // Parent and selected-child links point within the same statement's rows.
        sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,reasoning_content,status,generation_id,settings,error_message,created_at,parent_message_id,selected_child_id)
            SELECT u.id,u.conversation_id,u.ordinal,u.role,u.content,u.reasoning,u.status,u.generation_id,
              CASE WHEN u.model IS NULL THEN NULL ELSE jsonb_build_object('model',u.model) END,u.error,u.created_at,u.parent,u.child
            FROM unnest($1::uuid[],$2::uuid[],$3::integer[],$4::text[],$5::text[],$6::text[],$7::text[],$8::uuid[],$9::text[],$10::text[],$11::timestamptz[],$12::uuid[],$13::uuid[])
              AS u(id,conversation_id,ordinal,role,content,reasoning,status,generation_id,model,error,created_at,parent,child)")
            .bind(&rows.id).bind(&rows.conversation).bind(&rows.ordinal).bind(&rows.role).bind(&rows.content).bind(&rows.reasoning)
            .bind(&rows.status).bind(&rows.generation).bind(&rows.model).bind(&rows.error).bind(&rows.created).bind(&rows.parent).bind(&rows.child)
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE conversations c SET selected_leaf_id=u.leaf FROM unnest($2::uuid[],$3::uuid[]) AS u(id,leaf) WHERE c.id=u.id AND c.owner_id=$1")
            .bind(owner).bind(&ids).bind(&leaves).execute(&mut *tx).await?;
        tx.commit().await?;
        summary.already_imported += already;
        summary.over_limit += over;
        summary.imported += ids.len();
        Ok(over == 0)
    }
}

#[derive(Default)]
struct MessageRows {
    id: Vec<Uuid>,
    conversation: Vec<Uuid>,
    ordinal: Vec<i32>,
    role: Vec<&'static str>,
    content: Vec<String>,
    reasoning: Vec<String>,
    status: Vec<&'static str>,
    generation: Vec<Option<Uuid>>,
    model: Vec<Option<String>>,
    error: Vec<Option<String>>,
    created: Vec<DateTime<Utc>>,
    parent: Vec<Option<Uuid>>,
    child: Vec<Option<Uuid>>,
}

fn text_bytes(chat: &ImportedChat) -> usize {
    chat.messages
        .iter()
        .map(|m| m.content.len() + m.reasoning.len())
        .sum()
}
