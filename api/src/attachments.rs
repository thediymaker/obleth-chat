//! Owner-scoped, bounded file uploads and durable references for model context.
//! Files are never fetched from remote URLs and are never served as active content.
use std::collections::{HashMap, HashSet};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{error::ApiError, AppState};

type MessageAttachmentRow = (Uuid, Uuid, String, String, i32, String, Vec<u8>);

pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_USER_BYTES: i64 = 50 * 1024 * 1024;
pub const MAX_USER_FILES: i64 = 100;
pub const MAX_PER_MESSAGE: usize = 4;
/// Encoded images, including replayed images, must fit in this inference budget.
pub const MAX_IMAGE_CONTEXT_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentMeta {
    pub id: Uuid,
    pub name: String,
    pub mime_type: String,
    pub size: i32,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub meta: AttachmentMeta,
    data: Vec<u8>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchivedAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
}
impl Attachment {
    pub fn archive(&self) -> ArchivedAttachment {
        ArchivedAttachment {
            name: self.meta.name.clone(),
            mime_type: self.meta.mime_type.clone(),
            data_base64: STANDARD.encode(&self.data),
        }
    }
    /// Filenames are represented as JSON strings to keep their boundaries clear.
    pub fn text_context(&self) -> Option<String> {
        (self.meta.kind == "text").then(|| {
            format!(
                "\n\nAttached file {}:\n{}\nEnd of attached file.\n",
                json!(self.meta.name),
                String::from_utf8_lossy(&self.data)
            )
        })
    }

    pub fn content_part(&self) -> Value {
        if let Some(text) = self.text_context() {
            json!({"type":"text","text":text})
        } else {
            json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",self.meta.mime_type,STANDARD.encode(&self.data))}})
        }
    }

    /// UTF-8 context bytes for text, serialized image-part bytes for images.
    pub fn context_bytes(&self) -> usize {
        self.text_context()
            .map(|text| text.len())
            .unwrap_or_else(|| self.content_part().to_string().len())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Upload {
    name: String,
    #[serde(default)]
    mime_type: String,
    data_base64: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/attachments",
            get(list)
                .post(upload)
                .layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/api/attachments/{id}", get(download).delete(delete))
        .with_state(state)
}

async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let attachments = sqlx::query_as::<_, AttachmentMeta>(
        "SELECT id,name,mime_type,size,kind FROM attachments WHERE owner_id=$1 ORDER BY created_at DESC,id LIMIT 100",
    ).bind(user.id).fetch_all(&s.store.pool).await?;
    Ok(Json(
        json!({"attachments":attachments,"limits":{"maxFiles":MAX_USER_FILES,"maxBytes":MAX_USER_BYTES,"maxTextBytes":MAX_TEXT_BYTES,"maxImageBytes":MAX_IMAGE_BYTES,"maxPerMessage":MAX_PER_MESSAGE}}),
    ))
}

async fn upload(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Upload>,
) -> Result<(StatusCode, Json<AttachmentMeta>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let file = validate_upload(body)?;
    let mut tx = s.store.pool.begin().await?;
    lock_owner(&mut tx, user.id).await?;
    insert_in(&mut tx, user.id, &file).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(file.meta)))
}

async fn insert_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    file: &Attachment,
) -> Result<(), ApiError> {
    let (count, bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),COALESCE(SUM(size),0)::BIGINT FROM attachments WHERE owner_id=$1",
    )
    .bind(owner)
    .fetch_one(&mut **tx)
    .await?;
    if count >= MAX_USER_FILES || bytes + i64::from(file.meta.size) > MAX_USER_BYTES {
        return Err(ApiError(StatusCode::CONFLICT,"Your file storage is full (100 files or 50 MiB). Delete unused files before uploading more.".into()));
    }
    sqlx::query("INSERT INTO attachments(id,owner_id,name,mime_type,kind,size,data)VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(file.meta.id).bind(owner).bind(&file.meta.name).bind(&file.meta.mime_type)
        .bind(&file.meta.kind).bind(file.meta.size).bind(&file.data).execute(&mut **tx).await?;
    Ok(())
}

/// Portable archives carry file bytes explicitly. Reuse identical private files
/// across imported messages so replayed attachments do not multiply disk usage.
/// Nothing is committed independently of the caller's conversation transaction.
pub async fn import_in(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    files: &[ArchivedAttachment],
) -> Result<Vec<Uuid>, ApiError> {
    if files.len() > MAX_PER_MESSAGE {
        return Err(ApiError::bad("Attach at most four files to a message"));
    }
    if files.is_empty() {
        return Ok(vec![]);
    }
    lock_owner(tx, owner).await?;
    let mut ids = vec![];
    for item in files {
        let file = validate_upload(Upload {
            name: item.name.clone(),
            mime_type: item.mime_type.clone(),
            data_base64: item.data_base64.clone(),
        })?;
        let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM attachments WHERE owner_id=$1 AND name=$2 AND mime_type=$3 AND size=$4 AND data=$5 AND NOT(id=ANY($6)) ORDER BY created_at,id LIMIT 1")
            .bind(owner).bind(&file.meta.name).bind(&file.meta.mime_type).bind(file.meta.size).bind(&file.data).bind(&ids).fetch_optional(&mut **tx).await?;
        if let Some(id) = existing {
            ids.push(id);
        } else {
            insert_in(tx, owner, &file).await?;
            ids.push(file.meta.id);
        }
    }
    Ok(ids)
}

async fn download(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let row: Option<(String, String, Vec<u8>)> =
        sqlx::query_as("SELECT name,mime_type,data FROM attachments WHERE id=$1 AND owner_id=$2")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&s.store.pool)
            .await?;
    let (name, mime, data) = row.ok_or_else(ApiError::not_found)?;
    file_response(&name, &mime, data)
}

/// An attachment as a download, never rendered in the app's origin.
pub(crate) fn file_response(name: &str, mime: &str, data: Vec<u8>) -> Result<Response, ApiError> {
    let encoded_name: String = url::form_urlencoded::byte_serialize(name.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    let mut response = Response::new(Body::from(data));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).map_err(|_| ApiError::internal())?,
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"download\"; filename*=UTF-8''{encoded_name}"
        ))
        .map_err(|_| ApiError::internal())?,
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    Ok(response)
}

async fn delete(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let mut tx = s.store.pool.begin().await?;
    lock_owner(&mut tx, user.id).await?;
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM attachments WHERE id=$1 AND owner_id=$2 FOR UPDATE")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?;
    exists.ok_or_else(ApiError::not_found)?;
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM message_attachments WHERE attachment_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if used {
        return Err(ApiError(StatusCode::CONFLICT,"This file is used by a saved conversation. Delete those conversations before deleting the file.".into()));
    }
    sqlx::query("DELETE FROM attachments WHERE id=$1 AND owner_id=$2")
        .bind(id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn lock_owner(tx: &mut Transaction<'_, Postgres>, owner: Uuid) -> Result<(), ApiError> {
    let found: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id=$1 AND enabled=TRUE FOR UPDATE")
            .bind(owner)
            .fetch_optional(&mut **tx)
            .await?;
    found.ok_or_else(ApiError::unauthorized)?;
    Ok(())
}

fn validate_ids(ids: &[Uuid]) -> Result<(), ApiError> {
    if ids.len() > MAX_PER_MESSAGE || ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(ApiError::bad(
            "Attach at most four distinct files to a message",
        ));
    }
    Ok(())
}

fn require_vision(files: &[Attachment], vision: bool) -> Result<(), ApiError> {
    if !vision && files.iter().any(|file| file.meta.kind == "image") {
        return Err(ApiError::bad("This conversation contains images. Choose a model with confirmed image support to continue."));
    }
    Ok(())
}

/// Call inside the existing owner-locked turn transaction. Missing and other
/// users' IDs are indistinguishable. Input order is retained for model context.
pub async fn load_owner(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    ids: &[Uuid],
    vision: bool,
) -> Result<Vec<Attachment>, ApiError> {
    validate_ids(ids)?;
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows: Vec<(Uuid, String, String, i32, String, Vec<u8>)> = sqlx::query_as(
        "SELECT id,name,mime_type,size,kind,data FROM attachments WHERE owner_id=$1 AND id=ANY($2)",
    )
    .bind(owner)
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != ids.len() {
        return Err(ApiError::not_found());
    }
    let mut by_id: HashMap<_, _> = rows
        .into_iter()
        .map(|(id, name, mime_type, size, kind, data)| {
            (
                id,
                Attachment {
                    meta: AttachmentMeta {
                        id,
                        name,
                        mime_type,
                        size,
                        kind,
                    },
                    data,
                },
            )
        })
        .collect();
    let files: Vec<_> = ids
        .iter()
        .map(|id| by_id.remove(id).expect("all IDs checked above"))
        .collect();
    require_vision(&files, vision)?;
    Ok(files)
}

/// The message and every file must belong to this owner. User-row locking makes
/// quota changes, deletion, and linking serializable across application replicas.
pub async fn link_message(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    message: Uuid,
    ids: &[Uuid],
) -> Result<(), ApiError> {
    validate_ids(ids)?;
    if ids.is_empty() {
        return Ok(());
    }
    lock_owner(tx, owner).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages m JOIN conversations c ON c.id=m.conversation_id WHERE m.id=$1 AND c.owner_id=$2 AND m.role='user')")
        .bind(message).bind(owner).fetch_one(&mut **tx).await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE owner_id=$1 AND id=ANY($2)")
            .bind(owner)
            .bind(ids)
            .fetch_one(&mut **tx)
            .await?;
    if count != ids.len() as i64 {
        return Err(ApiError::not_found());
    }
    let linked: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_attachments WHERE message_id=$1)")
            .bind(message)
            .fetch_one(&mut **tx)
            .await?;
    if linked {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Files have already been attached to this message".into(),
        ));
    }
    sqlx::query("INSERT INTO message_attachments(message_id,attachment_id,position) SELECT $1,id,(ordinality-1)::SMALLINT FROM UNNEST($2::UUID[]) WITH ORDINALITY AS files(id,ordinality)")
        .bind(message).bind(ids).execute(&mut **tx).await?;
    Ok(())
}

/// Load all replayed files in one bounded query and check the current model's
/// capabilities again; switching models cannot bypass image support checks.
pub async fn load_messages(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    messages: &[Uuid],
    vision: bool,
) -> Result<HashMap<Uuid, Vec<Attachment>>, ApiError> {
    if messages.len() > crate::store::MAX_HISTORY {
        return Err(ApiError::bad("Too many messages"));
    }
    // Count repeated references before loading bytea: one 5 MiB upload replayed
    // hundreds of times must not create a multi-gigabyte database result.
    let (text_bytes,image_bytes): (i64,i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(CASE WHEN a.kind='text' THEN a.size ELSE 0 END),0)::BIGINT,COALESCE(SUM(CASE WHEN a.kind='image' THEN ((a.size::BIGINT+2)/3)*4+128 ELSE 0 END),0)::BIGINT FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2)",
    ).bind(owner).bind(messages).fetch_one(&mut **tx).await?;
    if image_bytes > 0 && !vision {
        return Err(ApiError::bad("This conversation contains images. Choose a model with confirmed image support to continue."));
    }
    if text_bytes > crate::store::MAX_CONTEXT_BYTES as i64
        || image_bytes > MAX_IMAGE_CONTEXT_BYTES as i64
    {
        return Err(ApiError::bad("Attached files exceed the conversation context limit. Start a new conversation or branch an earlier message."));
    }
    load_message_data(tx, owner, messages).await
}

/// Export is bounded by encoded size rather than inference capabilities.
pub async fn load_for_archive(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    messages: &[Uuid],
    max_bytes: usize,
) -> Result<HashMap<Uuid, Vec<Attachment>>, ApiError> {
    if messages.len() > crate::store::MAX_HISTORY {
        return Err(ApiError::bad("Too many messages"));
    }
    let bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(((a.size::BIGINT+2)/3)*4+OCTET_LENGTH(a.name)+OCTET_LENGTH(a.mime_type)+128),0)::BIGINT FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2)",
    ).bind(owner).bind(messages).fetch_one(&mut **tx).await?;
    if bytes > max_bytes.min(20 * 1024 * 1024) as i64 {
        return Err(ApiError::bad("These attachments exceed the archive size limit. Export a smaller conversation branch."));
    }
    load_message_data(tx, owner, messages).await
}

async fn load_message_data(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    messages: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Attachment>>, ApiError> {
    let rows: Vec<MessageAttachmentRow> = sqlx::query_as(
        "SELECT ma.message_id,a.id,a.name,a.mime_type,a.size,a.kind,a.data FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2) ORDER BY ma.message_id,ma.position",
    ).bind(owner).bind(messages).fetch_all(&mut **tx).await?;
    let mut result: HashMap<Uuid, Vec<Attachment>> = HashMap::new();
    for (message, id, name, mime_type, size, kind, data) in rows {
        result.entry(message).or_default().push(Attachment {
            meta: AttachmentMeta {
                id,
                name,
                mime_type,
                size,
                kind,
            },
            data,
        });
    }
    Ok(result)
}

/// Metadata never includes file bytes or data URLs and is safe for chat detail.
pub async fn metadata_for_messages(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    messages: &[Uuid],
) -> Result<HashMap<Uuid, Vec<AttachmentMeta>>, ApiError> {
    if messages.len() > crate::store::MAX_HISTORY {
        return Err(ApiError::bad("Too many messages"));
    }
    let rows: Vec<(Uuid,Uuid,String,String,i32,String)> = sqlx::query_as(
        "SELECT ma.message_id,a.id,a.name,a.mime_type,a.size,a.kind FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2) ORDER BY ma.message_id,ma.position",
    ).bind(owner).bind(messages).fetch_all(&mut **tx).await?;
    let mut result: HashMap<Uuid, Vec<AttachmentMeta>> = HashMap::new();
    for (message, id, name, mime_type, size, kind) in rows {
        result.entry(message).or_default().push(AttachmentMeta {
            id,
            name,
            mime_type,
            size,
            kind,
        });
    }
    Ok(result)
}

/// Fast owner-scoped capability preflight before model resolution.
pub async fn has_images(
    pool: &PgPool,
    owner: Uuid,
    conversation: Uuid,
    pending: &[Uuid],
) -> Result<bool, ApiError> {
    validate_ids(pending)?;
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attachments a WHERE a.owner_id=$1 AND a.kind='image' AND (a.id=ANY($2) OR EXISTS(SELECT 1 FROM message_attachments ma JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE ma.attachment_id=a.id AND c.owner_id=$1 AND c.id=$3)))")
        .bind(owner).bind(pending).bind(conversation).fetch_one(pool).await?)
}

fn validate_upload(body: Upload) -> Result<Attachment, ApiError> {
    let name = body.name.trim();
    if name.is_empty()
        || name.chars().count() > 200
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(ApiError::bad(
            "Use a filename of 1 to 200 characters without path separators",
        ));
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if [
        "svg", "svgz", "pdf", "zip", "gz", "bz2", "xz", "7z", "rar", "tar",
    ]
    .contains(&extension.as_str())
    {
        return Err(ApiError::bad(
            "Upload a text/code file, PNG, JPEG, or WebP image",
        ));
    }
    let requested = body
        .mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let guessed = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "json" => "application/json",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        _ => "text/plain",
    };
    let mime = if requested.is_empty() || requested == "application/octet-stream" {
        guessed
    } else {
        &requested
    };
    let image = ["image/png", "image/jpeg", "image/webp"].contains(&mime);
    let text = mime.starts_with("text/")
        || [
            "application/json",
            "application/ld+json",
            "application/javascript",
            "application/typescript",
            "application/x-yaml",
            "application/yaml",
            "application/toml",
            "application/xml",
            "application/x-sh",
            "application/sql",
        ]
        .contains(&mime);
    if !image && !text {
        return Err(ApiError::bad(
            "Upload a text/code file, PNG, JPEG, or WebP image",
        ));
    }
    let max = if image {
        MAX_IMAGE_BYTES
    } else {
        MAX_TEXT_BYTES
    };
    if body.data_base64.len() > max.div_ceil(3) * 4 {
        return Err(ApiError::bad(if image {
            "Images must be at most 5 MiB"
        } else {
            "Text files must be at most 256 KiB"
        }));
    }
    let data = STANDARD
        .decode(&body.data_base64)
        .map_err(|_| ApiError::bad("File data must use standard base64 encoding"))?;
    if data.is_empty() || data.len() > max {
        return Err(ApiError::bad("The file is empty or exceeds its size limit"));
    }
    if image {
        if !valid_image_header(mime, &data) {
            return Err(ApiError::bad(
                "Image contents do not match a valid PNG, JPEG, or WebP header",
            ));
        }
    } else {
        let content = std::str::from_utf8(&data)
            .map_err(|_| ApiError::bad("Text and code files must use UTF-8 encoding"))?;
        if content
            .chars()
            .any(|c| c.is_control() && !['\n', '\r', '\t', '\u{c}'].contains(&c))
        {
            return Err(ApiError::bad(
                "This file contains binary data; upload UTF-8 text or code",
            ));
        }
    }
    // Code/HTML is served as a download with text/plain to avoid active content.
    let stored_mime = if image {
        mime
    } else {
        "text/plain; charset=utf-8"
    };
    Ok(Attachment {
        meta: AttachmentMeta {
            id: Uuid::new_v4(),
            name: name.to_string(),
            mime_type: stored_mime.into(),
            size: data.len() as i32,
            kind: if image { "image" } else { "text" }.into(),
        },
        data,
    })
}

fn valid_image_header(mime: &str, data: &[u8]) -> bool {
    match mime {
        "image/png" => {
            data.len() >= 45
                && data.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
                && data[16..20] != [0, 0, 0, 0]
                && data[20..24] != [0, 0, 0, 0]
                && data[data.len() - 12..data.len() - 4] == *b"\0\0\0\0IEND"
        }
        "image/jpeg" => {
            data.len() >= 16 && data.starts_with(b"\xff\xd8\xff") && data.ends_with(b"\xff\xd9")
        }
        "image/webp" => {
            data.len() >= 22
                && data.starts_with(b"RIFF")
                && &data[8..12] == b"WEBP"
                && [b"VP8 ", b"VP8L", b"VP8X"].contains(&data[12..16].try_into().unwrap())
                && u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize + 8 == data.len()
        }
        _ => false,
    }
}
