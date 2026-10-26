//! Images a model generates in a reply. The gateway's `generate_image` tool appends
//! them to the answer as markdown with data URLs, megabytes each. Chat stores the bytes
//! and rewrites each one to a short link, `![alt](/api/images/<id>)`, so the saved
//! reply, later turns and the page carry a link rather than the picture.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue},
    response::Response,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{error::ApiError, files::image_key, store::Store, AppState};

pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// Generated images kept per person; past it, new ones are not saved.
pub const MAX_USER_BYTES: i64 = 512 * 1024 * 1024;
const MARKER: &str = "](data:image/";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/images", get(list))
        .route("/api/images/{id}", get(download).delete(remove))
        .with_state(state)
}

/// Whether a piece of the reply carries an inline image to store.
pub fn has_inline(text: &str) -> bool {
    text.contains(MARKER)
}

/// Replaces every `![alt](data:image/...;base64,...)` in `text` with a link to the
/// stored image. An image that cannot be kept becomes a short note instead.
pub async fn store_inline(store: &Store, owner: Uuid, conversation: Uuid, text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(4096));
    let mut rest = text;
    while let Some((before, alt, url, after)) = next_image(rest) {
        out.push_str(before);
        let alt = clean_alt(alt);
        match save(store, owner, conversation, url).await {
            Ok(id) => out.push_str(&format!("![{alt}](/api/images/{id})")),
            Err(reason) => {
                tracing::warn!("generated image not saved: {reason}");
                out.push_str(&format!("*(An image could not be saved: {reason}.)*"));
            }
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The next inline image: text before it, its alt text, its data URL, text after it.
fn next_image(text: &str) -> Option<(&str, &str, &str, &str)> {
    let mut from = 0;
    loop {
        let marker = from + text[from..].find(MARKER)?;
        let open = text[..marker].rfind("![");
        let close = text[marker..].find(')').map(|i| marker + i);
        match (open, close) {
            (Some(open), Some(close)) if !text[open + 2..marker].contains(']') => {
                let url = &text[marker + 2..close];
                return Some((
                    &text[..open],
                    &text[open + 2..marker],
                    url,
                    &text[close + 1..],
                ));
            }
            _ => from = marker + MARKER.len(),
        }
    }
}

fn clean_alt(alt: &str) -> String {
    let alt: String = alt
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '[' | ']'))
        .take(200)
        .collect();
    if alt.trim().is_empty() {
        "Generated image".into()
    } else {
        alt.trim().to_owned()
    }
}

/// Decodes a data URL and checks the bytes are the image type it claims.
fn decode(url: &str) -> Result<(&'static str, Vec<u8>), &'static str> {
    let (head, data) = url
        .strip_prefix("data:")
        .and_then(|u| u.split_once(','))
        .ok_or("not a data URL")?;
    let (mime, encoding) = head.split_once(';').ok_or("not base64")?;
    if encoding != "base64" {
        return Err("not base64");
    }
    let compact: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() > MAX_IMAGE_BYTES / 3 * 4 + 8 {
        return Err("it is larger than 8 MB");
    }
    let bytes = STANDARD
        .decode(compact)
        .map_err(|_| "it is not valid base64")?;
    let sniffed = sniff(&bytes).ok_or("it is not a PNG, JPEG, WebP or GIF image")?;
    // The data URL's own type is a hint; the bytes decide (backends label JPEGs as PNG).
    let _ = mime;
    Ok((sniffed, bytes))
}

fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

async fn save(
    store: &Store,
    owner: Uuid,
    conversation: Uuid,
    url: &str,
) -> Result<Uuid, &'static str> {
    let (mime, bytes) = decode(url)?;
    let id = Uuid::new_v4();
    let size = bytes.len() as i32;
    let placed = store
        .files
        .place(image_key(owner, id), bytes, mime)
        .await
        .map_err(|_| "storage is unavailable")?;
    // The insert checks the person's total in the same statement.
    let inserted = sqlx::query(
        "INSERT INTO generated_images(id, owner_id, conversation_id, mime_type, size, data, object_key)
         SELECT $1, $2, $3, $4, $5, $6, $7
         WHERE (SELECT COALESCE(SUM(size), 0) FROM generated_images WHERE owner_id = $2) + $5 <= $8",
    )
    .bind(id)
    .bind(owner)
    .bind(conversation)
    .bind(mime)
    .bind(size)
    .bind(&placed.data)
    .bind(&placed.key)
    .bind(MAX_USER_BYTES)
    .execute(&store.pool)
    .await;
    let inserted = match inserted {
        Ok(done) => done.rows_affected() > 0,
        Err(e) => {
            tracing::warn!("storing a generated image failed: {e}");
            store.files.discard(&placed).await;
            return Err("storage is unavailable");
        }
    };
    if !inserted {
        store.files.discard(&placed).await;
        return Err("your 512 MB of generated images is full; delete older chats to make room");
    }
    Ok(id)
}

/// The person's generated images, newest first, with the chat each came from.
async fn list(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let rows: Vec<(Uuid, Uuid, String, i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT g.id, g.conversation_id, c.title, g.size, g.created_at FROM generated_images g
         JOIN conversations c ON c.id = g.conversation_id WHERE g.owner_id=$1 ORDER BY g.created_at DESC LIMIT 200",
    )
    .bind(user.id)
    .fetch_all(&s.store.pool)
    .await?;
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size),0)::bigint FROM generated_images WHERE owner_id=$1",
    )
    .bind(user.id)
    .fetch_one(&s.store.pool)
    .await?;
    Ok(Json(json!({
        "images": rows.into_iter().map(|(id, conversation, title, size, at)| json!({"id":id,"url":format!("/api/images/{id}"),"conversationId":conversation,"conversationTitle":title,"size":size,"createdAt":at})).collect::<Vec<_>>(),
        "usage": {"used": used, "limit": MAX_USER_BYTES},
    })))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let done = sqlx::query("DELETE FROM generated_images WHERE id=$1 AND owner_id=$2")
        .bind(id)
        .bind(user.id)
        .execute(&s.store.pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn download(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let row: Option<(String, Option<Vec<u8>>, Option<String>)> = sqlx::query_as(
        "SELECT mime_type, data, object_key FROM generated_images WHERE id=$1 AND owner_id=$2",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.store.pool)
    .await?;
    let (mime, data, key) = row.ok_or_else(ApiError::not_found)?;
    let data = s.store.files.read(data, key, MAX_IMAGE_BYTES).await?;
    image_response(&mime, data)
}

/// A stored image, sandboxed and cached privately.
pub(crate) fn image_response(mime: &str, data: Vec<u8>) -> Result<Response, ApiError> {
    let (mime, inline) = crate::files::served_type(mime);
    let mut response = Response::new(Body::from(data));
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    // Stored images are checked when saved; anything else would only download.
    if !inline {
        h.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    // An image never changes once stored.
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        "content-security-policy",
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn finds_inline_images_and_leaves_other_text() {
        let url = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
        let text = format!("Before [link](https://x) ![A moth](  {url}) after");
        let text = text.replace("(  ", "(");
        let (before, alt, found, after) = next_image(&text).unwrap();
        assert_eq!(before, "Before [link](https://x) ");
        assert_eq!(alt, "A moth");
        assert_eq!(found, url);
        assert_eq!(after, " after");
        assert!(next_image("no images ](data:image/png;base64,AAAA").is_none());
    }

    #[test]
    fn decodes_by_content_not_label() {
        let url = format!(
            "data:image/png;base64,{}",
            STANDARD.encode([0xff, 0xd8, 0xff, 0xe0])
        );
        assert_eq!(decode(&url).unwrap().0, "image/jpeg");
        let url = format!(
            "data:image/png;base64,{}",
            STANDARD.encode(b"<svg onload=x>")
        );
        assert!(decode(&url).is_err());
        assert!(decode("data:image/png,raw").is_err());
        assert_eq!(clean_alt(" [x]\n"), "x");
        assert_eq!(clean_alt(""), "Generated image");
    }
}
