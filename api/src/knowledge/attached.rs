//! Long documents attached to a chat message. A document too long for the model to
//! read whole is indexed into a collection of its own, which its chat searches like
//! any other: passages that answer each message go to the model, which can search for
//! more and cites pages, slides and sheets. The collection is made when the document
//! is uploaded, so indexing starts while the person is still writing; the first chat
//! the document is sent in takes it, and it goes with that chat.

use super::Knowledge;
use crate::{attachments::AttachmentMeta, error::ApiError};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Makes the uploaded document's collection and queues it for indexing, in the
/// upload's transaction.
pub async fn stage_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    file: &AttachmentMeta,
) -> Result<(), ApiError> {
    stage(tx, owner, file.id, None).await.map(|_| ())
}

/// A collection named after the file, holding one document that reads the
/// attachment's bytes.
async fn stage(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    attachment: Uuid,
    conversation: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let collection = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO knowledge_collections (id, owner_id, name, attachment_id, conversation_id)
         SELECT $1, $2, LEFT(a.name, 120), a.id, $4 FROM attachments a WHERE a.id=$3 AND a.owner_id=$2",
    )
    .bind(collection)
    .bind(owner)
    .bind(attachment)
    .bind(conversation)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_documents (id, collection_id, owner_id, name, mime_type, size)
         SELECT $1, $2, $3, LEFT(a.name, 255), a.mime_type, a.size FROM attachments a WHERE a.id=$4 AND a.owner_id=$3",
    )
    .bind(Uuid::new_v4())
    .bind(collection)
    .bind(owner)
    .bind(attachment)
    .execute(&mut **tx)
    .await?;
    Ok(collection)
}

impl Knowledge {
    /// The collections of a turn's long documents. Each is the chat's own: the one
    /// made on upload if no chat has taken it yet, or a new one, indexed again, when
    /// the file was already sent in another chat.
    pub async fn attached(
        &self,
        owner: Uuid,
        conversation: Uuid,
        attachments: &[Uuid],
    ) -> Result<Vec<Uuid>, ApiError> {
        let mut tx = self.pool.begin().await?;
        let mut collections = Vec::new();
        for attachment in attachments {
            let mine: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM knowledge_collections WHERE owner_id=$1 AND attachment_id=$2 AND conversation_id=$3",
            )
            .bind(owner)
            .bind(attachment)
            .bind(conversation)
            .fetch_optional(&mut *tx)
            .await?;
            let taken: Option<Uuid> = match mine {
                Some(id) => Some(id),
                None => sqlx::query_scalar(
                    "UPDATE knowledge_collections SET conversation_id=$3, updated_at=NOW()
                     WHERE id = (SELECT id FROM knowledge_collections WHERE owner_id=$1 AND attachment_id=$2
                                 AND conversation_id IS NULL LIMIT 1 FOR UPDATE SKIP LOCKED)
                     RETURNING id",
                )
                .bind(owner)
                .bind(attachment)
                .bind(conversation)
                .fetch_optional(&mut *tx)
                .await?,
            };
            let id = match taken {
                Some(id) => id,
                None => stage(&mut tx, owner, *attachment, Some(conversation)).await?,
            };
            collections.push(id);
        }
        tx.commit().await?;
        Ok(collections)
    }

    /// Whether any document in the collections is still waiting or being indexed.
    pub async fn indexing(&self, collections: &[Uuid]) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM knowledge_documents WHERE collection_id = ANY($1) AND status IN ('queued', 'processing'))",
        )
        .bind(collections)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }
}
