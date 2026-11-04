//! A conversation keeps one selected path through immutable message alternatives.
//! Parent links preserve context; selected-child links remember each alternative's
//! continuation so switching away and back also restores its later turns.
use super::*;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantContext {
    pub draft: String,
    pub attachments: Vec<AttachmentMeta>,
    pub settings: Value,
    pub model: String,
}
/// Resolves the user message a variant replaces on a selected path of (id, role).
pub(super) fn source_index(
    path: &[(Uuid, &str)],
    message: Uuid,
    mode: &str,
) -> Result<usize, ApiError> {
    let index = path
        .iter()
        .position(|m| m.0 == message)
        .ok_or_else(ApiError::not_found)?;
    match mode {
        "edit" if path[index].1 == "user" => Ok(index),
        "regenerate" if path[index].1 == "assistant" => path[..index]
            .iter()
            .rposition(|m| m.1 == "user")
            .ok_or_else(|| ApiError::bad("This response has no preceding user message")),
        "edit" | "regenerate" => Err(ApiError::bad(
            "Edit a user message or regenerate an assistant response",
        )),
        _ => Err(ApiError::bad("Choose edit or regenerate")),
    }
}
impl Store {
    /// Pre-admission preview: walks only the selected path's ids, roles and settings,
    /// then reads the one source message. start_turn_inner re-resolves the source under
    /// the conversation lock and stays the only full transcript load of a turn.
    pub async fn variant_context(
        &self,
        owner: Uuid,
        id: Uuid,
        message: Uuid,
        mode: &str,
    ) -> Result<VariantContext, ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let (saved, conversation_model, active): (Value, String, Option<Uuid>) = sqlx::query_as(
            "SELECT settings,model,active_generation FROM conversations WHERE id=$1 AND owner_id=$2",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
        if active.is_some() {
            return Err(conflict(
                "Stop the active response before editing or regenerating",
            ));
        }
        initialize_path_in(&mut tx, id).await?;
        // read_messages' walk and bound, without content, attachments or versions.
        let path:Vec<(Uuid,String,Option<Value>)>=sqlx::query_as("WITH RECURSIVE path AS(
          SELECT m.id,m.parent_message_id,m.role,m.settings,0 AS depth FROM messages m JOIN conversations c ON c.selected_leaf_id=m.id WHERE c.id=$1 AND c.owner_id=$2 AND m.conversation_id=c.id
          UNION ALL SELECT m.id,m.parent_message_id,m.role,m.settings,p.depth+1 FROM messages m JOIN path p ON m.id=p.parent_message_id WHERE m.conversation_id=$1 AND p.depth<200
        ) SELECT id,role,settings FROM path ORDER BY depth DESC").bind(id).bind(owner).fetch_all(&mut *tx).await?;
        let roles = path.iter().map(|m| (m.0, m.1.as_str())).collect::<Vec<_>>();
        let index = source_index(&roles, message, mode)?;
        let source = path[index].0;
        let draft: String =
            sqlx::query_scalar("SELECT content FROM messages WHERE id=$1 AND conversation_id=$2")
                .bind(source)
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        let attachments = attachments::metadata_for_messages(&mut tx, owner, &[source])
            .await?
            .remove(&source)
            .unwrap_or_default();
        tx.commit().await?;
        let snapshot = path.get(index + 1).and_then(|m| m.2.as_ref());
        let mut settings = snapshot.cloned().unwrap_or(saved);
        let model = settings
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&conversation_model)
            .to_owned();
        if let Some(map) = settings.as_object_mut() {
            map.remove("model");
        }
        Ok(VariantContext {
            draft,
            attachments,
            settings,
            model,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_variant_turn(
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
        source_message_id: Uuid,
        mode: &str,
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
            Some((source_message_id, mode)),
        )
        .await
    }

    pub async fn select_version(
        &self,
        owner: Uuid,
        id: Uuid,
        message_id: Uuid,
    ) -> Result<Conversation, ApiError> {
        let mut tx = self.owner_transaction(owner).await?;
        recover_expired_in(&mut tx, owner).await?;
        let active: Option<Uuid> = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT active_generation FROM conversations WHERE id=$1 AND owner_id=$2 FOR UPDATE",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
        if active.is_some() {
            return Err(conflict(
                "Stop the active response before switching versions",
            ));
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id=$1 AND conversation_id=$2)",
        )
        .bind(message_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(ApiError::not_found());
        }
        // Restore this entire ancestor choice, including a previously edited
        // root prompt, before following its remembered continuation.
        sqlx::query("WITH RECURSIVE path AS(
          SELECT id,parent_message_id,0 AS depth FROM messages WHERE id=$1 AND conversation_id=$2
          UNION ALL SELECT m.id,m.parent_message_id,p.depth+1 FROM messages m JOIN path p ON m.id=p.parent_message_id WHERE m.conversation_id=$2 AND p.depth<$3
        ) UPDATE messages m SET selected_child_id=p.id FROM path p WHERE m.id=p.parent_message_id AND m.conversation_id=$2")
            .bind(message_id).bind(id).bind(MAX_HISTORY as i32).execute(&mut *tx).await?;
        let leaf:Uuid=sqlx::query_scalar("WITH RECURSIVE path AS(
          SELECT id,selected_child_id,0 AS depth FROM messages WHERE id=$1 AND conversation_id=$2
          UNION ALL SELECT m.id,m.selected_child_id,p.depth+1 FROM messages m JOIN path p ON m.id=p.selected_child_id WHERE m.conversation_id=$2 AND p.depth<$3
        ) SELECT id FROM path ORDER BY depth DESC LIMIT 1").bind(message_id).bind(id).bind(MAX_HISTORY as i32).fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE conversations SET selected_leaf_id=$2,updated_at=clock_timestamp() WHERE id=$1 AND owner_id=$3").bind(id).bind(leaf).bind(owner).execute(&mut *tx).await?;
        let mut conversation = read_conversation(&mut tx, owner, id).await?;
        if let Some(snapshot) = conversation
            .messages
            .iter()
            .rev()
            .find_map(|m| m.settings.as_ref())
            .filter(|s| s.is_object())
        {
            let mut settings = snapshot.clone();
            // Snapshots predating import validation may hold an unusable model.
            let model = settings
                .get("model")
                .and_then(Value::as_str)
                .filter(|m| validate_model(m).is_ok())
                .unwrap_or(&conversation.model)
                .to_owned();
            if let Some(map) = settings.as_object_mut() {
                map.remove("model");
            }
            let effort = settings
                .get("reasoningEffort")
                .and_then(Value::as_str)
                .unwrap_or(&conversation.reasoning_effort)
                .to_owned();
            sqlx::query("UPDATE conversations SET model=$2,settings=$3,reasoning_effort=$4 WHERE id=$1 AND owner_id=$5").bind(id).bind(&model).bind(&settings).bind(&effort).bind(owner).execute(&mut *tx).await?;
            conversation.model = model;
            conversation.settings = settings;
            conversation.reasoning_effort = effort;
        }
        tx.commit().await?;
        Ok(conversation)
    }
}

/// Newly imported or explicitly copied linear transcripts initialize lazily under
/// their owner's lock. Existing production transcripts are backfilled by 010.
pub(super) async fn initialize_path_in(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(), ApiError> {
    let uninitialized: bool =
        sqlx::query_scalar("SELECT selected_leaf_id IS NULL FROM conversations WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if uninitialized {
        sqlx::query("WITH links AS(SELECT id,lag(id) OVER(ORDER BY ordinal) AS parent,lead(id) OVER(ORDER BY ordinal) AS child FROM messages WHERE conversation_id=$1) UPDATE messages m SET parent_message_id=l.parent,selected_child_id=l.child FROM links l WHERE m.id=l.id").bind(id).execute(&mut **tx).await?;
        sqlx::query("UPDATE conversations SET selected_leaf_id=(SELECT id FROM messages WHERE conversation_id=$1 ORDER BY ordinal DESC LIMIT 1) WHERE id=$1").bind(id).execute(&mut **tx).await?;
    }
    Ok(())
}
pub(super) async fn populate_versions(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    messages: &mut [Message],
) -> Result<(), ApiError> {
    type VersionRow = (Uuid, Option<Uuid>, String, String, DateTime<Utc>);
    let rows:Vec<VersionRow>=sqlx::query_as("SELECT id,parent_message_id,role,status,created_at FROM messages WHERE conversation_id=$1 ORDER BY ordinal").bind(id).fetch_all(&mut **tx).await?;
    let mut groups: HashMap<(Option<Uuid>, String), Vec<MessageVersion>> = HashMap::new();
    for (id, parent, role, status, created_at) in rows {
        groups
            .entry((parent, role))
            .or_default()
            .push(MessageVersion {
                id,
                status,
                created_at,
            });
    }
    for message in messages {
        message.versions = groups
            .get(&(message.parent_message_id, message.role.clone()))
            .cloned()
            .unwrap_or_default();
        message.version_index = message
            .versions
            .iter()
            .position(|v| v.id == message.id)
            .map(|i| i + 1)
            .unwrap_or(1);
    }
    Ok(())
}
