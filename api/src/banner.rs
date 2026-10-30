//! The banner: one line an administrator shows at the top of every page, such as an
//! announcement or a note that the service is in development. It reaches people with
//! their session, so it shows from their next page load.
use crate::{error::ApiError, AppState};
use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const TEXT_CHARS: usize = 200;
const LABEL_CHARS: usize = 40;
const URL_CHARS: usize = 2048;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    text: String,
    #[serde(default)]
    link: Option<Link>,
    tone: String,
    signed_out: bool,
    dismissible: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    label: String,
    url: String,
}

#[derive(sqlx::FromRow)]
struct Row {
    text: String,
    link_label: Option<String>,
    link_url: Option<String>,
    tone: String,
    signed_out: bool,
    dismissible: bool,
    updated_at: DateTime<Utc>,
}

impl Row {
    fn link(&self) -> Value {
        match (&self.link_label, &self.link_url) {
            (Some(label), Some(url)) => json!({"label": label, "url": url}),
            _ => Value::Null,
        }
    }
}

/// What administrators see and edit.
fn document(row: &Row) -> Value {
    json!({
        "text": row.text,
        "link": row.link(),
        "tone": row.tone,
        "signedOut": row.signed_out,
        "dismissible": row.dismissible,
        "updatedAt": row.updated_at,
    })
}

/// The banner one person sees, or null: none is set, or they are signed out and it
/// is only for people who are signed in.
pub async fn shown(pool: &PgPool, signed_in: bool) -> Result<Value, ApiError> {
    let row = read_row(pool).await?;
    if row.text.is_empty() || !(signed_in || row.signed_out) {
        return Ok(Value::Null);
    }
    Ok(
        json!({"text": row.text, "link": row.link(), "tone": row.tone, "dismissible": row.dismissible}),
    )
}

async fn read_row(pool: &PgPool) -> Result<Row, ApiError> {
    Ok(sqlx::query_as(
        "SELECT text, link_label, link_url, tone, signed_out, dismissible, updated_at FROM banner WHERE id",
    )
    .fetch_one(pool)
    .await?)
}

/// The text as one line: runs of spaces and line breaks become one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A link may go to a web or mail address only.
fn link_url(url: &str) -> Option<String> {
    let url = url.trim();
    let parsed = url::Url::parse(url).ok()?;
    (["http", "https", "mailto"].contains(&parsed.scheme())
        && url.chars().count() <= URL_CHARS
        && !url.chars().any(|c| c.is_whitespace() || c.is_control()))
    .then(|| url.to_owned())
}

/// The change as it will be saved: text on one line, and a link only to a web or
/// mail address.
fn check(body: Value) -> Result<Change, ApiError> {
    let mut change: Change = serde_json::from_value(body).map_err(|_| {
        ApiError::bad("A banner has text, an optional link, a tone and two switches: signedOut and dismissible")
    })?;
    change.text = one_line(&change.text);
    if change.text.chars().count() > TEXT_CHARS || change.text.chars().any(char::is_control) {
        return Err(ApiError::bad(format!(
            "The banner's text must be one line of at most {TEXT_CHARS} characters"
        )));
    }
    if let Some(link) = &mut change.link {
        link.label = one_line(&link.label);
        if link.label.is_empty() || link.label.chars().count() > LABEL_CHARS {
            return Err(ApiError::bad(format!(
                "The link's label must be 1 to {LABEL_CHARS} characters"
            )));
        }
        link.url = link_url(&link.url).ok_or_else(|| {
            ApiError::bad("The link must be a web (http or https) or mail (mailto) address")
        })?;
    }
    if !["info", "warning"].contains(&change.tone.as_str()) {
        return Err(ApiError::bad("The banner's tone is info or warning"));
    }
    Ok(change)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/banner", get(read).patch(save))
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
    let (link_label, link_url) = change.link.map(|l| (l.label, l.url)).unzip();
    let mut tx = crate::operations::admin_transaction(&s.store.pool, user.id).await?;
    let row: Row = sqlx::query_as(
        "UPDATE banner SET text=$1, link_label=$2, link_url=$3, tone=$4, signed_out=$5, dismissible=$6, updated_at=NOW() WHERE id RETURNING text, link_label, link_url, tone, signed_out, dismissible, updated_at",
    )
    .bind(&change.text)
    .bind(&link_label)
    .bind(&link_url)
    .bind(&change.tone)
    .bind(change.signed_out)
    .bind(change.dismissible)
    .fetch_one(&mut *tx)
    .await?;
    let saved = document(&row);
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,metadata)VALUES($1,$2,'banner.update','site',$3)")
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(json!({"text": row.text, "link": row.link(), "tone": row.tone, "signedOut": row.signed_out, "dismissible": row.dismissible}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_reads_as_one_line_and_links_go_only_to_web_or_mail() {
        assert_eq!(
            one_line("  In development:\n  features   may change. "),
            "In development: features may change."
        );
        assert_eq!(
            link_url(" https://example.edu/feedback "),
            Some("https://example.edu/feedback".into())
        );
        assert_eq!(
            link_url("mailto:help@example.edu"),
            Some("mailto:help@example.edu".into())
        );
        for bad in [
            "javascript:alert(1)",
            "/relative",
            "data:text/html,hi",
            "https://example.edu/a b",
            "",
        ] {
            assert_eq!(link_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_change_is_checked_before_it_is_saved() {
        let ok = json!({"text": "In development", "link": null, "tone": "warning", "signedOut": true, "dismissible": false});
        assert!(check(ok).is_ok());
        for bad in [
            json!({"text": "x".repeat(201), "tone": "info", "signedOut": false, "dismissible": false}),
            json!({"text": "Hi", "tone": "loud", "signedOut": false, "dismissible": false}),
            json!({"text": "Hi", "link": {"label": "", "url": "https://example.edu"}, "tone": "info", "signedOut": false, "dismissible": false}),
            json!({"text": "Hi", "link": {"label": "Go", "url": "javascript:alert(1)"}, "tone": "info", "signedOut": false, "dismissible": false}),
            json!({"text": "Hi", "tone": "info", "signedOut": false}),
            json!({"text": "Hi", "tone": "info", "signedOut": false, "dismissible": false, "color": "red"}),
        ] {
            assert!(check(bad.clone()).is_err(), "{bad}");
        }
    }
}
