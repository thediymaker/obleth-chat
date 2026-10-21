use super::*;
use crate::models::ParameterOverrides;
use sqlx::Row;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryQuery {
    pub query: Option<String>,
    pub archived: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub conversations: Vec<ConversationSummary>,
    pub total: i64,
    pub has_more: bool,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPatch {
    pub title: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub settings: Option<ParameterOverrides>,
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkResult {
    pub attachments: Vec<AttachmentMeta>,
    pub conversation: Conversation,
    pub draft: String,
    pub settings: Value,
}

/// Portable, bounded interchange. Import ignores old IDs and never accepts owners,
/// pending generations, sessions, or provider configuration from a document.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatArchive {
    pub format: String,
    pub version: u32,
    pub conversations: Vec<ArchivedChat>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchivedChat {
    pub title: String,
    pub model: String,
    pub reasoning_effort: ReasoningEffort,
    #[serde(default)]
    pub settings: ParameterOverrides,
    #[serde(default)]
    pub messages: Vec<ArchivedMessage>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchivedMessage {
    pub role: String,
    #[serde(default)]
    pub attachments: Vec<attachments::ArchivedAttachment>,
    pub content: String,
    #[serde(default)]
    pub reasoning_content: String,
    pub status: String,
    pub settings: Option<Value>,
    pub usage: Option<Value>,
    pub finish_reason: Option<String>,
    pub error_message: Option<String>,
}
const SUMMARY: &str =
    "id,title,model,reasoning_effort,updated_at,settings,archived,pinned,parent_id";
pub const ARCHIVE_BYTES: usize = 10 * 1024 * 1024;
/// Conversations per export, import, or bulk history action.
pub const MAX_ARCHIVE_CONVERSATIONS: usize = 50;

impl Store {
    pub async fn history(&self, owner: Uuid, q: HistoryQuery) -> Result<HistoryPage, ApiError> {
        let query = q.query.unwrap_or_default().trim().to_owned();
        if query.chars().count() > 200 || query.contains('\0') {
            return Err(ApiError::bad("Search must be at most 200 characters"));
        }
        let archived = match q.archived.as_deref().unwrap_or("false") {
            "true" => Some(true),
            "false" => Some(false),
            "all" => None,
            _ => return Err(ApiError::bad("Archived must be true, false, or all")),
        };
        let limit = q.limit.unwrap_or(50);
        let offset = q.offset.unwrap_or(0);
        if !(1..=100).contains(&limit) || !(0..=1000).contains(&offset) {
            return Err(ApiError::bad(
                "Use a page size of 1–100 and offset of 0–1000",
            ));
        }
        // Search runs without the user's row lock, which checkpoints and turn
        // completion take under their own short timeouts. Recover first, separately.
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        tx.commit().await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='5s'")
            .execute(&mut *tx)
            .await?;
        // Never create a full-message tsvector: valid stored messages can exceed
        // PostgreSQL's vector-size limit. Each hits branch is gated on the query
        // shape and runs once, not per conversation. Single terms and positional
        // phrases match the overlapping chunk vectors in one join that can use the
        // GIN index. For
        // non-phrase Boolean queries, gather only matching query lexemes into a
        // bounded vector so AND/NOT still work when words lie in different chunks;
        // only messages with a lexeme hit can match unless an empty vector
        // satisfies the query (pure negation). Lexeme-free queries match substrings.
        let matching = "WITH owned AS (
              SELECT id FROM conversations WHERE owner_id=$1 AND ($3::boolean IS NULL OR archived=$3)
            ), hits AS (
              SELECT m.conversation_id FROM messages m JOIN owned o ON o.id=m.conversation_id
                WHERE $2<>'' AND numnode(websearch_to_tsquery('simple',$2))=0
                  AND strpos(lower(m.content),lower($2))>0
              UNION ALL
              SELECT m.conversation_id FROM message_search_chunks s
                JOIN messages m ON m.id=s.message_id JOIN owned o ON o.id=m.conversation_id
                WHERE numnode(websearch_to_tsquery('simple',$2))>0
                  AND (numnode(websearch_to_tsquery('simple',$2))=1 OR strpos($2,'\"')>0)
                  AND s.document @@ websearch_to_tsquery('simple',$2)
              UNION ALL
              SELECT m.conversation_id FROM messages m JOIN owned o ON o.id=m.conversation_id
                WHERE numnode(websearch_to_tsquery('simple',$2))>1 AND strpos($2,'\"')=0
                  AND (''::tsvector @@ websearch_to_tsquery('simple',$2) OR m.id IN (
                    SELECT s.message_id FROM unnest(tsvector_to_array(to_tsvector('simple',$2))) AS words(lexeme)
                      JOIN message_search_chunks s ON s.document @@ plainto_tsquery('simple',words.lexeme)))
                  AND array_to_tsvector(ARRAY(
                    SELECT lexeme FROM unnest(tsvector_to_array(to_tsvector('simple',$2))) AS words(lexeme)
                    WHERE EXISTS(SELECT 1 FROM message_search_chunks s WHERE s.message_id=m.id
                      AND s.document @@ plainto_tsquery('simple',lexeme))
                  )) @@ websearch_to_tsquery('simple',$2)
            )";
        let filter = "id IN (SELECT id FROM owned) AND ($2='' OR strpos(lower(title),lower($2))>0
            OR id IN (SELECT conversation_id FROM hits))";
        let rows = sqlx::query(&format!(
            "{matching} SELECT {SUMMARY},COUNT(*) OVER () AS total FROM conversations WHERE {filter}
             ORDER BY pinned DESC,updated_at DESC,id LIMIT $4 OFFSET $5"
        ))
        .bind(owner)
        .bind(&query)
        .bind(archived)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await
        .map_err(search_error)?;
        let total: i64 = match rows.first() {
            Some(row) => row.try_get("total")?,
            None if offset == 0 => 0,
            // A page past the end has no row to carry the window count.
            None => sqlx::query_scalar(&format!(
                "{matching} SELECT COUNT(*) FROM conversations WHERE {filter}"
            ))
            .bind(owner)
            .bind(&query)
            .bind(archived)
            .fetch_one(&mut *tx)
            .await
            .map_err(search_error)?,
        };
        let conversations = rows
            .iter()
            .map(ConversationSummary::from_row)
            .collect::<Result<Vec<_>, _>>()?;
        tx.commit().await?;
        Ok(HistoryPage {
            conversations,
            total,
            has_more: offset + limit < total,
        })
    }

    pub async fn patch(
        &self,
        owner: Uuid,
        id: Uuid,
        patch: ConversationPatch,
    ) -> Result<Conversation, ApiError> {
        if let Some(title) = &patch.title {
            validate_title(title)?;
        }
        if let Some(settings) = &patch.settings {
            settings.validate()?;
        }
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let original = read_conversation(&mut tx, owner, id).await?;
        let active: Option<Uuid> = sqlx::query_scalar(
            "SELECT active_generation FROM conversations WHERE id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if active.is_some()
            && (patch.settings.is_some()
                || patch.reasoning_effort.is_some()
                || patch.archived == Some(true))
        {
            return Err(conflict("Stop the response before changing generation settings or archiving this conversation"));
        }
        let mut settings = original.settings;
        if let Some(p) = patch.settings {
            for (k, v) in json!(p).as_object().unwrap() {
                settings[k] = v.clone();
            }
        }
        if let Some(effort) = patch.reasoning_effort {
            settings["reasoningEffort"] = json!(effort);
        }
        let effort = settings
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .unwrap_or(&original.reasoning_effort);
        sqlx::query("UPDATE conversations SET title=COALESCE($3,title),reasoning_effort=$4,settings=$5,archived=COALESCE($6,archived),pinned=COALESCE($7,pinned),updated_at=clock_timestamp() WHERE id=$1 AND owner_id=$2")
            .bind(id).bind(owner).bind(patch.title.as_deref().map(str::trim)).bind(effort).bind(&settings).bind(patch.archived).bind(patch.pinned)
            .execute(&mut *tx).await?;
        let conversation = read_conversation(&mut tx, owner, id).await?;
        tx.commit().await?;
        Ok(conversation)
    }

    pub async fn fork_turn(
        &self,
        owner: Uuid,
        id: Uuid,
        message: Uuid,
        mode: &str,
        edited: Option<&str>,
    ) -> Result<ForkResult, ApiError> {
        if !matches!(mode, "edit" | "regenerate") {
            return Err(ApiError::bad("Choose edit or regenerate"));
        }
        if edited.is_some_and(|s| {
            s.trim().is_empty() || s.chars().count() > MAX_USER_MESSAGE_CHARS || s.contains('\0')
        }) {
            return Err(ApiError::bad(
                "Messages must contain 1 to 12,000 characters",
            ));
        }
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let original = read_conversation(&mut tx, owner, id).await?;
        if original.messages.iter().any(|m| m.status == "pending") {
            return Err(conflict("Stop the response before editing or retrying it"));
        }
        require_conversation_capacity(&mut tx, owner).await?;
        let selected = original
            .messages
            .iter()
            .position(|m| m.id == message)
            .ok_or_else(ApiError::not_found)?;
        let user_index = if mode == "edit" {
            if original.messages[selected].role != "user" {
                return Err(ApiError::bad("Select a user message to edit"));
            }
            selected
        } else {
            (0..=selected)
                .rev()
                .find(|&i| original.messages[i].role == "user")
                .ok_or_else(ApiError::not_found)?
        };
        let draft = edited
            .unwrap_or(&original.messages[user_index].content)
            .to_owned();
        let snapshot = original
            .messages
            .get(user_index + 1)
            .and_then(|m| m.settings.as_ref());
        let mut settings = snapshot
            .cloned()
            .unwrap_or_else(|| original.settings.clone());
        if let Some(map) = settings.as_object_mut() {
            map.remove("model");
        }
        let parameters: ParameterOverrides =
            serde_json::from_value(settings.clone()).map_err(|_| ApiError::internal())?;
        parameters.validate()?;
        let new_id = Uuid::new_v4();
        sqlx::query("INSERT INTO conversations(id,owner_id,title,model,reasoning_effort,settings,parent_id,branch_point_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(new_id).bind(owner).bind(&original.title).bind(&original.model).bind(&original.reasoning_effort).bind(&settings)
            .bind(original.parent_id.unwrap_or(id)).bind(original.messages[user_index].id).execute(&mut *tx).await?;
        let selected = original.messages[..user_index]
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>();
        copy_selected_messages(&mut tx, id, new_id, &selected).await?;
        let conversation = read_conversation(&mut tx, owner, new_id).await?;
        tx.commit().await?;
        Ok(ForkResult {
            attachments: original.messages[user_index].attachments.clone(),
            conversation,
            draft,
            settings,
        })
    }

    pub async fn branches(
        &self,
        owner: Uuid,
        id: Uuid,
    ) -> Result<Vec<ConversationSummary>, ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        let original = read_conversation(&mut tx, owner, id).await?;
        let root = original.parent_id.unwrap_or(id);
        let rows=sqlx::query_as::<_,ConversationSummary>(&format!("SELECT {SUMMARY} FROM conversations WHERE owner_id=$1 AND (id=$2 OR parent_id=$2) ORDER BY created_at,id LIMIT 100"))
            .bind(owner).bind(root).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn bulk(&self, owner: Uuid, ids: &[Uuid], action: &str) -> Result<(), ApiError> {
        validate_ids(ids)?;
        if !matches!(action, "archive" | "unarchive" | "delete") {
            return Err(ApiError::bad("Unknown history action"));
        }
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let rows:Vec<Option<Uuid>>=sqlx::query_scalar("SELECT active_generation FROM conversations WHERE owner_id=$1 AND id=ANY($2) FOR UPDATE")
            .bind(owner).bind(ids).fetch_all(&mut *tx).await?;
        if rows.len() != ids.len() {
            return Err(ApiError::not_found());
        }
        if rows.iter().any(Option::is_some) {
            return Err(conflict(
                "Stop active responses before updating these conversations",
            ));
        }
        if action == "delete" {
            sqlx::query("DELETE FROM conversations WHERE owner_id=$1 AND id=ANY($2)")
                .bind(owner)
                .bind(ids)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("UPDATE conversations SET archived=$3,updated_at=clock_timestamp() WHERE owner_id=$1 AND id=ANY($2)").bind(owner).bind(ids).bind(action=="archive").execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn export(&self, owner: Uuid, ids: &[Uuid]) -> Result<ChatArchive, ApiError> {
        validate_ids(ids)?;
        let mut tx = self.owner_transaction(owner).await?;
        let mut conversations = Vec::new();
        let mut bytes = 0usize;
        for id in ids {
            let chat = read_conversation(&mut tx, owner, *id).await?;
            if chat.messages.iter().any(|m| m.status == "pending") {
                return Err(conflict("Stop responses before exporting"));
            }
            let ids = chat.messages.iter().map(|m| m.id).collect::<Vec<_>>();
            let mut files = attachments::load_for_archive(
                &mut tx,
                owner,
                &ids,
                ARCHIVE_BYTES.saturating_sub(bytes),
            )
            .await?;
            let exported = ArchivedChat {
                title: chat.title,
                model: chat.model,
                reasoning_effort: serde_json::from_value(json!(chat.reasoning_effort))
                    .map_err(|_| ApiError::internal())?,
                settings: serde_json::from_value(chat.settings)
                    .map_err(|_| ApiError::internal())?,
                messages: chat
                    .messages
                    .into_iter()
                    .map(|m| ArchivedMessage {
                        attachments: files
                            .remove(&m.id)
                            .unwrap_or_default()
                            .iter()
                            .map(Attachment::archive)
                            .collect(),
                        role: m.role,
                        content: m.content,
                        reasoning_content: m.reasoning_content,
                        status: m.status,
                        settings: m.settings,
                        usage: m.usage,
                        finish_reason: m.finish_reason,
                        error_message: m.error_message,
                    })
                    .collect(),
            };
            bytes += serde_json::to_vec(&exported)
                .map_err(|_| ApiError::internal())?
                .len();
            if bytes > ARCHIVE_BYTES - 1024 {
                return Err(ApiError::bad(
                    "This export exceeds 10 MiB. Select fewer conversations.",
                ));
            }
            conversations.push(exported);
        }
        tx.commit().await?;
        Ok(ChatArchive {
            format: "obleth-chat".into(),
            version: 1,
            conversations,
        })
    }

    pub async fn import(
        &self,
        owner: Uuid,
        archive: ChatArchive,
    ) -> Result<Vec<ConversationSummary>, ApiError> {
        if archive.format != "obleth-chat"
            || archive.version != 1
            || archive.conversations.is_empty()
            || archive.conversations.len() > MAX_ARCHIVE_CONVERSATIONS
        {
            return Err(ApiError::bad(
                "Choose an Obleth Chat v1 export containing 1–50 conversations",
            ));
        }
        if serde_json::to_vec(&archive)
            .map_err(|_| ApiError::bad("Invalid archive"))?
            .len()
            > ARCHIVE_BYTES
        {
            return Err(ApiError::bad("Import exceeds 10 MiB"));
        }
        for chat in &archive.conversations {
            validate_title(&chat.title)?;
            validate_model(&chat.model)?;
            chat.settings.validate()?;
            if chat.messages.len() > MAX_HISTORY {
                return Err(ApiError::bad(
                    "An imported conversation exceeds 200 messages",
                ));
            }
            let mut bytes = 0;
            for m in &chat.messages {
                if !matches!(m.role.as_str(), "user" | "assistant")
                    || !matches!(m.status.as_str(), "completed" | "error" | "interrupted")
                    || (m.role == "user"
                        && (m.status != "completed" || !m.reasoning_content.is_empty()))
                    || m.content.contains('\0')
                    || m.reasoning_content.contains('\0')
                {
                    return Err(ApiError::bad("Invalid imported message"));
                }
                // Regenerating reuses stored prompts, which must meet the send limit.
                if m.role == "user" && m.content.chars().count() > MAX_USER_MESSAGE_CHARS {
                    return Err(ApiError::bad(
                        "An imported user message exceeds 12,000 characters",
                    ));
                }
                if m.attachments.len() > attachments::MAX_PER_MESSAGE
                    || (m.role != "user" && !m.attachments.is_empty())
                {
                    return Err(ApiError::bad(
                        "Only user messages can contain up to four attachments",
                    ));
                }
                let size = m.content.len() + m.reasoning_content.len();
                bytes += size;
                if size > MAX_RESPONSE_BYTES || bytes > MAX_CONVERSATION_BYTES {
                    return Err(ApiError::bad("Imported messages exceed the storage limit"));
                }
                if m.error_message
                    .as_ref()
                    .is_some_and(|v| v.chars().count() > 1000)
                    || m.finish_reason.as_ref().is_some_and(|v| v.len() > 80)
                {
                    return Err(ApiError::bad("Invalid imported message result"));
                }
                if let Some(v) = &m.settings {
                    // Version selection copies a snapshot model into the conversation.
                    let mut map = v
                        .as_object()
                        .cloned()
                        .ok_or_else(|| ApiError::bad("Invalid imported generation settings"))?;
                    if let Some(model) = map.remove("model") {
                        validate_model(model.as_str().ok_or_else(|| {
                            ApiError::bad("Invalid imported generation settings")
                        })?)?;
                    }
                    let p: ParameterOverrides = serde_json::from_value(Value::Object(map))
                        .map_err(|_| ApiError::bad("Invalid imported generation settings"))?;
                    p.validate()?;
                }
                if let Some(v) = &m.usage {
                    if !v.as_object().is_some_and(|m| {
                        m.len() <= 3
                            && m.iter().all(|(k, v)| {
                                matches!(
                                    k.as_str(),
                                    "prompt_tokens" | "completion_tokens" | "total_tokens"
                                ) && v.as_u64().is_some()
                            })
                    }) {
                        return Err(ApiError::bad("Invalid imported token usage"));
                    }
                }
            }
        }
        let mut tx = self.owner_transaction(owner).await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE owner_id=$1")
            .bind(owner)
            .fetch_one(&mut *tx)
            .await?;
        if count + archive.conversations.len() as i64 > MAX_CONVERSATIONS_PER_USER {
            return Err(ApiError::bad(
                "Import would exceed the 1,000 conversation limit",
            ));
        }
        let mut imported = Vec::new();
        for chat in archive.conversations {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO conversations(id,owner_id,title,model,reasoning_effort,settings) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(id).bind(owner).bind(chat.title.trim()).bind(&chat.model).bind(chat.reasoning_effort.as_str()).bind(json!(chat.settings)).execute(&mut *tx).await?;
            for (ordinal, m) in chat.messages.into_iter().enumerate() {
                let message_id = Uuid::new_v4();
                let file_ids = attachments::import_in(&mut tx, owner, &m.attachments).await?;
                sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,reasoning_content,status,generation_id,settings,usage,finish_reason,error_message) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
                    .bind(message_id).bind(id).bind(ordinal as i32).bind(&m.role).bind(m.content).bind(m.reasoning_content).bind(m.status)
                    .bind(if m.role=="assistant"{Some(Uuid::new_v4())}else{None}).bind(m.settings).bind(m.usage).bind(m.finish_reason).bind(m.error_message).execute(&mut *tx).await?;
                attachments::link_message(&mut tx, owner, message_id, &file_ids).await?;
            }
            imported.push(
                sqlx::query_as::<_, ConversationSummary>(&format!(
                    "SELECT {SUMMARY} FROM conversations WHERE id=$1 AND owner_id=$2"
                ))
                .bind(id)
                .bind(owner)
                .fetch_one(&mut *tx)
                .await?,
            );
        }
        tx.commit().await?;
        Ok(imported)
    }
}
fn search_error(error: sqlx::Error) -> ApiError {
    match error.as_database_error().and_then(|e| e.code()).as_deref() {
        Some("57014") => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Search took too long. Try a more specific search.".into(),
        ),
        _ => error.into(),
    }
}
fn validate_ids(ids: &[Uuid]) -> Result<(), ApiError> {
    if ids.is_empty()
        || ids.len() > MAX_ARCHIVE_CONVERSATIONS
        || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
    {
        return Err(ApiError::bad("Select 1–50 distinct conversations"));
    }
    Ok(())
}
