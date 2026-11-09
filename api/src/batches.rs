//! Batches: a spreadsheet of inputs turned into one request per row, run in the
//! background by the gateway's `/v1/batches` as the person, and handed back as the
//! same spreadsheet with the answers added.
//!
//! The gateway owns the batch: its queue, its progress and its results, private to
//! the person's gateway identity. Chat keeps only what it needs to put the answers
//! back next to the rows they came from (`batch_sources`), and the tags and names
//! people give their batches (`batch_labels`).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, TimeZone, Utc};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

use crate::{
    auth::Identity,
    error::ApiError,
    gateway::{reason, upstream_error, valid_model, Gateway},
    AppState,
};

/// Most rows in one batch: the gateway takes up to 50,000 requests per file.
pub const MAX_ROWS: usize = 50_000;
const MAX_COLUMNS: usize = 200;
const MAX_COLUMN_NAME: usize = 200;
/// Most characters in one cell.
const MAX_CELL: usize = 100_000;
const MAX_PROMPT: usize = 50_000;
const MAX_NAME: usize = 200;
/// The JSON a browser may send to start a batch.
const MAX_BODY: usize = 96 * 1024 * 1024;
/// The input file sent to the gateway (its own limit is 200 MB by default).
const MAX_FILE: usize = 190 * 1024 * 1024;
/// What a results download may read from the gateway.
const MAX_RESULTS: usize = 512 * 1024 * 1024;
/// How long Chat keeps a batch's rows; the gateway keeps batches 30 days.
const KEEP_DAYS: i32 = 35;
/// Deadlines a person may choose.
const WINDOWS: &[&str] = &["24h", "3d", "7d"];
/// The gateway lists 100 batches a page; the page shows up to this many pages.
const LIST_PAGES: usize = 5;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/batches",
            get(list)
                .post(create)
                .layer(DefaultBodyLimit::max(MAX_BODY)),
        )
        .route("/api/batches/{id}", patch(update).delete(remove))
        .route("/api/batches/{id}/cancel", post(cancel))
        .route("/api/batches/{id}/results", get(results))
        .with_state(state)
}

fn gateway(state: &AppState) -> Result<&Gateway, ApiError> {
    state.gateway.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Batches need the gateway, which isn't set up here".into(),
        )
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A gateway answer that isn't a success, as the person should read it: the
/// gateway's own words for a request it refused, the usual mapping otherwise.
async fn refused(response: reqwest::Response) -> ApiError {
    let status = response.status().as_u16();
    match status {
        400 | 404 | 409 | 413 | 422 => {
            let body = tokio::time::timeout(Duration::from_secs(5), response.bytes())
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let message = reason(&body).unwrap_or_else(|| match status {
                404 => "Not found".into(),
                413 => "The spreadsheet is too large for one batch".into(),
                _ => "The gateway refused this batch".into(),
            });
            let code = match status {
                404 => StatusCode::NOT_FOUND,
                409 => StatusCode::CONFLICT,
                413 => StatusCode::PAYLOAD_TOO_LARGE,
                _ => StatusCode::BAD_REQUEST,
            };
            ApiError(code, message)
        }
        _ => upstream_error(response).await,
    }
}

async fn call(
    gw: &Gateway,
    user: &Identity,
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, ApiError> {
    let response = request
        .header(header::AUTHORIZATION, gw.authorization(user).await?)
        .send()
        .await
        .map_err(|_| crate::gateway::unavailable())?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(refused(response).await)
    }
}

/// A response body, refusing one larger than `limit`.
async fn read_capped(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ApiError> {
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(part) = stream.next().await {
        let part = part.map_err(|_| crate::gateway::unavailable())?;
        if bytes.len() + part.len() > limit {
            return Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "The results are too large to put together here".into(),
            ));
        }
        bytes.extend_from_slice(&part);
    }
    Ok(bytes)
}

async fn json_of(response: reqwest::Response) -> Result<Value, ApiError> {
    let bytes = read_capped(response, 8 * 1024 * 1024).await?;
    serde_json::from_slice(&bytes).map_err(|_| crate::gateway::unavailable())
}

/// What Chat kept about a batch it started.
struct Source {
    name: String,
    file_name: String,
    row_count: i32,
}

fn time(v: &Value) -> Option<String> {
    v.as_i64()
        .and_then(|t| Utc.timestamp_opt(t, 0).single())
        .map(|t: DateTime<Utc>| t.to_rfc3339())
}

/// The tags and name a person gave a batch.
#[derive(Default, Clone)]
struct Label {
    name: Option<String>,
    tags: Vec<String>,
}

/// A gateway batch object as the page reads it.
fn entry(b: &Value, source: Option<&Source>, label: Option<&Label>) -> Value {
    let counts = &b["request_counts"];
    let n = |k: &str| counts[k].as_i64().unwrap_or(0);
    let finished = ["completed_at", "failed_at", "expired_at", "cancelled_at"]
        .iter()
        .find_map(|k| time(&b[*k]));
    let error = b["errors"]["data"][0]["message"].as_str().map(|m| {
        match b["errors"]["data"][0]["line"].as_i64() {
            Some(line) => format!("Line {line}: {m}"),
            None => m.to_string(),
        }
    });
    let name = label
        .and_then(|l| l.name.clone())
        .or_else(|| source.map(|s| s.name.clone()))
        .or_else(|| b["metadata"]["name"].as_str().map(String::from))
        .unwrap_or_else(|| b["id"].as_str().unwrap_or_default().to_string());
    json!({
        "id": b["id"],
        "name": name,
        "model": b["obleth"]["model"].as_str().unwrap_or_default(),
        "status": b["status"],
        "total": n("total"),
        "completed": n("completed"),
        "failed": n("failed"),
        "createdAt": time(&b["created_at"]),
        "inProgressAt": time(&b["in_progress_at"]),
        "finishedAt": finished,
        "deadlineAt": time(&b["obleth"]["deadline_at"]).or_else(|| time(&b["expires_at"])),
        "lastProgressAt": time(&b["obleth"]["last_progress_at"]),
        "waiting": b["obleth"]["waiting"].as_str() == Some("capacity"),
        "window": b["completion_window"],
        "error": error,
        "fileName": source.map(|s| s.file_name.clone()),
        "rows": source.map(|s| s.row_count),
        "fromChat": source.is_some(),
        "tags": label.map(|l| l.tags.clone()).unwrap_or_default(),
    })
}

async fn labels(
    state: &AppState,
    user: Uuid,
    ids: &[String],
) -> Result<HashMap<String, Label>, ApiError> {
    let rows: Vec<(String, Option<String>, Vec<String>)> = sqlx::query_as(
        "SELECT batch_id, name, tags FROM batch_labels
         WHERE user_id = $1 AND batch_id = ANY($2)",
    )
    .bind(user)
    .bind(ids)
    .fetch_all(&state.store.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, tags)| (id, Label { name, tags }))
        .collect())
}

/// One batch as the page reads it, with what Chat kept about it.
async fn entry_for(state: &AppState, user: Uuid, b: &Value) -> Result<Value, ApiError> {
    let id = b["id"].as_str().unwrap_or_default().to_string();
    let ids = std::slice::from_ref(&id);
    let kept = sources(state, user, ids).await?;
    let label = labels(state, user, ids).await?;
    Ok(entry(b, kept.get(&id), label.get(&id)))
}

async fn sources(
    state: &AppState,
    user: Uuid,
    ids: &[String],
) -> Result<HashMap<String, Source>, ApiError> {
    let rows: Vec<(String, String, String, i32)> = sqlx::query_as(
        "SELECT batch_id, name, file_name, row_count FROM batch_sources
         WHERE user_id = $1 AND batch_id = ANY($2)",
    )
    .bind(user)
    .bind(ids)
    .fetch_all(&state.store.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, file_name, row_count)| {
            (
                id,
                Source {
                    name,
                    file_name,
                    row_count,
                },
            )
        })
        .collect())
}

/// The person's batches, newest first, with their progress.
async fn list(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, false).await?;
    let gw = gateway(&state)?;
    let mut list: Vec<Value> = Vec::new();
    let mut more = false;
    for page in 0..LIST_PAGES {
        let path = match list.last().and_then(|b| b["id"].as_str()) {
            Some(after) => format!("v1/batches?limit=100&after={after}"),
            None => "v1/batches?limit=100".to_string(),
        };
        let response = gw
            .http()
            .get(gw.url(&path))
            .header(header::AUTHORIZATION, gw.authorization(&user).await?)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| crate::gateway::unavailable())?;
        // A gateway without batches (another kind, an older one, or turned off).
        if page == 0 && matches!(response.status().as_u16(), 404 | 405 | 501) {
            return Ok(Json(json!({"available": false, "batches": []})));
        }
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        let data = json_of(response).await?;
        let got = data["data"].as_array().cloned().unwrap_or_default();
        more = data["has_more"].as_bool().unwrap_or(false) && !got.is_empty();
        list.extend(got);
        if !more {
            break;
        }
    }
    let ids: Vec<String> = list
        .iter()
        .filter_map(|b| b["id"].as_str().map(String::from))
        .collect();
    let kept = sources(&state, user.id, &ids).await?;
    let label = labels(&state, user.id, &ids).await?;
    // Rows outlive the gateway's own retention by a few days, then go.
    for table in ["batch_sources", "batch_labels"] {
        let stamp = if table == "batch_sources" {
            "created_at"
        } else {
            "updated_at"
        };
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE user_id = $1
             AND {stamp} < NOW() - make_interval(days => $2)"
        ))
        .bind(user.id)
        .bind(KEEP_DAYS)
        .execute(&state.store.pool)
        .await?;
    }
    let batches: Vec<Value> = list
        .iter()
        .map(|b| {
            let id = b["id"].as_str().unwrap_or_default();
            entry(b, kept.get(id), label.get(id))
        })
        .collect();
    Ok(Json(
        json!({"available": true, "batches": batches, "more": more}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewBatch {
    name: String,
    model: String,
    #[serde(default)]
    system: String,
    template: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    file_name: String,
}

/// `template` with each `{column}` replaced by that column's value in `row`.
/// Braces around anything that isn't a column stay as they are.
pub fn render(template: &str, columns: &HashMap<&str, usize>, row: &[String]) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if columns.contains_key(&after[..close]) => {
                let i = columns[&after[..close]];
                out.push_str(row.get(i).map(String::as_str).unwrap_or(""));
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Placeholders in `template` that name no column.
pub fn unknown_placeholders(template: &str, columns: &HashMap<&str, usize>) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                if !name.is_empty()
                    && name.len() <= MAX_COLUMN_NAME
                    && !name.contains('{')
                    && !columns.contains_key(name)
                    && !found.iter().any(|f| f == name)
                {
                    found.push(name.to_string());
                }
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    found
}

fn custom_id(row: usize) -> String {
    format!("row-{}", row + 1)
}

fn row_of(custom_id: &str) -> Option<usize> {
    custom_id
        .strip_prefix("row-")
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| n.checked_sub(1))
}

/// The JSONL input file for `batch`: one chat request per row that isn't blank.
fn input_file(batch: &NewBatch) -> Result<(Vec<u8>, usize), ApiError> {
    let columns: HashMap<&str, usize> = batch
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| (c.as_str(), i))
        .collect();
    let mut out = Vec::new();
    let mut sent = 0;
    for (i, row) in batch.rows.iter().enumerate() {
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let prompt = render(&batch.template, &columns, row);
        if prompt.chars().count() > MAX_PROMPT * 4 {
            return Err(ApiError::bad(format!(
                "Row {} makes a prompt longer than {} characters",
                i + 1,
                MAX_PROMPT * 4
            )));
        }
        let mut messages = Vec::new();
        if !batch.system.trim().is_empty() {
            messages.push(json!({"role": "system", "content": batch.system}));
        }
        messages.push(json!({"role": "user", "content": prompt}));
        let line = json!({
            "custom_id": custom_id(i),
            "method": "POST",
            "url": "/v1/chat/completions",
            "body": {"model": batch.model, "messages": messages},
        });
        serde_json::to_writer(&mut out, &line).map_err(|_| ApiError::internal())?;
        out.push(b'\n');
        sent += 1;
        if out.len() > MAX_FILE {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "The prompts add up to more than one batch can hold. Split the spreadsheet.".into(),
            ));
        }
    }
    if sent == 0 {
        return Err(ApiError::bad("Every row is blank"));
    }
    Ok((out, sent))
}

fn check(batch: &mut NewBatch) -> Result<(), ApiError> {
    batch.name = batch.name.trim().chars().take(MAX_NAME).collect();
    if batch.name.is_empty() {
        batch.name = "Batch".into();
    }
    batch.file_name = batch.file_name.trim().chars().take(255).collect();
    if !valid_model(&batch.model) || batch.model == "auto" {
        return Err(ApiError::bad("Choose a model for the batch"));
    }
    if batch.template.trim().is_empty() {
        return Err(ApiError::bad("Write the prompt each row is sent with"));
    }
    if batch.template.chars().count() > MAX_PROMPT || batch.system.chars().count() > MAX_PROMPT {
        return Err(ApiError::bad(format!(
            "The prompt and instructions can each be up to {MAX_PROMPT} characters"
        )));
    }
    if batch.columns.is_empty() || batch.columns.len() > MAX_COLUMNS {
        return Err(ApiError::bad(format!(
            "The spreadsheet needs 1 to {MAX_COLUMNS} columns"
        )));
    }
    let mut seen = HashSet::new();
    for c in &batch.columns {
        if c.is_empty() || c.chars().count() > MAX_COLUMN_NAME {
            return Err(ApiError::bad(format!(
                "Column names must be 1 to {MAX_COLUMN_NAME} characters"
            )));
        }
        if !seen.insert(c.as_str()) {
            return Err(ApiError::bad(format!("Two columns are named \"{c}\"")));
        }
    }
    if batch.rows.is_empty() {
        return Err(ApiError::bad("The spreadsheet has no rows"));
    }
    if batch.rows.len() > MAX_ROWS {
        return Err(ApiError::bad(format!(
            "A batch can hold up to {MAX_ROWS} rows; this one has {}",
            batch.rows.len()
        )));
    }
    let width = batch.columns.len();
    for (i, row) in batch.rows.iter_mut().enumerate() {
        row.resize(width, String::new());
        if let Some(cell) = row.iter().find(|c| c.chars().count() > MAX_CELL) {
            let _ = cell;
            return Err(ApiError::bad(format!(
                "Row {} has a cell longer than {MAX_CELL} characters",
                i + 1
            )));
        }
    }
    match batch.window.as_deref() {
        None | Some("") => batch.window = Some("24h".into()),
        Some(w) if WINDOWS.contains(&w) => {}
        Some(_) => return Err(ApiError::bad("Choose a deadline of 24h, 3d or 7d")),
    }
    Ok(())
}

/// A multipart body with the purpose field and the file, as `/v1/files` takes it.
fn multipart(boundary: &str, file: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(file.len() + 512);
    write!(
        body,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"purpose\"\r\n\r\nbatch\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"chat-batch.jsonl\"\r\n\
         Content-Type: application/jsonl\r\n\r\n"
    )
    .expect("writing to a Vec");
    body.extend_from_slice(file);
    write!(body, "\r\n--{boundary}--\r\n").expect("writing to a Vec");
    body
}

fn gzip_rows(rows: &[Vec<String>]) -> Result<Vec<u8>, ApiError> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut gz, rows).map_err(|_| ApiError::internal())?;
    gz.finish().map_err(|_| ApiError::internal())
}

fn gunzip_rows(bytes: &[u8]) -> Option<Vec<Vec<String>>> {
    let mut text = Vec::new();
    GzDecoder::new(bytes)
        .take(1024 * 1024 * 1024)
        .read_to_end(&mut text)
        .ok()?;
    serde_json::from_slice(&text).ok()
}

/// Starts a batch from a spreadsheet's rows and a prompt.
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut batch): Json<NewBatch>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    let gw = gateway(&state)?;
    check(&mut batch)?;
    let (file, _sent) = input_file(&batch)?;

    let boundary = format!("chatbatch{}", Uuid::new_v4().simple());
    let uploaded = call(
        gw,
        &user,
        gw.http()
            .post(gw.url("v1/files"))
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .timeout(Duration::from_secs(300))
            .body(multipart(&boundary, &file)),
    )
    .await?;
    let file_id = json_of(uploaded).await?["id"]
        .as_str()
        .map(String::from)
        .ok_or_else(crate::gateway::unavailable)?;

    let created = call(
        gw,
        &user,
        gw.http()
            .post(gw.url("v1/batches"))
            .timeout(Duration::from_secs(300))
            .json(&json!({
                "input_file_id": file_id,
                "endpoint": "/v1/chat/completions",
                "completion_window": batch.window,
                "metadata": {"name": batch.name, "source": "chat"},
            })),
    )
    .await?;
    let created = json_of(created).await?;
    let id = created["id"]
        .as_str()
        .filter(|id| valid_id(id))
        .ok_or_else(crate::gateway::unavailable)?
        .to_string();

    let rows = gzip_rows(&batch.rows)?;
    sqlx::query(
        "INSERT INTO batch_sources
           (batch_id, user_id, name, model, file_name, columns, rows, row_count, system_prompt, template)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (batch_id) DO NOTHING",
    )
    .bind(&id)
    .bind(user.id)
    .bind(&batch.name)
    .bind(&batch.model)
    .bind(&batch.file_name)
    .bind(json!(batch.columns))
    .bind(rows)
    .bind(batch.rows.len() as i32)
    .bind(&batch.system)
    .bind(&batch.template)
    .execute(&state.store.pool)
    .await?;
    tracing::info!(batch = %id, model = %batch.model, rows = batch.rows.len(), "batch started from Chat");
    let source = Source {
        name: batch.name.clone(),
        file_name: batch.file_name.clone(),
        row_count: batch.rows.len() as i32,
    };
    Ok(Json(json!({"batch": entry(&created, Some(&source), None)})))
}

async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    if !valid_id(&id) {
        return Err(ApiError::not_found());
    }
    let gw = gateway(&state)?;
    let response = call(
        gw,
        &user,
        gw.http()
            .post(gw.url(&format!("v1/batches/{id}/cancel")))
            .timeout(Duration::from_secs(30)),
    )
    .await?;
    let b = json_of(response).await?;
    Ok(Json(
        json!({"batch": entry_for(&state, user.id, &b).await?}),
    ))
}

#[derive(Deserialize)]
struct LabelChange {
    /// A new name; an empty one goes back to the name the batch started with.
    name: Option<String>,
    tags: Option<Vec<String>>,
}

/// Renames a batch or sets its tags, for this person only.
async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(change): Json<LabelChange>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    if !valid_id(&id) {
        return Err(ApiError::not_found());
    }
    let gw = gateway(&state)?;
    // Only a batch the person's gateway identity can see.
    let response = call(
        gw,
        &user,
        gw.http()
            .get(gw.url(&format!("v1/batches/{id}")))
            .timeout(Duration::from_secs(15)),
    )
    .await?;
    let b = json_of(response).await?;
    let current = labels(&state, user.id, std::slice::from_ref(&id))
        .await?
        .remove(&id)
        .unwrap_or_default();
    let name = match change.name {
        Some(n) => {
            let n = n.trim();
            if n.chars().count() > MAX_NAME {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    format!("A name can be up to {MAX_NAME} characters"),
                ));
            }
            (!n.is_empty()).then(|| n.to_string())
        }
        None => current.name,
    };
    let tags = match change.tags {
        Some(t) => crate::knowledge::saved::clean_tags(&t),
        None => current.tags,
    };
    if name.is_none() && tags.is_empty() {
        sqlx::query("DELETE FROM batch_labels WHERE user_id = $1 AND batch_id = $2")
            .bind(user.id)
            .bind(&id)
            .execute(&state.store.pool)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO batch_labels (user_id, batch_id, name, tags) VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, batch_id)
             DO UPDATE SET name = EXCLUDED.name, tags = EXCLUDED.tags, updated_at = NOW()",
        )
        .bind(user.id)
        .bind(&id)
        .bind(&name)
        .bind(&tags)
        .execute(&state.store.pool)
        .await?;
    }
    Ok(Json(
        json!({"batch": entry_for(&state, user.id, &b).await?}),
    ))
}

/// Deletes a batch that has stopped: the gateway drops its requests, answers and
/// files, and Chat forgets its rows and labels.
async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user = state.auth.require(&headers, true).await?;
    if !valid_id(&id) {
        return Err(ApiError::not_found());
    }
    let gw = gateway(&state)?;
    let response = gw
        .http()
        .delete(gw.url(&format!("v1/batches/{id}")))
        .header(header::AUTHORIZATION, gw.authorization(&user).await?)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| crate::gateway::unavailable())?;
    let status = response.status().as_u16();
    if !response.status().is_success() && !matches!(status, 404 | 405) {
        return Err(refused(response).await);
    }
    if !response.status().is_success() {
        // Gone already, or a gateway that can't delete batches: tell them apart.
        let still = gw
            .http()
            .get(gw.url(&format!("v1/batches/{id}")))
            .header(header::AUTHORIZATION, gw.authorization(&user).await?)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| crate::gateway::unavailable())?;
        if still.status().is_success() {
            return Err(ApiError(
                StatusCode::NOT_IMPLEMENTED,
                "This gateway can't delete batches yet".into(),
            ));
        }
    }
    for table in ["batch_sources", "batch_labels"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE user_id = $1 AND batch_id = $2"
        ))
        .bind(user.id)
        .bind(&id)
        .execute(&state.store.pool)
        .await?;
    }
    tracing::info!(batch = %id, "batch deleted from Chat");
    Ok(Json(json!({"id": id, "deleted": true})))
}

/// One finished request: its answer, or what went wrong.
#[derive(Default, Clone)]
struct Outcome {
    answer: String,
    status: &'static str,
    error: String,
}

/// The answer text of an output line, or the error of an error-file line.
fn outcome_of(line: &Value) -> Outcome {
    let body = &line["response"]["body"];
    let code = line["response"]["status_code"].as_i64().unwrap_or(0);
    if line["error"].is_null() && (200..300).contains(&code) {
        let answer = body["choices"][0]["message"]["content"]
            .as_str()
            .or_else(|| body["choices"][0]["text"].as_str())
            .unwrap_or_default()
            .to_string();
        return Outcome {
            answer,
            status: "done",
            error: String::new(),
        };
    }
    let code_name = line["error"]["code"].as_str().unwrap_or_default();
    let message = line["error"]["message"]
        .as_str()
        .or_else(|| body["error"]["message"].as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("The model answered with status {code}"));
    let status = match code_name {
        "batch_expired" => "ran out of time",
        // The gateway names a row a cancel stopped by its status.
        "cancelled" | "batch_cancelled" => "cancelled",
        _ => "failed",
    };
    Outcome {
        answer: String::new(),
        status,
        error: message,
    }
}

fn parse_lines(bytes: &[u8], into: &mut HashMap<String, Outcome>) {
    for line in bytes.split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(line) {
            if let Some(id) = v["custom_id"].as_str() {
                into.insert(id.to_string(), outcome_of(&v));
            }
        }
    }
}

/// One CSV field, quoted when it needs to be. A cell that a spreadsheet would
/// read as a formula is written as text.
pub fn csv_field(value: &str, out: &mut String) {
    let formula = value.starts_with(['=', '+', '@', '\t', '\r']);
    let quote = formula || value.contains([',', '"', '\n', '\r']);
    if quote {
        out.push('"');
        if formula {
            out.push('\'');
        }
        for c in value.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
    } else {
        out.push_str(value);
    }
}

fn csv_row<'a>(fields: impl IntoIterator<Item = &'a str>, out: &mut String) {
    for (i, f) in fields.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        csv_field(f, out);
    }
    out.push_str("\r\n");
}

fn file_name_for(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-');
    format!(
        "{}-results.csv",
        if safe.is_empty() { "batch" } else { safe }
    )
}

/// The results so far as a spreadsheet: the rows the batch came from with the
/// answer, its status and any error added. Works while the batch still runs.
async fn results(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = state.auth.require(&headers, false).await?;
    if !valid_id(&id) {
        return Err(ApiError::not_found());
    }
    let gw = gateway(&state)?;
    let batch = json_of(
        call(
            gw,
            &user,
            gw.http()
                .get(gw.url(&format!("v1/batches/{id}")))
                .timeout(Duration::from_secs(15)),
        )
        .await?,
    )
    .await?;
    let mut outcomes = HashMap::new();
    for errors in [false, true] {
        let path = if errors {
            format!("v1/batches/{id}/results?errors=true")
        } else {
            format!("v1/batches/{id}/results")
        };
        let response = call(
            gw,
            &user,
            gw.http()
                .get(gw.url(&path))
                .timeout(Duration::from_secs(600)),
        )
        .await?;
        parse_lines(&read_capped(response, MAX_RESULTS).await?, &mut outcomes);
    }
    let source: Option<(String, Value, Vec<u8>)> = sqlx::query_as(
        "SELECT name, columns, rows FROM batch_sources WHERE batch_id = $1 AND user_id = $2",
    )
    .bind(&id)
    .bind(user.id)
    .fetch_optional(&state.store.pool)
    .await?;
    let running = matches!(
        batch["status"].as_str(),
        Some("validating" | "in_progress" | "finalizing")
    );
    let pending = if running { "waiting" } else { "not run" };

    // A UTF-8 byte order mark, so spreadsheet programs read accents and symbols.
    let mut out = String::from("\u{feff}");
    let name = match source {
        Some((name, columns, rows)) => {
            let columns: Vec<String> = serde_json::from_value(columns).unwrap_or_default();
            let rows = gunzip_rows(&rows).unwrap_or_default();
            let mut header: Vec<&str> = columns.iter().map(String::as_str).collect();
            header.extend(["answer", "status", "error"]);
            csv_row(header, &mut out);
            for (i, row) in rows.iter().enumerate() {
                let blank = row.iter().all(|c| c.trim().is_empty());
                let o = outcomes.get(&custom_id(i)).cloned().unwrap_or(Outcome {
                    status: if blank { "skipped (blank)" } else { pending },
                    ..Outcome::default()
                });
                let mut fields: Vec<&str> = (0..columns.len())
                    .map(|c| row.get(c).map(String::as_str).unwrap_or(""))
                    .collect();
                fields.extend([o.answer.as_str(), o.status, o.error.as_str()]);
                csv_row(fields, &mut out);
            }
            name
        }
        None => {
            csv_row(["custom_id", "answer", "status", "error"], &mut out);
            let mut ids: Vec<&String> = outcomes.keys().collect();
            ids.sort_by_key(|k| (row_of(k).unwrap_or(usize::MAX), k.to_string()));
            for k in ids {
                let o = &outcomes[k];
                csv_row([k.as_str(), &o.answer, o.status, &o.error], &mut out);
            }
            batch["metadata"]["name"]
                .as_str()
                .unwrap_or(id.as_str())
                .to_string()
        }
    };
    // A name the person gave it names the file too.
    let name = labels(&state, user.id, std::slice::from_ref(&id))
        .await?
        .remove(&id)
        .and_then(|l| l.name)
        .unwrap_or(name);
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", file_name_for(&name)),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        out,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&'static str]) -> HashMap<&'static str, usize> {
        names.iter().enumerate().map(|(i, c)| (*c, i)).collect()
    }

    #[test]
    fn placeholders_take_the_row_values() {
        let c = cols(&["title", "abstract"]);
        let row = vec!["Gaudi".to_string(), "We measure {things}.".to_string()];
        assert_eq!(
            render("Summarize {title}: {abstract} {missing} {", &c, &row),
            "Summarize Gaudi: We measure {things}. {missing} {"
        );
        assert_eq!(
            unknown_placeholders("{title} {missing} {missing} {}", &c),
            vec!["missing".to_string()]
        );
    }

    #[test]
    fn blank_rows_are_skipped_and_ids_follow_the_rows() {
        let mut b = NewBatch {
            name: " ".into(),
            model: "m".into(),
            system: "Be brief".into(),
            template: "Tag: {t}".into(),
            columns: vec!["t".into()],
            rows: vec![vec!["a".into()], vec!["  ".into()], vec!["c".into()]],
            window: None,
            file_name: "x.csv".into(),
        };
        check(&mut b).unwrap();
        assert_eq!(b.name, "Batch");
        assert_eq!(b.window.as_deref(), Some("24h"));
        let (file, sent) = input_file(&b).unwrap();
        assert_eq!(sent, 2);
        let lines: Vec<Value> = file
            .split(|c| *c == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect();
        assert_eq!(lines[0]["custom_id"], "row-1");
        assert_eq!(lines[1]["custom_id"], "row-3");
        assert_eq!(lines[1]["body"]["messages"][0]["content"], "Be brief");
        assert_eq!(lines[1]["body"]["messages"][1]["content"], "Tag: c");
        assert_eq!(row_of("row-3"), Some(2));
        assert_eq!(row_of("other"), None);
    }

    #[test]
    fn bad_batches_are_refused() {
        let base = || NewBatch {
            name: "n".into(),
            model: "m".into(),
            system: String::new(),
            template: "{t}".into(),
            columns: vec!["t".into()],
            rows: vec![vec!["a".into()]],
            window: None,
            file_name: String::new(),
        };
        assert!(check(&mut NewBatch {
            model: "auto".into(),
            ..base()
        })
        .is_err());
        assert!(check(&mut NewBatch {
            template: " ".into(),
            ..base()
        })
        .is_err());
        assert!(check(&mut NewBatch {
            columns: vec!["t".into(), "t".into()],
            ..base()
        })
        .is_err());
        assert!(check(&mut NewBatch {
            rows: vec![],
            ..base()
        })
        .is_err());
        assert!(check(&mut NewBatch {
            window: Some("1h".into()),
            ..base()
        })
        .is_err());
        let mut ragged = NewBatch {
            columns: vec!["t".into(), "u".into()],
            ..base()
        };
        check(&mut ragged).unwrap();
        assert_eq!(ragged.rows[0], vec!["a".to_string(), String::new()]);
    }

    #[test]
    fn outcomes_read_answers_and_errors() {
        let ok = json!({"custom_id": "row-1", "response": {"status_code": 200, "body": {"choices": [{"message": {"content": "Yes"}}]}}, "error": null});
        let o = outcome_of(&ok);
        assert_eq!((o.answer.as_str(), o.status), ("Yes", "done"));
        let bad = json!({"custom_id": "row-2", "response": {"status_code": 400, "body": {"error": {"message": "too long"}}}, "error": null});
        let o = outcome_of(&bad);
        assert_eq!((o.status, o.error.as_str()), ("failed", "too long"));
        let late = json!({"custom_id": "row-3", "response": null, "error": {"code": "batch_expired", "message": "late"}});
        assert_eq!(outcome_of(&late).status, "ran out of time");
        let stopped = json!({"custom_id": "row-4", "response": null, "error": {"code": "cancelled", "message": "cancelled"}});
        assert_eq!(outcome_of(&stopped).status, "cancelled");
    }

    #[test]
    fn csv_fields_are_quoted_and_formulas_disarmed() {
        let mut out = String::new();
        csv_row(
            ["plain", "a,b", "say \"hi\"", "=SUM(A1)", "- item"],
            &mut out,
        );
        assert_eq!(
            out,
            "plain,\"a,b\",\"say \"\"hi\"\"\",\"'=SUM(A1)\",- item\r\n"
        );
        assert_eq!(
            file_name_for("Gene notes / v2"),
            "Gene-notes---v2-results.csv"
        );
        assert_eq!(file_name_for("///"), "batch-results.csv");
    }

    #[test]
    fn rows_survive_the_gzip_round_trip() {
        let rows = vec![vec!["é".to_string(), "2".to_string()]];
        assert_eq!(gunzip_rows(&gzip_rows(&rows).unwrap()).unwrap(), rows);
    }

    #[test]
    fn the_upload_is_a_multipart_form() {
        let body = String::from_utf8(multipart("B", b"{}\n")).unwrap();
        assert!(body.starts_with(
            "--B\r\nContent-Disposition: form-data; name=\"purpose\"\r\n\r\nbatch\r\n--B\r\n"
        ));
        assert!(body.ends_with("{}\n\r\n--B--\r\n"));
    }
}
