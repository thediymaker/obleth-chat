//! The site's look as an administrator set it: its name, the accent color, the theme
//! people start in, whether they may pick their own accent, and how round the corners
//! are. Each setting is optional: one left unset stays as the installation's
//! branding/site.config.js has it. It reaches every page with the session answer, the
//! sign-in page included, so the look is the same before and after signing in.
use crate::audit::{self, Action, Target};
use crate::{error::ApiError, AppState};
use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

const NAME_CHARS: usize = 60;
const THEMES: [&str; 3] = ["system", "light", "dark"];
const CORNERS: [&str; 3] = ["sharp", "soft", "round"];

/// A change sets every setting: a null puts that one back to the installation's own.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    site_name: Option<String>,
    accent: Option<String>,
    default_theme: Option<String>,
    personal_accent: Option<bool>,
    corners: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Row {
    site_name: Option<String>,
    accent: Option<String>,
    default_theme: Option<String>,
    personal_accent: Option<bool>,
    corners: Option<String>,
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
        "updatedAt": row.updated_at,
    })
}

/// The settings that are set, for every page: `{}` when the file decides everything.
pub async fn shown(pool: &PgPool) -> Result<Value, ApiError> {
    Ok(read_row(pool).await?.set())
}

async fn read_row(pool: &PgPool) -> Result<Row, ApiError> {
    Ok(sqlx::query_as(
        "SELECT site_name, accent, default_theme, personal_accent, corners, updated_at FROM appearance WHERE id",
    )
    .fetch_one(pool)
    .await?)
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
        ApiError::bad("An appearance has siteName, accent, defaultTheme, personalAccent and corners; null leaves one to the installation")
    })?;
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
        .with_state(state)
}

async fn read(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    s.auth.require_admin(&headers, false).await?;
    Ok(Json(document(&read_row(&s.store.pool).await?)))
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
        "UPDATE appearance SET site_name=$1, accent=$2, default_theme=$3, personal_accent=$4, corners=$5, updated_at=NOW() WHERE id
         RETURNING site_name, accent, default_theme, personal_accent, corners, updated_at",
    )
    .bind(&change.site_name)
    .bind(&change.accent)
    .bind(&change.default_theme)
    .bind(change.personal_accent)
    .bind(&change.corners)
    .fetch_one(&mut *tx)
    .await?;
    let saved = document(&row);
    audit::record(
        &mut *tx,
        Some(user.id),
        Action::APPEARANCE_UPDATE,
        Target::site(),
        row.set(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(saved))
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
}
