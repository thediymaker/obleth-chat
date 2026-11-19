//! Owner-scoped, bounded file uploads and durable references for model context.
//! Files are never fetched from remote URLs and are never served as active content.
//! Documents (PDF, Word, PowerPoint, Excel) have their text read once, on upload, by
//! Knowledge's extractor; the model reads that text, never the file itself. Any other
//! file (an archive, data, a binary, text too long to read whole) is kept as kind
//! `file`: the model is told what it is and, from a chat's workspace, can open it.
use std::collections::{HashMap, HashSet};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::paging::{Page, PageQuery};
use crate::{
    error::ApiError,
    files::{attachment_key, Files, Placed},
    knowledge::extract::{self, Extracted, Tools},
    AppState,
};

mod about;
pub mod workspace;

/// Name, type and stored bytes of a file to download.
pub(crate) type DownloadRow = (String, String, Option<Vec<u8>>, Option<String>);
type AttachmentRow = (
    Uuid,
    String,
    String,
    i32,
    String,
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
    Option<i32>,
    bool,
);
type MessageAttachmentRow = (
    Uuid,
    Uuid,
    String,
    String,
    i32,
    String,
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
    Option<i32>,
    bool,
);
type MessageMetaRow = (Uuid, Uuid, String, String, i32, String, Option<i32>, bool);
/// Columns of a file for model context. A document's or other file's bytes are left
/// unread (empty) unless `$3` asks for them: the model reads its text or note.
const CONTEXT_COLUMNS: &str = "a.id,a.name,a.mime_type,a.size,a.kind,CASE WHEN a.kind IN ('document','file') AND NOT $3 THEN ''::BYTEA ELSE a.data END,CASE WHEN a.kind IN ('document','file') AND NOT $3 THEN NULL ELSE a.object_key END,a.extracted_text,a.parts,a.searched";

/// Most a text file may be to be read whole; a longer one is kept as a file.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// Most an image may be to be viewed; a larger one is kept as a file.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_PER_MESSAGE: usize = 4;
const MIB: usize = 1024 * 1024;
/// Most text of one document a model reads whole: three quarters of what a whole
/// conversation holds (store::MAX_CONTEXT_BYTES), so the conversation still fits. A
/// longer document is searched instead when Knowledge is set up, and refused when not.
pub const MAX_DOCUMENT_TEXT_BYTES: usize = crate::store::MAX_CONTEXT_BYTES * 3 / 4;
/// Largest file any configuration allows, for reading stored files back.
pub const MAX_FILE_BYTES: usize = 2000 * MIB;
/// Encoded images, including replayed images, must fit in this inference budget.
pub const MAX_IMAGE_CONTEXT_BYTES: usize = 20 * 1024 * 1024;

/// How large files may be and how much one person may keep, from the server's
/// configuration (OBLETH_CHAT_MAX_FILE_MB, OBLETH_CHAT_FILE_QUOTA_MB and
/// OBLETH_CHAT_MAX_FILES). Images and text read whole keep their own limits.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The largest file or document.
    pub max_file_bytes: usize,
    /// One person's files in all.
    pub quota_bytes: i64,
    pub max_files: i64,
}

impl Limits {
    /// Without object storage, files are kept in Postgres, which holds at most 1 GiB
    /// in a value, and the default quota is smaller.
    pub fn new(max_file_mb: u32, quota_mb: Option<u32>, max_files: u32, in_bucket: bool) -> Self {
        let largest = if in_bucket { 2000 } else { 1000 };
        let quota = quota_mb.unwrap_or(if in_bucket { 1024 } else { 200 });
        Self {
            max_file_bytes: max_file_mb.min(largest) as usize * MIB,
            quota_bytes: i64::from(quota) * MIB as i64,
            max_files: i64::from(max_files),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(50, None, 500, false)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentMeta {
    pub id: Uuid,
    pub name: String,
    pub mime_type: String,
    pub size: i32,
    pub kind: String,
    /// A document's pages, slides or sheets, or the entries of an archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<i32>,
    /// A document too long to read whole, which the chat searches instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub searched: bool,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub meta: AttachmentMeta,
    data: Vec<u8>,
    /// A document's text with its page, slide or sheet markers, or what the model is
    /// told about a file.
    text: Option<String>,
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
        self.text_context_in(None)
    }

    /// As `text_context`, saying where a file is when the chat has a workspace:
    /// `uploads` is its path there.
    pub fn text_context_in(&self, uploads: Option<&str>) -> Option<String> {
        let name = json!(self.meta.name);
        let parts = self
            .parts_label()
            .map_or(String::new(), |p| format!(" ({p})"));
        match self.meta.kind.as_str() {
            "text" => Some(format!(
                "\n\nAttached file {name}:\n{}\nEnd of attached file.\n",
                String::from_utf8_lossy(&self.data)
            )),
            "document" => Some(match &self.text {
                Some(text) => {
                    format!("\n\nAttached file {name}{parts}:\n{text}\nEnd of attached file.\n")
                }
                None => format!("\n\nAttached file {name}{parts} is too long to read whole. Chat searches it for passages that answer the message, given with your instructions; search it again with search_knowledge if you have that tool.\n"),
            }),
            "file" => {
                let place = match uploads {
                    Some(path) => format!(
                        "It is in this conversation's workspace at {} in the project folder.",
                        json!(path)
                    ),
                    None => "This conversation has no workspace, so its contents are not available. When the workspace is on, Chat copies the file into uploads/ in the project folder; if you need its contents and can ask the person to turn the workspace on, ask.".into(),
                };
                Some(format!(
                    "\n\nAttached file {name} ({}), not read whole by Chat. {}\n{place}\nEnd of attached file.\n",
                    size_label(self.meta.size as usize),
                    self.text.as_deref().unwrap_or("A file.")
                ))
            }
            _ => None,
        }
    }

    /// A document's searched copy: it is read from its collection, not sent whole.
    pub fn searched(&self) -> bool {
        self.meta.kind == "document" && self.meta.searched
    }

    /// "12 pages", "1 slide", "3 sheets".
    fn parts_label(&self) -> Option<String> {
        let n = self.meta.parts?;
        let unit = match extract::kind(&self.meta.name, &self.meta.mime_type)? {
            extract::Kind::Pdf => "page",
            extract::Kind::Pptx => "slide",
            extract::Kind::Xlsx => "sheet",
            _ => return None,
        };
        Some(format!("{n} {unit}{}", if n == 1 { "" } else { "s" }))
    }

    pub fn content_part(&self) -> Value {
        self.content_part_in(None)
    }

    pub fn content_part_in(&self, uploads: Option<&str>) -> Value {
        if let Some(text) = self.text_context_in(uploads) {
            json!({"type":"text","text":text})
        } else {
            json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",self.meta.mime_type,STANDARD.encode(&self.data))}})
        }
    }

    /// UTF-8 context bytes for text, serialized image-part bytes for images:
    /// what `text_context` and `content_part` would measure, computed from the
    /// metadata alone, so the check neither needs the bytes nor encodes them.
    pub fn context_bytes(&self) -> usize {
        match self.meta.kind.as_str() {
            "text" => TEXT_WRAP + json!(self.meta.name).to_string().len() + self.meta.size as usize,
            "image" => {
                IMAGE_PART_WRAP
                    + self.meta.mime_type.len()
                    + (self.meta.size as usize).div_ceil(3) * 4
            }
            _ => self.text_context().map_or(0, |text| text.len()),
        }
    }
}

/// What `text_context` adds around a text file's bytes and its JSON name.
const TEXT_WRAP: usize =
    "\n\nAttached file ".len() + ":\n".len() + "\nEnd of attached file.\n".len();
/// What `content_part` adds around an image's media type and base64 bytes.
const IMAGE_PART_WRAP: usize = r#"{"type":"image_url","image_url":{"url":"data:;base64,"}}"#.len();

/// An attachment as read under the owner's lock: its metadata and extracted
/// text, with its bytes still where they are stored. [`read`] fetches them
/// once the transaction has committed, so a slow bucket never holds the user
/// row (user-row locks serialize only short database transactions).
#[derive(Debug)]
pub struct Unread {
    file: Attachment,
    stored: (Option<Vec<u8>>, Option<String>),
}

impl Unread {
    pub fn meta(&self) -> &AttachmentMeta {
        &self.file.meta
    }
    pub fn context_bytes(&self) -> usize {
        self.file.context_bytes()
    }
}

/// The attachments with their bytes, in the order given.
pub async fn read(files: &Files, unread: Vec<Unread>) -> Result<Vec<Attachment>, ApiError> {
    let (mut loaded, stored): (Vec<_>, Vec<_>) =
        unread.into_iter().map(|u| (u.file, u.stored)).unzip();
    let data = files.read_all(stored, MAX_FILE_BYTES).await?;
    for (file, data) in loaded.iter_mut().zip(data) {
        file.data = data;
    }
    Ok(loaded)
}

/// A turn's new files and the replayed messages' files, fetched in one batch.
pub async fn read_turn(
    files: &Files,
    new: Vec<Unread>,
    replayed: HashMap<Uuid, Vec<Unread>>,
) -> Result<(Vec<Attachment>, HashMap<Uuid, Vec<Attachment>>), ApiError> {
    let mut replayed = replayed;
    let marker = Uuid::nil();
    replayed.insert(marker, new);
    let mut read = read_messages(files, replayed).await?;
    let new = read.remove(&marker).unwrap_or_default();
    Ok((new, read))
}

/// As [`read`], for the attachments of several messages, fetched in one batch.
pub async fn read_messages(
    files: &Files,
    unread: HashMap<Uuid, Vec<Unread>>,
) -> Result<HashMap<Uuid, Vec<Attachment>>, ApiError> {
    let (messages, per_message): (Vec<Uuid>, Vec<Vec<Unread>>) = unread.into_iter().unzip();
    let counts: Vec<usize> = per_message.iter().map(Vec::len).collect();
    let mut loaded = read(files, per_message.into_iter().flatten().collect())
        .await?
        .into_iter();
    Ok(messages
        .into_iter()
        .zip(counts)
        .map(|(message, n)| (message, loaded.by_ref().take(n).collect()))
        .collect())
}

/// The JSON form of an upload, with the file as base64.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Upload {
    name: String,
    #[serde(default)]
    mime_type: String,
    data_base64: String,
}

/// An upload's name, the type its sender gave, and its bytes.
struct Incoming {
    name: String,
    mime_type: String,
    data: Vec<u8>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/attachments",
            // The upload reads its own body, after sign-in.
            get(list).post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/attachments/limits", get(limits))
        .route("/api/attachments/{id}", get(download).delete(delete))
        .with_state(state)
}

/// Files per page of the list, unless the request asks for a limit.
const LISTED: i64 = 100;

/// A page of the person's files, newest first. `limits` rides along for interfaces
/// that still read it here; `GET /api/attachments/limits` serves it alone.
async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let (items, more) = list_for(&s.store.pool, user.id, page.page(LISTED)).await?;
    Ok(Json(
        json!({"items":items,"more":more,"limits":limits_json(&s.store.limits)}),
    ))
}

async fn list_for(
    pool: &PgPool,
    owner: Uuid,
    page: Page,
) -> Result<(Vec<AttachmentMeta>, bool), sqlx::Error> {
    let rows = sqlx::query_as::<_, AttachmentMeta>(
        "SELECT id,name,mime_type,size,kind,parts,searched FROM attachments WHERE owner_id=$1
         ORDER BY created_at DESC,id LIMIT $2 OFFSET $3",
    )
    .bind(owner)
    .bind(page.fetch())
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok(page.cut(rows))
}

/// The sizes and counts an upload must fit, for the composer to check a file
/// before sending it.
async fn limits(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    s.auth.require(&headers, false).await?;
    Ok(Json(limits_json(&s.store.limits)))
}

fn limits_json(limits: &Limits) -> Value {
    json!({"maxFiles":limits.max_files,"maxBytes":limits.quota_bytes,"maxFileBytes":limits.max_file_bytes,"maxTextBytes":MAX_TEXT_BYTES,"maxImageBytes":MAX_IMAGE_BYTES,"maxDocumentBytes":limits.max_file_bytes,"maxDocumentTextBytes":MAX_DOCUMENT_TEXT_BYTES,"maxPerMessage":MAX_PER_MESSAGE})
}

/// File bytes held in memory at once on each server, in MiB: upload bodies while
/// they are checked and stored, and files on their way into a workspace. One larger
/// than the whole budget waits for all of it, so it is held alone.
const MEMORY_BUDGET_MIB: usize = 256;
static MEMORY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MEMORY_BUDGET_MIB);

/// Waits until `bytes` fit in the memory budget, and holds them there.
pub(crate) async fn hold(bytes: usize) -> Result<tokio::sync::SemaphorePermit<'static>, ApiError> {
    let mib = bytes.div_ceil(MIB).clamp(1, MEMORY_BUDGET_MIB) as u32;
    MEMORY
        .acquire_many(mib)
        .await
        .map_err(|_| ApiError::internal())
}

/// Two forms: the file's own bytes as `application/octet-stream`, named by the
/// `name` (and optionally `type`) query parameters, or JSON with the file as base64.
/// The body's size must be declared, and is checked before anything is read.
async fn upload(
    State(s): State<AppState>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Body,
) -> Result<(StatusCode, Json<AttachmentMeta>), ApiError> {
    let limits = s.store.limits;
    let raw = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/octet-stream"));
    // Base64 takes 4 bytes for every 3, and the JSON around it a little more.
    let most = if raw {
        limits.max_file_bytes
    } else {
        limits.max_file_bytes.div_ceil(3) * 4 + MIB
    };
    let too_large = format!(
        "Files can be at most {}. Choose a smaller file.",
        size_label(limits.max_file_bytes)
    );
    // Everything that can refuse the upload is decided before any of it is read:
    // who sends it, and that its declared size fits.
    let size_hint = axum::body::HttpBody::size_hint(&body).exact();
    let checks = async {
        let user = s.auth.require(&headers, true).await?;
        let declared = headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .or(size_hint)
            .ok_or_else(|| {
                ApiError(
                    StatusCode::LENGTH_REQUIRED,
                    "Send the upload with its size (Content-Length)".into(),
                )
            })?;
        if declared > most as u64 {
            return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, too_large.clone()));
        }
        Ok::<_, ApiError>((user, declared as usize))
    };
    let ((user, length), body) = crate::error::admitted(body, checks.await).await?;
    // Room in memory for the body, its decoded bytes (JSON only) and its copy on the
    // way to storage, before the body is read.
    let _held = hold(if raw { length } else { length * 2 }).await?;
    let body = crate::error::upload(body, length, too_large).await?;
    // The JSON decode and the hash walk the whole file: off the async workers.
    let (incoming, digest) = tokio::task::spawn_blocking(move || {
        let incoming = if raw {
            let mut name = None;
            let mut mime_type = String::new();
            for (key, value) in
                url::form_urlencoded::parse(query.as_deref().unwrap_or("").as_bytes())
            {
                match key.as_ref() {
                    "name" => name = Some(value.into_owned()),
                    "type" => mime_type = value.into_owned(),
                    _ => {}
                }
            }
            Incoming {
                name: name.ok_or_else(|| ApiError::bad("Name the file with ?name="))?,
                mime_type,
                data: body,
            }
        } else {
            let upload: Upload =
                serde_json::from_slice(&body).map_err(|_| ApiError::bad("Invalid upload"))?;
            drop(body);
            decoded(upload, &limits)?
        };
        let digest = Sha256::digest(&incoming.data).to_vec();
        Ok::<_, ApiError>((incoming, digest))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    // A long document can be searched when Knowledge is set up and has a gateway to
    // embed it through.
    let searchable = s.knowledge.is_some() && s.gateway.is_some();
    let tools = s
        .knowledge
        .as_ref()
        .map_or_else(Tools::from_env, |k| k.tools().clone());
    let mut file = validate_upload(incoming, &tools, searchable, &limits).await?;
    let files = &s.store.files;
    let placed = place(
        files,
        user.id,
        &file.meta,
        std::mem::take(&mut file.data),
        &digest,
    )
    .await?;
    let saved = async {
        let mut tx = s.store.pool.begin().await?;
        lock_owner(&mut tx, user.id).await?;
        insert_in(&mut tx, &limits, user.id, &file, &placed, &digest).await?;
        if file.searched() {
            crate::knowledge::attached::stage_in(&mut tx, user.id, &file.meta).await?;
        }
        tx.commit().await?;
        Ok::<_, ApiError>(())
    }
    .await;
    if let Err(error) = saved {
        files.discard(&placed).await;
        return Err(error);
    }
    Ok((StatusCode::CREATED, Json(file.meta)))
}

/// The JSON form's bytes, refused before decoding when they would be too large.
fn decoded(upload: Upload, limits: &Limits) -> Result<Incoming, ApiError> {
    if upload.data_base64.len() > limits.max_file_bytes.div_ceil(3) * 4 {
        return Err(ApiError::bad(format!(
            "Files can be at most {}. Choose a smaller file.",
            size_label(limits.max_file_bytes)
        )));
    }
    let data = STANDARD
        .decode(&upload.data_base64)
        .map_err(|_| ApiError::bad("File data must use standard base64 encoding"))?;
    Ok(Incoming {
        name: upload.name,
        mime_type: upload.mime_type,
        data,
    })
}

/// Stores the file's bytes before its row is written.
async fn place(
    files: &Files,
    owner: Uuid,
    meta: &AttachmentMeta,
    data: Vec<u8>,
    sha256: &[u8],
) -> Result<Placed, ApiError> {
    files
        .place_hashed(
            attachment_key(owner, meta.id),
            data,
            &meta.mime_type,
            sha256,
        )
        .await
}

async fn insert_in(
    tx: &mut Transaction<'_, Postgres>,
    limits: &Limits,
    owner: Uuid,
    file: &Attachment,
    placed: &Placed,
    digest: &[u8],
) -> Result<(), ApiError> {
    let (count, bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),COALESCE(SUM(size),0)::BIGINT FROM attachments WHERE owner_id=$1",
    )
    .bind(owner)
    .fetch_one(&mut **tx)
    .await?;
    let (max_files, quota) = (limits.max_files, limits.quota_bytes);
    if count >= max_files || bytes + i64::from(file.meta.size) > quota {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!(
                "Your file storage is full: you can keep {max_files} files and {} in all. Delete files you no longer need before uploading more.",
                size_label(quota as usize)
            ),
        ));
    }
    sqlx::query("INSERT INTO attachments(id,owner_id,name,mime_type,kind,size,data,object_key,sha256,extracted_text,parts,searched)VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(file.meta.id).bind(owner).bind(&file.meta.name).bind(&file.meta.mime_type)
        .bind(&file.meta.kind).bind(file.meta.size).bind(&placed.data).bind(&placed.key)
        .bind(digest).bind(&file.text).bind(file.meta.parts)
        .bind(file.meta.searched).execute(&mut **tx).await?;
    Ok(())
}

/// Portable archives carry file bytes explicitly. Reuse identical private files
/// across imported messages so replayed attachments do not multiply disk usage.
/// Nothing is committed independently of the caller's conversation transaction.
pub async fn import_in(
    tx: &mut Transaction<'_, Postgres>,
    store: &Files,
    limits: &Limits,
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
    let tools = Tools::from_env();
    for item in files {
        // A long document is kept to be searched; a chat on a server without Knowledge
        // tells the model it can't be read whole.
        let incoming = decoded(
            Upload {
                name: item.name.clone(),
                mime_type: item.mime_type.clone(),
                data_base64: item.data_base64.clone(),
            },
            limits,
        )?;
        let mut file = validate_upload(incoming, &tools, true, limits).await?;
        let digest = Sha256::digest(&file.data).to_vec();
        let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM attachments WHERE owner_id=$1 AND name=$2 AND mime_type=$3 AND size=$4 AND sha256=$5 AND NOT(id=ANY($6)) ORDER BY created_at,id LIMIT 1")
            .bind(owner).bind(&file.meta.name).bind(&file.meta.mime_type).bind(file.meta.size).bind(&digest).bind(&ids).fetch_optional(&mut **tx).await?;
        if let Some(id) = existing {
            ids.push(id);
        } else {
            let placed = place(
                store,
                owner,
                &file.meta,
                std::mem::take(&mut file.data),
                &digest,
            )
            .await?;
            if let Err(error) = insert_in(tx, limits, owner, &file, &placed, &digest).await {
                store.discard(&placed).await;
                return Err(error);
            }
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
    let row: Option<DownloadRow> = sqlx::query_as(&format!(
        "SELECT name,{SERVED_TYPE},data,object_key FROM attachments WHERE id=$1 AND owner_id=$2"
    ))
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.store.pool)
    .await?;
    let (name, mime, data, key) = row.ok_or_else(ApiError::not_found)?;
    let body = s.store.files.body(data, key, MAX_FILE_BYTES).await?;
    file_response(&name, &mime, body)
}

/// The type a stored file is served with. A `file` is always opaque bytes, whatever
/// its name or contents, so an uploaded page or SVG never runs in the app's origin.
pub(crate) const SERVED_TYPE: &str =
    "CASE WHEN kind='file' THEN 'application/octet-stream' ELSE mime_type END";

/// An attachment as a download, never rendered in the app's origin.
pub(crate) fn file_response(
    name: &str,
    mime: &str,
    data: impl Into<Body>,
) -> Result<Response, ApiError> {
    let encoded_name: String = url::form_urlencoded::byte_serialize(name.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    let mut response = Response::new(data.into());
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(crate::files::served_type(mime).0),
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

fn require_vision<'a>(
    files: impl IntoIterator<Item = &'a AttachmentMeta>,
    vision: bool,
) -> Result<(), ApiError> {
    if !vision && files.into_iter().any(|meta| meta.kind == "image") {
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
) -> Result<Vec<Unread>, ApiError> {
    validate_ids(ids)?;
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows: Vec<AttachmentRow> = sqlx::query_as(&format!(
        "SELECT {CONTEXT_COLUMNS} FROM attachments a WHERE a.owner_id=$1 AND a.id=ANY($2)"
    ))
    .bind(owner)
    .bind(ids)
    .bind(false)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != ids.len() {
        return Err(ApiError::not_found());
    }
    let (rows, stored): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .map(
            |(id, name, mime_type, size, kind, data, key, text, parts, searched)| {
                (
                    (id, name, mime_type, size, kind, text, parts, searched),
                    (data, key),
                )
            },
        )
        .unzip();
    let mut by_id: HashMap<_, _> = rows
        .into_iter()
        .zip(stored)
        .map(
            |((id, name, mime_type, size, kind, text, parts, searched), stored)| {
                (
                    id,
                    Unread {
                        file: Attachment {
                            meta: AttachmentMeta {
                                id,
                                name,
                                mime_type,
                                size,
                                kind,
                                parts,
                                searched,
                            },
                            data: Vec::new(),
                            text,
                        },
                        stored,
                    },
                )
            },
        )
        .collect();
    let files: Vec<_> = ids
        .iter()
        .map(|id| by_id.remove(id).expect("all IDs checked above"))
        .collect();
    require_vision(files.iter().map(Unread::meta), vision)?;
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
) -> Result<HashMap<Uuid, Vec<Unread>>, ApiError> {
    if messages.len() > crate::store::MAX_HISTORY {
        return Err(ApiError::bad("Too many messages"));
    }
    // Count repeated references before loading bytea: one 5 MiB upload replayed
    // hundreds of times must not create a multi-gigabyte database result.
    let (text_bytes,image_bytes): (i64,i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(CASE WHEN a.kind='text' THEN a.size WHEN a.kind IN ('document','file') THEN COALESCE(OCTET_LENGTH(a.extracted_text),0) ELSE 0 END),0)::BIGINT,COALESCE(SUM(CASE WHEN a.kind='image' THEN ((a.size::BIGINT+2)/3)*4+128 ELSE 0 END),0)::BIGINT FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2)",
    ).bind(owner).bind(messages).fetch_one(&mut **tx).await?;
    if image_bytes > 0 && !vision {
        return Err(ApiError::bad("This conversation contains images. Choose a model with confirmed image support to continue."));
    }
    if text_bytes > crate::store::MAX_CONTEXT_BYTES as i64
        || image_bytes > MAX_IMAGE_CONTEXT_BYTES as i64
    {
        return Err(ApiError::bad("Attached files exceed the conversation context limit. Start a new conversation or branch an earlier message."));
    }
    load_message_data(tx, owner, messages, false, &[]).await
}

/// The files of exported messages, by message, and the files left out of them.
pub struct ForArchive {
    pub files: HashMap<Uuid, Vec<Unread>>,
    /// Files too large for what is left of the archive's budget.
    pub left_out: HashMap<Uuid, Vec<AttachmentMeta>>,
}

/// Export is bounded by encoded size rather than inference capabilities. Text,
/// images and documents must fit; a `file` that doesn't is left out, and the export
/// says so, so a chat with a large file still exports.
pub async fn load_for_archive(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    messages: &[Uuid],
    max_bytes: usize,
) -> Result<ForArchive, ApiError> {
    if messages.len() > crate::store::MAX_HISTORY {
        return Err(ApiError::bad("Too many messages"));
    }
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT a.id,a.kind,((a.size::BIGINT+2)/3)*4+OCTET_LENGTH(a.name)+OCTET_LENGTH(a.mime_type)+128 FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2) ORDER BY m.created_at,m.id,ma.position",
    ).bind(owner).bind(messages).fetch_all(&mut **tx).await?;
    let budget = max_bytes.min(20 * 1024 * 1024) as i64;
    let needed: i64 = rows
        .iter()
        .filter(|(_, kind, _)| kind != "file")
        .map(|(_, _, bytes)| bytes)
        .sum();
    if needed > budget {
        return Err(ApiError::bad("These attachments exceed the archive size limit. Export a smaller conversation branch."));
    }
    // Files in the order they were sent, each with every message it is in, while
    // they fit.
    let mut room = budget - needed;
    let mut cost: Vec<(Uuid, i64)> = vec![];
    for (id, _, bytes) in rows.iter().filter(|(_, kind, _)| kind == "file") {
        match cost.iter_mut().find(|(seen, _)| seen == id) {
            Some((_, total)) => *total += bytes,
            None => cost.push((*id, *bytes)),
        }
    }
    let mut skipped = vec![];
    for (id, bytes) in cost {
        if bytes <= room {
            room -= bytes;
        } else {
            skipped.push(id);
        }
    }
    let loaded = load_message_data(tx, owner, messages, true, &skipped).await?;
    let mut left_out: HashMap<Uuid, Vec<AttachmentMeta>> = HashMap::new();
    if !skipped.is_empty() {
        for (message, metas) in metadata_for_messages(tx, owner, messages).await? {
            let metas: Vec<_> = metas
                .into_iter()
                .filter(|m| skipped.contains(&m.id))
                .collect();
            if !metas.is_empty() {
                left_out.insert(message, metas);
            }
        }
    }
    Ok(ForArchive {
        files: loaded,
        left_out,
    })
}

/// `originals`: read documents' and files' own bytes too, which only an archive
/// needs. Files in `skip` are left out.
async fn load_message_data(
    tx: &mut Transaction<'_, Postgres>,
    owner: Uuid,
    messages: &[Uuid],
    originals: bool,
    skip: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Unread>>, ApiError> {
    let rows: Vec<MessageAttachmentRow> = sqlx::query_as(&format!(
        "SELECT ma.message_id,{CONTEXT_COLUMNS} FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2) AND NOT a.id=ANY($4) ORDER BY ma.message_id,ma.position",
    )).bind(owner).bind(messages).bind(originals).bind(skip).fetch_all(&mut **tx).await?;
    let (rows, stored): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .map(
            |(message, id, name, mime_type, size, kind, data, key, text, parts, searched)| {
                (
                    (
                        message, id, name, mime_type, size, kind, text, parts, searched,
                    ),
                    (data, key),
                )
            },
        )
        .unzip();
    let mut result: HashMap<Uuid, Vec<Unread>> = HashMap::new();
    for ((message, id, name, mime_type, size, kind, text, parts, searched), stored) in
        rows.into_iter().zip(stored)
    {
        result.entry(message).or_default().push(Unread {
            stored,
            file: Attachment {
                meta: AttachmentMeta {
                    id,
                    name,
                    mime_type,
                    size,
                    kind,
                    parts,
                    searched,
                },
                data: Vec::new(),
                text,
            },
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
    let rows: Vec<MessageMetaRow> = sqlx::query_as(
        "SELECT ma.message_id,a.id,a.name,a.mime_type,a.size,a.kind,a.parts,a.searched FROM message_attachments ma JOIN attachments a ON a.id=ma.attachment_id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE a.owner_id=$1 AND c.owner_id=$1 AND ma.message_id=ANY($2) ORDER BY ma.message_id,ma.position",
    ).bind(owner).bind(messages).fetch_all(&mut **tx).await?;
    let mut result: HashMap<Uuid, Vec<AttachmentMeta>> = HashMap::new();
    for (message, id, name, mime_type, size, kind, parts, searched) in rows {
        result.entry(message).or_default().push(AttachmentMeta {
            id,
            name,
            mime_type,
            size,
            kind,
            parts,
            searched,
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

/// Checks an upload and, for a document, reads its text. `searchable`: a document too
/// long to read whole may be kept to be searched instead of being refused. A file
/// that is not a document, an image to view or text to read whole is kept as a file.
async fn validate_upload(
    body: Incoming,
    tools: &Tools,
    searchable: bool,
    limits: &Limits,
) -> Result<Attachment, ApiError> {
    let name = clean_upload_name(&body.name)?;
    let name = name.as_str();
    let data = body.data;
    if data.is_empty() {
        return Err(ApiError::bad("This file is empty"));
    }
    if data.len() > limits.max_file_bytes {
        return Err(ApiError::bad(format!(
            "This file is {}, and files can be at most {}. Choose a smaller file.",
            size_label(data.len()),
            size_label(limits.max_file_bytes)
        )));
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if ["pdf", "docx", "pptx", "xlsx"].contains(&extension.as_str()) {
        return document(name, data, tools, searchable).await;
    }
    // SVG can carry scripts, and archives are never text to read: whatever type they
    // were sent with, they are files.
    if [
        "svg", "svgz", "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "tar",
    ]
    .contains(&extension.as_str())
    {
        return Ok(file(name, data));
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
    let image = ["image/png", "image/jpeg", "image/webp"].contains(&mime)
        && data.len() <= MAX_IMAGE_BYTES
        && valid_image_header(mime, &data);
    let text = !image
        && data.len() <= MAX_TEXT_BYTES
        && (mime.starts_with("text/")
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
            .contains(&mime))
        && std::str::from_utf8(&data).is_ok_and(|content| {
            !content
                .chars()
                .any(|c| c.is_control() && !['\n', '\r', '\t', '\u{c}'].contains(&c))
        });
    if !image && !text {
        return Ok(file(name, data));
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
            parts: None,
            searched: false,
        },
        data,
        text: None,
    })
}

/// Anything else: an archive, data, a binary, or text too long to read whole. It is
/// kept as opaque bytes with a note for the model, worked out now, of what it is.
fn file(name: &str, data: Vec<u8>) -> Attachment {
    let (note, entries) = about::describe(name, &data);
    Attachment {
        meta: AttachmentMeta {
            id: Uuid::new_v4(),
            name: name.to_string(),
            mime_type: "application/octet-stream".into(),
            size: data.len() as i32,
            kind: "file".into(),
            parts: entries,
            searched: false,
        },
        data,
        text: Some(note),
    }
}

/// A PDF, Word, PowerPoint or Excel file, with its text read now so every turn that
/// sends it reuses the same text.
async fn document(
    name: &str,
    data: Vec<u8>,
    tools: &Tools,
    searchable: bool,
) -> Result<Attachment, ApiError> {
    let kind =
        extract::kind(name, "").ok_or_else(|| ApiError::bad("This document could not be read"))?;
    let (signature, what) = match kind {
        extract::Kind::Pdf => (&b"%PDF-"[..], "PDF"),
        extract::Kind::Docx => (&b"PK\x03\x04"[..], "Word file"),
        extract::Kind::Pptx => (&b"PK\x03\x04"[..], "PowerPoint file"),
        _ => (&b"PK\x03\x04"[..], "Excel file"),
    };
    let unreadable = || {
        ApiError::bad(format!(
            "This {what} could not be read. Check that it opens, then upload it again."
        ))
    };
    if !data.starts_with(signature) {
        return Err(unreadable());
    }
    let pages = match extract::extract(tools, kind, &data).await {
        Ok(Extracted::Pages(pages)) => pages,
        Ok(Extracted::NeedsOcr { .. }) => {
            return Err(ApiError::bad(
                "This PDF has no text to read; it may be a scan.",
            ))
        }
        Err(error) => {
            tracing::warn!("could not read an attached document: {error:#}");
            return Err(unreadable());
        }
    };
    let text = extract::marked_text(&pages);
    if !text.chars().any(char::is_alphanumeric) {
        return Err(ApiError::bad(format!("This {what} has no text to read.")));
    }
    let searched = text.len() > MAX_DOCUMENT_TEXT_BYTES;
    if searched && !searchable {
        return Err(ApiError::bad(format!(
            "This {what} has {} of text, and Chat can read up to {} of a document in a conversation. Attach a shorter part of it.",
            size_label(text.len()),
            size_label(MAX_DOCUMENT_TEXT_BYTES)
        )));
    }
    Ok(Attachment {
        meta: AttachmentMeta {
            id: Uuid::new_v4(),
            name: name.to_string(),
            // The type follows the kind of file, never what the uploader said.
            mime_type: extract::mime_for(kind),
            size: data.len() as i32,
            kind: "document".into(),
            parts: extract::parts(kind, &pages),
            searched,
        },
        data,
        text: (!searched).then_some(text),
    })
}

/// 76800 bytes as "75 KB".
/// One plain file name for an upload, on every path that takes one: trimmed, path
/// separators rewritten to `_` (files dropped from a file manager often carry them),
/// no control characters, at most 200 characters.
pub(crate) fn clean_upload_name(name: &str) -> Result<String, ApiError> {
    let name: String = name
        .trim()
        .replace(['/', '\\'], "_")
        .chars()
        .take(200)
        .collect();
    if name.is_empty() || name == "." || name == ".." || name.chars().any(char::is_control) {
        return Err(ApiError::bad("Give the file a name of 1 to 200 characters"));
    }
    Ok(name)
}

pub(crate) fn size_label(bytes: usize) -> String {
    match bytes {
        b if b < 1024 * 1024 => format!("{} KB", b.div_ceil(1024)),
        b if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / 1024.0 / 1024.0),
        b => format!("{:.1} GB", b as f64 / 1024.0 / 1024.0 / 1024.0),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Database;

    #[test]
    fn upload_names_are_one_plain_name_on_every_path() {
        assert_eq!(clean_upload_name("  notes.txt ").unwrap(), "notes.txt");
        assert_eq!(clean_upload_name("a/b\\c.pdf").unwrap(), "a_b_c.pdf");
        assert_eq!(
            clean_upload_name(&"x".repeat(300)).unwrap().chars().count(),
            200
        );
        for bad in ["", "   ", ".", "..", "a\u{7}b"] {
            assert_eq!(
                clean_upload_name(bad).unwrap_err().0,
                StatusCode::BAD_REQUEST,
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_file_list_pages_past_the_first_hundred() {
        let Some(db) = Database::new().await else {
            return;
        };
        let owner = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, username, display_name, role, password_hash) VALUES ($1, 'ana', 'Ana', 'user', 'x')")
            .bind(owner)
            .execute(&db.pool)
            .await
            .unwrap();
        for n in 0..150 {
            sqlx::query("INSERT INTO attachments (id, owner_id, name, mime_type, kind, size, data, created_at) VALUES ($1, $2, $3, 'text/plain', 'file', 1, 'x', NOW() - make_interval(secs => $4))")
                .bind(Uuid::new_v4())
                .bind(owner)
                .bind(format!("file-{n}"))
                .bind(n as f64)
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let first = PageQuery::default().page(LISTED);
        let (items, more) = list_for(&db.pool, owner, first).await.unwrap();
        assert_eq!(
            (items.len(), more, items[0].name.as_str()),
            (100, true, "file-0")
        );
        let next = PageQuery {
            offset: Some(100),
            limit: None,
        }
        .page(LISTED);
        let (items, more) = list_for(&db.pool, owner, next).await.unwrap();
        assert_eq!(
            (items.len(), more, items[49].name.as_str()),
            (50, false, "file-149")
        );
        db.close().await;
    }

    #[test]
    fn the_limits_are_the_configured_ones() {
        let limits = Limits::new(20, Some(300), 400, true);
        let json = limits_json(&limits);
        assert_eq!(json["maxFileBytes"], 20 * MIB);
        assert_eq!(json["maxBytes"], 300 * MIB as i64);
        assert_eq!(json["maxFiles"], 400);
        assert_eq!(json["maxPerMessage"], MAX_PER_MESSAGE);
    }

    fn attachment(kind: &str, mime_type: &str, data: Vec<u8>, text: Option<&str>) -> Attachment {
        Attachment {
            meta: AttachmentMeta {
                id: Uuid::nil(),
                name: "notes \"q\".txt".into(),
                mime_type: mime_type.into(),
                size: data.len() as i32,
                kind: kind.into(),
                parts: None,
                searched: false,
            },
            data,
            text: text.map(str::to_owned),
        }
    }

    /// The arithmetic size is exactly what building the part would measure,
    /// so the context check decides the same without encoding anything.
    #[test]
    fn context_bytes_match_the_built_part_without_building_it() {
        for size in (0..=10).chain([1_000_003, 3 * 1024 * 1024]) {
            let image = attachment("image", "image/png", vec![7; size], None);
            assert_eq!(
                image.context_bytes(),
                image.content_part().to_string().len(),
                "image of {size} bytes"
            );
            let text = attachment("text", "text/plain", vec![b'a'; size], None);
            assert_eq!(
                text.context_bytes(),
                text.text_context().unwrap().len(),
                "text of {size} bytes"
            );
        }
        let document = attachment("document", "application/pdf", vec![], Some("page one"));
        assert_eq!(
            document.context_bytes(),
            document.text_context().unwrap().len()
        );
    }
}
