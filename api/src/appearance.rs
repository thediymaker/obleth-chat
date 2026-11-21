//! The site's branding as an administrator set it: its name and look (the accent
//! color, the theme people start in, whether they may pick their own accent, how round
//! corners are), the words on the home and sign-in pages, the links and icons, and the
//! logo and tab icon as uploaded images. Each setting is optional: one left unset stays
//! as the installation's branding/site.config.js has it. It reaches every page with the
//! session answer, the sign-in page included, so the site is the same before and after
//! signing in.
use crate::audit::{self, Action, Target};
use crate::{error::ApiError, AppState};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

const NAME_CHARS: usize = 60;
const THEMES: [&str; 3] = ["system", "light", "dark"];
const CORNERS: [&str; 3] = ["sharp", "soft", "round"];
/// A heading or a label, and a sentence or two.
const SHORT_CHARS: usize = 120;
const LONG_CHARS: usize = 300;
const LINKS: usize = 8;
const SLOTS: [&str; 3] = ["logo-light", "logo-dark", "favicon"];
const IMAGE_BYTES: usize = 512 * 1024;

/// A change sets every setting: a null puts that one back to the installation's own.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    site_name: Option<String>,
    accent: Option<String>,
    default_theme: Option<String>,
    personal_accent: Option<bool>,
    corners: Option<String>,
    #[serde(default)]
    branding: Branding,
}

/// The words, links and icons, in the shape of site.config.js. Only what is present
/// is set; a present but empty text hides that line, as in the file.
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Branding {
    #[serde(skip_serializing_if = "Option::is_none")]
    site_subtitle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assistant_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_alt: Option<String>,
    /// A text-only brand: no logo beside the name, whatever is installed or uploaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    hide_logo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icons: Option<Icons>,
    #[serde(skip_serializing_if = "Option::is_none")]
    login: Option<Login>,
    #[serde(skip_serializing_if = "Option::is_none")]
    links: Option<Vec<Link>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    home: Option<Home>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Icons {
    #[serde(skip_serializing_if = "Option::is_none")]
    greeting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assistant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loading: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Login {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sso_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sso_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<String>,
    /// A link with a blank label and address is no link.
    #[serde(skip_serializing_if = "Option::is_none")]
    help_link: Option<Link>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Link {
    label: String,
    url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Home {
    #[serde(skip_serializing_if = "Option::is_none")]
    eyebrow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    footer: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Row {
    site_name: Option<String>,
    accent: Option<String>,
    default_theme: Option<String>,
    personal_accent: Option<bool>,
    corners: Option<String>,
    branding: Value,
    updated_at: DateTime<Utc>,
}

impl Row {
    /// Only what is set, so the page keeps its own value for the rest.
    fn set(&self) -> Value {
        let mut out = Map::new();
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                out.insert(key.into(), value);
            }
        };
        put("siteName", self.site_name.as_deref().map(Value::from));
        put("accent", self.accent.as_deref().map(Value::from));
        put(
            "defaultTheme",
            self.default_theme.as_deref().map(Value::from),
        );
        put("personalAccent", self.personal_accent.map(Value::from));
        put("corners", self.corners.as_deref().map(Value::from));
        Value::Object(out)
    }
}

/// What administrators see and edit: every setting, null where it is left to the file.
fn document(row: &Row) -> Value {
    json!({
        "siteName": row.site_name,
        "accent": row.accent,
        "defaultTheme": row.default_theme,
        "personalAccent": row.personal_accent,
        "corners": row.corners,
        "branding": row.branding,
        "updatedAt": row.updated_at,
    })
}

/// What every page gets with the session answer: the look that is set (`{}` when the
/// file decides everything), and the branding that is set with where its images are.
pub struct Shown {
    pub appearance: Value,
    pub branding: Value,
}

pub async fn shown(pool: &PgPool) -> Result<Shown, ApiError> {
    let row = read_row(pool).await?;
    let mut branding = row.branding.clone();
    let images = images(pool).await?;
    if !images.is_empty() {
        branding["images"] = Value::Object(images);
    }
    Ok(Shown {
        appearance: row.set(),
        branding,
    })
}

/// Each uploaded image's address by its slot. The address changes with the image, so
/// browsers may keep a copy for good.
async fn images(pool: &PgPool) -> Result<Map<String, Value>, ApiError> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT slot, etag FROM branding_images ORDER BY slot")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(slot, etag)| {
            let path = format!("/api/branding/images/{slot}?v={etag}");
            (slot, Value::from(path))
        })
        .collect())
}

async fn read_row(pool: &PgPool) -> Result<Row, ApiError> {
    Ok(sqlx::query_as(
        "SELECT site_name, accent, default_theme, personal_accent, corners, branding, updated_at FROM appearance WHERE id",
    )
    .fetch_one(pool)
    .await?)
}

/// Text as it will be shown: trimmed, at most `max` characters, line breaks allowed
/// where the file allows them, nothing else that is not text.
fn copy(text: &mut Option<String>, what: &str, max: usize, lines: bool) -> Result<(), ApiError> {
    let Some(value) = text else { return Ok(()) };
    *value = value.replace("\r\n", "\n").trim().to_owned();
    let bad = value
        .chars()
        .any(|c| c.is_control() && !(lines && c == '\n'));
    if bad || value.chars().count() > max {
        return Err(ApiError::bad(format!(
            "{what} must be text of at most {max} characters{}",
            if lines { "" } else { " on one line" }
        )));
    }
    Ok(())
}

/// A link's label and address as they will be saved; both blank is no link.
fn link(link: &mut Link, what: &str) -> Result<(), ApiError> {
    link.label = link.label.split_whitespace().collect::<Vec<_>>().join(" ");
    link.url = link.url.trim().to_owned();
    if link.label.is_empty() && link.url.is_empty() {
        return Ok(());
    }
    if link.label.is_empty() || link.label.chars().count() > 40 {
        return Err(ApiError::bad(format!(
            "{what} needs a label of 1 to 40 characters"
        )));
    }
    link.url = crate::banner::link_url(&link.url).ok_or_else(|| {
        ApiError::bad(format!(
            "{what} must go to a web (http or https) or mail (mailto) address"
        ))
    })?;
    Ok(())
}

/// The branding as it will be saved.
fn check_branding(b: &mut Branding) -> Result<(), ApiError> {
    copy(&mut b.site_subtitle, "The subtitle", SHORT_CHARS, false)?;
    copy(&mut b.assistant_name, "The assistant's name", 40, false)?;
    copy(&mut b.description, "The description", LONG_CHARS, false)?;
    copy(
        &mut b.logo_alt,
        "The logo's description",
        SHORT_CHARS,
        false,
    )?;
    if let Some(icons) = &b.icons {
        for name in [&icons.greeting, &icons.assistant, &icons.loading]
            .into_iter()
            .flatten()
        {
            if name.len() > 24 || !name.bytes().all(|c| c.is_ascii_lowercase() || c == b'-') {
                return Err(ApiError::bad(
                    "An icon is one of the built-in names, or blank for none",
                ));
            }
        }
    }
    if let Some(login) = &mut b.login {
        copy(&mut login.title, "The sign-in title", SHORT_CHARS, true)?;
        copy(
            &mut login.description,
            "The sign-in description",
            LONG_CHARS,
            true,
        )?;
        copy(&mut login.sso_label, "The single sign-on button", 60, false)?;
        copy(
            &mut login.sso_hint,
            "The single sign-on hint",
            SHORT_CHARS,
            true,
        )?;
        copy(&mut login.help, "The sign-in help line", LONG_CHARS, true)?;
        if let Some(help_link) = &mut login.help_link {
            link(help_link, "The sign-in help link")?;
        }
    }
    if let Some(links) = &mut b.links {
        if links.len() > LINKS {
            return Err(ApiError::bad(format!("At most {LINKS} links")));
        }
        for item in links.iter_mut() {
            link(item, "A link")?;
        }
        links.retain(|item| !item.label.is_empty());
    }
    if let Some(home) = &mut b.home {
        copy(
            &mut home.eyebrow,
            "The home page's small line",
            SHORT_CHARS,
            false,
        )?;
        copy(&mut home.title, "The home page's title", SHORT_CHARS, true)?;
        copy(
            &mut home.description,
            "The home page's description",
            LONG_CHARS,
            true,
        )?;
        copy(&mut home.footer, "The home page's footer", LONG_CHARS, true)?;
    }
    Ok(())
}

/// A color as six hex digits with its #, lower case: "#D7F25A" and "d7f25a" both read.
fn hex(color: &str) -> Option<String> {
    let digits = color.trim().trim_start_matches('#');
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("#{}", digits.to_ascii_lowercase()))
}

/// The change as it will be saved.
fn check(body: Value) -> Result<Change, ApiError> {
    let mut change: Change = serde_json::from_value(body).map_err(|_| {
        ApiError::bad("An appearance has siteName, accent, defaultTheme, personalAccent, corners and branding; null leaves one to this server's file")
    })?;
    check_branding(&mut change.branding)?;
    if let Some(name) = &mut change.site_name {
        // One line; a blank name is no name of its own.
        *name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.chars().count() > NAME_CHARS || name.chars().any(char::is_control) {
            return Err(ApiError::bad(format!(
                "The site's name must be one line of at most {NAME_CHARS} characters"
            )));
        }
    }
    change.site_name = change.site_name.filter(|name| !name.is_empty());
    if let Some(accent) = &change.accent {
        change.accent = Some(hex(accent).ok_or_else(|| {
            ApiError::bad("The accent is a color as six hex digits, such as #d7f25a")
        })?);
    }
    if change
        .default_theme
        .as_deref()
        .is_some_and(|theme| !THEMES.contains(&theme))
    {
        return Err(ApiError::bad("The theme is system, light or dark"));
    }
    if change
        .corners
        .as_deref()
        .is_some_and(|corners| !CORNERS.contains(&corners))
    {
        return Err(ApiError::bad("Corners are sharp, soft or round"));
    }
    Ok(change)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/appearance", get(read).patch(save))
        .route(
            "/api/admin/branding/images/{slot}",
            axum::routing::put(upload).delete(remove),
        )
        .route("/api/branding/images/{slot}", get(image))
        .with_state(state)
}

async fn read(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    let mut doc = document(&read_row(&s.store.pool).await?);
    doc["images"] = Value::Object(images(&s.store.pool).await?);
    Ok(Json(doc))
}

async fn save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let change = check(body)?;
    let mut tx = crate::operations::admin_transaction(&s.store.pool, user.id).await?;
    let row: Row = sqlx::query_as(
        "UPDATE appearance SET site_name=$1, accent=$2, default_theme=$3, personal_accent=$4, corners=$5, branding=$6, updated_at=NOW() WHERE id
         RETURNING site_name, accent, default_theme, personal_accent, corners, branding, updated_at",
    )
    .bind(&change.site_name)
    .bind(&change.accent)
    .bind(&change.default_theme)
    .bind(change.personal_accent)
    .bind(&change.corners)
    .bind(serde_json::to_value(&change.branding).map_err(|_| ApiError::internal())?)
    .fetch_one(&mut *tx)
    .await?;
    let mut saved = document(&row);
    let mut logged = row.set();
    if row.branding != json!({}) {
        logged["branding"] = row.branding.clone();
    }
    audit::record(
        &mut *tx,
        Some(user.id),
        Action::APPEARANCE_UPDATE,
        Target::site(),
        logged,
    )
    .await?;
    tx.commit().await?;
    saved["images"] = Value::Object(images(&s.store.pool).await?);
    Ok(Json(saved))
}

/// What kind of image the bytes are, from the bytes themselves: what a browser was
/// told to send is not trusted.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if starts(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if starts(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else {
        // SVG is text: it opens with a tag (after an optional declaration or comment)
        // and has an <svg element near its start.
        let head = std::str::from_utf8(&bytes[..bytes.len().min(2048)]).ok()?;
        (head.trim_start().starts_with('<') && head.contains("<svg")).then_some("image/svg+xml")
    }
}

fn slot_name(slot: &str) -> Result<&'static str, ApiError> {
    SLOTS
        .into_iter()
        .find(|known| *known == slot)
        .ok_or_else(ApiError::not_found)
}

/// Puts an image in a slot: the logo on the light or the dark theme, or the tab icon.
async fn upload(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(slot): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let slot = slot_name(&slot)?;
    if body.is_empty() || body.len() > IMAGE_BYTES {
        return Err(ApiError::bad("The image must be at most 512 KB"));
    }
    let content_type = image_type(&body).ok_or_else(|| {
        ApiError::bad("The image must be a PNG, JPEG, WebP, GIF, SVG or ICO file")
    })?;
    let etag: String = Sha256::digest(&body)[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut tx = crate::operations::admin_transaction(&s.store.pool, user.id).await?;
    sqlx::query(
        "INSERT INTO branding_images(slot, content_type, bytes, etag) VALUES($1, $2, $3, $4)
         ON CONFLICT (slot) DO UPDATE SET content_type=$2, bytes=$3, etag=$4, updated_at=NOW()",
    )
    .bind(slot)
    .bind(content_type)
    .bind(body.as_ref())
    .bind(&etag)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        Action::APPEARANCE_UPDATE,
        Target::site(),
        json!({"image": slot, "contentType": content_type, "bytes": body.len()}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"images": images(&s.store.pool).await?})))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(slot): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let slot = slot_name(&slot)?;
    let mut tx = crate::operations::admin_transaction(&s.store.pool, user.id).await?;
    let gone = sqlx::query("DELETE FROM branding_images WHERE slot=$1")
        .bind(slot)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if gone > 0 {
        audit::record(
            &mut *tx,
            Some(user.id),
            Action::APPEARANCE_UPDATE,
            Target::site(),
            json!({"image": slot, "removed": true}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"images": images(&s.store.pool).await?})))
}

/// An uploaded image, for anyone: the sign-in page shows the logo too. It is served
/// as an image only and in a sandbox, so an SVG opened on its own runs nothing.
async fn image(State(s): State<AppState>, Path(slot): Path<String>) -> Result<Response, ApiError> {
    let slot = slot_name(&slot)?;
    let row: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT content_type, bytes FROM branding_images WHERE slot=$1")
            .bind(slot)
            .fetch_optional(&s.store.pool)
            .await?;
    let (content_type, bytes) = row.ok_or_else(ApiError::not_found)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.as_str()),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; sandbox",
            ),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_read_with_or_without_the_sign_in_either_case() {
        assert_eq!(hex("#D7F25A"), Some("#d7f25a".into()));
        assert_eq!(hex(" 7cb7ff "), Some("#7cb7ff".into()));
        for bad in ["", "#fff", "#12345g", "red", "#1234567", "url(x)"] {
            assert_eq!(hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_change_is_checked_before_it_is_saved() {
        let all = json!({"siteName": "  Research \n Computing  ", "accent": "5FD3BC", "defaultTheme": "dark", "personalAccent": false, "corners": "round"});
        let change = check(all).unwrap();
        assert_eq!(change.site_name.as_deref(), Some("Research Computing"));
        assert_eq!(change.accent.as_deref(), Some("#5fd3bc"));
        let none = check(json!({"siteName": " ", "accent": null, "defaultTheme": null, "personalAccent": null, "corners": null})).unwrap();
        assert!(
            none.site_name.is_none() && none.accent.is_none() && none.personal_accent.is_none()
        );
        for bad in [
            json!({"siteName": "x".repeat(61)}),
            json!({"accent": "chartreuse"}),
            json!({"defaultTheme": "sepia"}),
            json!({"corners": "square"}),
            json!({"personalAccent": "yes"}),
            json!({"logo": "/x.svg"}),
        ] {
            assert!(check(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn branding_keeps_only_what_was_sent_and_checks_it() {
        let change = check(json!({"branding": {
            "assistantName": "  Sparky ",
            "home": {"title": "Ask\r\nanything", "footer": ""},
            "login": {"helpLink": {"label": " Request  an account ", "url": "https://accounts.example.edu"}},
            "links": [{"label": "Help", "url": "mailto:help@example.edu"}, {"label": "", "url": ""}],
            "icons": {"greeting": "", "assistant": "book"},
            "hideLogo": true,
        }}))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&change.branding).unwrap(),
            json!({
                "assistantName": "Sparky",
                "home": {"title": "Ask\nanything", "footer": ""},
                "login": {"helpLink": {"label": "Request an account", "url": "https://accounts.example.edu"}},
                "links": [{"label": "Help", "url": "mailto:help@example.edu"}],
                "icons": {"greeting": "", "assistant": "book"},
                "hideLogo": true,
            })
        );
        assert_eq!(
            serde_json::to_value(check(json!({})).unwrap().branding).unwrap(),
            json!({})
        );
        for bad in [
            json!({"assistantName": "x".repeat(41)}),
            json!({"siteSubtitle": "two\nlines"}),
            json!({"home": {"title": "x".repeat(121)}}),
            json!({"links": [{"label": "Go", "url": "javascript:alert(1)"}]}),
            json!({"links": (0..9).map(|n| json!({"label": format!("L{n}"), "url": "https://example.edu"})).collect::<Vec<_>>()}),
            json!({"login": {"helpLink": {"label": "", "url": "https://example.edu"}}}),
            json!({"icons": {"greeting": "<svg>"}}),
            json!({"logo": {"light": "/x.svg"}}),
        ] {
            assert!(check(json!({"branding": bad.clone()})).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_image_is_known_by_its_bytes() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_type(b"\xff\xd8\xff\xe0...."), Some("image/jpeg"));
        assert_eq!(image_type(b"GIF89a...."), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(&[0, 0, 1, 0, 1, 0]), Some("image/x-icon"));
        assert_eq!(
            image_type(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some("image/svg+xml")
        );
        for bad in [
            &b"<html><body>hi</body></html>"[..],
            b"plain text",
            b"",
            b"MZ\x90\0",
        ] {
            assert_eq!(image_type(bad), None);
        }
    }
}
