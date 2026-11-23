//! Web pages as notebook sources. A page is read only in its person's own browser (a
//! headless Chromium in their pod, under the sandbox namespace's network rules; see
//! browser/read.rs), never by Chat itself, and kept as a document in a collection of
//! theirs in the project, through the same storage and indexing as an upload. The
//! document keeps the address and when it was read, and can be read again.
//!
//! Three ways in: addresses typed into the notebook's Add menu, read in the background
//! one after another; the page a chat's browser tab shows now, from the Browser tab;
//! and the model's notebook_add_page tool, when the person asks for it in a chat whose
//! browser is on. Every read counts against the project's reads in a day.

use super::{make_project_collection, tick, usable, view};
use crate::browser::read::{Limits, Read};
use crate::browser::Browsers;
use crate::knowledge::extract::Kind;
use crate::{auth::Identity, error::ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use uuid::Uuid;

pub const TOOL: &str = "notebook_add_page";
/// Addresses one Add may give.
pub const PAGES_PER_REQUEST: usize = 10;
/// Pages a project's notebook may read in a day, all its people together.
pub const READS_PER_DAY: i64 = 100;
/// Pages one person may have waiting to be read.
pub const READING_AT_ONCE: i64 = 20;
/// Characters of a page's text a source may hold.
pub const PAGE_CHARS: usize = 500_000;
/// The largest PDF, unless files are smaller on this server.
pub const PDF_BYTES: usize = 32 * 1024 * 1024;
/// Longest one read may take, starting the browser included.
const READ_TIME: Duration = Duration::from_secs(4 * 60);
/// A read taken up longer ago than this stopped with its replica.
const CLAIM: &str = "6 minutes";
/// Tries of a read whose replica stopped, before it is failed.
const TRIES: i16 = 2;
const LONGEST_ADDRESS: usize = 2000;

/// The address to read for what the person typed: http or https, to a public site,
/// without a user name or password. A bare site name gets https.
pub fn address(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Give a web address".into());
    }
    if text.chars().count() > LONGEST_ADDRESS {
        return Err("The address is longer than 2,000 characters".into());
    }
    let with_scheme = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| "It is not a web address".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https addresses can be added".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "Leave the user name and password out of the address, and sign in in the browser instead"
                .into(),
        );
    }
    let public = match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name.contains('.')
                && !name.ends_with(".localhost")
                && !name.ends_with(".local")
                && !name.ends_with(".internal")
        }
        Some(url::Host::Ipv4(ip)) => public_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => public_ip(IpAddr::V6(ip)),
        None => false,
    };
    if !public {
        return Err("Only addresses of public web sites can be added".into());
    }
    let url = url.to_string();
    if url.len() > LONGEST_ADDRESS {
        return Err("The address is longer than 2,000 characters".into());
    }
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => public_v4(v4),
            None => {
                let first = ip.segments()[0];
                !(ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)))
}

/// The site an address is on, for logs: never its path or query.
fn site(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Whether this person may add web pages: knowledge, and the browser on for this server
/// and for them.
pub fn offered(s: &AppState, user: &Identity) -> bool {
    s.knowledge.is_some() && s.browsers.as_ref().is_some_and(|b| b.allowed(user))
}

fn browsers<'a>(s: &'a AppState, user: &Identity) -> Result<&'a Browsers, ApiError> {
    let browsers = s.browsers.as_ref().ok_or_else(|| {
        ApiError::feature_off(
            "Web pages are read in the browser, which is not enabled on this server",
        )
    })?;
    if !browsers.allowed(user) {
        return Err(ApiError::denied(
            "Web pages are read in the browser, which is available to administrators",
        ));
    }
    Ok(browsers)
}

pub(crate) fn limits(s: &AppState) -> Limits {
    Limits {
        text_chars: PAGE_CHARS,
        pdf_bytes: PDF_BYTES.min(s.store.limits.max_file_bytes),
    }
}

/// A page as a document: its file name, kind and bytes. A web page is kept as
/// Markdown, so its headings become the sections it is searched and cited by.
pub(crate) fn document(read: Read) -> (String, Kind, Vec<u8>) {
    let name = |stem: &str| -> String {
        let stem: String = stem
            .chars()
            .map(|c| if matches!(c, '/' | '\\') { '_' } else { c })
            .filter(|c| !c.is_control())
            .take(190)
            .collect();
        let stem = stem.trim().trim_start_matches('.').trim().to_owned();
        if stem.is_empty() {
            "Web page".into()
        } else {
            stem
        }
    };
    match read {
        Read::Page { title, text, .. } => (
            format!("{}.md", name(&title)),
            Kind::Markdown,
            text.into_bytes(),
        ),
        Read::Pdf {
            name: file, bytes, ..
        } => {
            let stem = file
                .strip_suffix(".pdf")
                .or_else(|| file.strip_suffix(".PDF"))
                .unwrap_or(&file)
                .to_owned();
            (format!("{}.pdf", name(&stem)), Kind::Pdf, bytes)
        }
    }
}

/// The collection of the person's in the project that new sources go to; one is made
/// for the project's owner when none is theirs, as Add does for files.
async fn destination(pool: &PgPool, user: Uuid, project: Uuid) -> Result<Uuid, ApiError> {
    usable(pool, user, project).await?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT c.id FROM project_collections pc JOIN knowledge_collections c ON c.id = pc.collection_id
         WHERE pc.project_id = $2 AND c.owner_id = $1 AND c.attachment_id IS NULL ORDER BY c.name, c.id LIMIT 1",
    )
    .bind(user)
    .bind(project)
    .fetch_optional(pool)
    .await?;
    match existing {
        Some(id) => Ok(id),
        None => make_project_collection(pool, user, project).await,
    }
}

/// Refuses `more` reads when the project has read its pages for the day or the person
/// has too many waiting. Run in a transaction holding the project's lock.
async fn room(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: Uuid,
    project: Uuid,
    more: i64,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('notebook-pages:' || $1::text, 0))")
        .bind(project)
        .execute(&mut **tx)
        .await?;
    let (today, waiting): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM notebook_page_reads WHERE project_id = $2 AND requested_at > NOW() - INTERVAL '1 day'),
                (SELECT COUNT(*) FROM notebook_page_reads WHERE owner_id = $1 AND status = 'reading')",
    )
    .bind(owner)
    .bind(project)
    .fetch_one(&mut **tx)
    .await?;
    if today + more > READS_PER_DAY {
        return Err(ApiError::too_many(format!(
            "This project’s notebook has read {today} of its {READS_PER_DAY} pages for the day. Try again tomorrow."
        )));
    }
    if waiting + more > READING_AT_ONCE {
        return Err(ApiError::too_many(format!(
            "You have {waiting} pages waiting to be read. Add more when they are done."
        )));
    }
    Ok(())
}

/// Records reads of `urls` (each with the document it reads again, if any), to be read
/// in the background. Returns their ids.
async fn queue(
    pool: &PgPool,
    owner: Uuid,
    project: Uuid,
    collection: Uuid,
    urls: &[(String, Option<Uuid>)],
) -> Result<Vec<Uuid>, ApiError> {
    let mut tx = pool.begin().await?;
    room(&mut tx, owner, project, urls.len() as i64).await?;
    let mut ids = Vec::with_capacity(urls.len());
    for (n, (url, again)) in urls.iter().enumerate() {
        let id = Uuid::new_v4();
        // A moment apart, so they are read in the order given.
        sqlx::query(
            "INSERT INTO notebook_page_reads (id, owner_id, project_id, collection_id, document_id, again, url, requested_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, NOW() + make_interval(secs => $8))",
        )
        .bind(id)
        .bind(owner)
        .bind(project)
        .bind(collection)
        .bind(again)
        .bind(again.is_some())
        .bind(url)
        .bind(n as f64 / 1000.0)
        .execute(&mut *tx)
        .await?;
        ids.push(id);
    }
    tx.commit().await?;
    Ok(ids)
}

/// Queues the pages at `addresses` for the person's notebook in the project. Nothing
/// is read here: `drain` reads them in the person's browser.
pub async fn add_pages(
    pool: &PgPool,
    user: Uuid,
    project: Uuid,
    addresses: &[String],
) -> Result<Vec<Uuid>, ApiError> {
    let mut urls: Vec<String> = Vec::new();
    for given in addresses.iter().filter(|a| !a.trim().is_empty()) {
        let url = address(given).map_err(|why| {
            let shown: String = given.trim().chars().take(80).collect();
            ApiError::bad(format!("“{shown}”: {why}"))
        })?;
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    if urls.is_empty() {
        return Err(ApiError::bad("Give a web address"));
    }
    if urls.len() > PAGES_PER_REQUEST {
        return Err(ApiError::bad(format!(
            "Add up to {PAGES_PER_REQUEST} pages at a time"
        )));
    }
    let collection = destination(pool, user, project).await?;
    let urls: Vec<(String, Option<Uuid>)> = urls.into_iter().map(|u| (u, None)).collect();
    queue(pool, user, project, collection, &urls).await
}

/// Queues reading a web page's document again, for the owner of its collection.
pub async fn read_again(
    pool: &PgPool,
    user: Uuid,
    project: Uuid,
    document: Uuid,
) -> Result<Uuid, ApiError> {
    usable(pool, user, project).await?;
    let found: Option<(Option<String>, Uuid)> = sqlx::query_as(
        "SELECT d.source_url, d.collection_id FROM knowledge_documents d
         JOIN project_collections pc ON pc.collection_id = d.collection_id AND pc.project_id = $2
         JOIN knowledge_collections c ON c.id = d.collection_id
         WHERE d.id = $3 AND c.owner_id = $1",
    )
    .bind(user)
    .bind(project)
    .bind(document)
    .fetch_optional(pool)
    .await?;
    let (url, collection) = found.ok_or_else(ApiError::not_found)?;
    let url = url.ok_or_else(|| ApiError::bad("Only a web page can be read again"))?;
    let going: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM notebook_page_reads WHERE document_id = $1 AND again AND status = 'reading')",
    )
    .bind(document)
    .fetch_one(pool)
    .await?;
    if going {
        return Err(ApiError::conflict("This page is being read again already"));
    }
    // An earlier failed try is replaced by this one.
    sqlx::query("DELETE FROM notebook_page_reads WHERE document_id = $1 AND again AND owner_id = $2 AND status = 'failed'")
        .bind(document)
        .bind(user)
        .execute(pool)
        .await?;
    Ok(queue(pool, user, project, collection, &[(url, Some(document))]).await?[0])
}

/// Tries a failed read again. Returns its project.
pub async fn retry(pool: &PgPool, user: Uuid, read: Uuid) -> Result<Uuid, ApiError> {
    let project: Uuid = sqlx::query_scalar(
        "SELECT project_id FROM notebook_page_reads WHERE id = $1 AND owner_id = $2 AND status = 'failed'",
    )
    .bind(read)
    .bind(user)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)?;
    usable(pool, user, project).await?;
    let mut tx = pool.begin().await?;
    room(&mut tx, user, project, 1).await?;
    sqlx::query(
        "UPDATE notebook_page_reads SET status = 'reading', error = NULL, attempts = 0, claimed_at = NULL,
             requested_at = NOW(), finished_at = NULL
         WHERE id = $1 AND status = 'failed'",
    )
    .bind(read)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(project)
}

/// Removes a read that failed. Returns its project.
pub async fn remove(pool: &PgPool, user: Uuid, read: Uuid) -> Result<Uuid, ApiError> {
    sqlx::query_scalar(
        "DELETE FROM notebook_page_reads WHERE id = $1 AND owner_id = $2 AND status = 'failed' RETURNING project_id",
    )
    .bind(read)
    .bind(user)
    .fetch_optional(pool)
    .await?
    .ok_or_else(ApiError::not_found)
}

struct Job {
    id: Uuid,
    project: Uuid,
    collection: Uuid,
    document: Option<Uuid>,
    again: bool,
    url: String,
}

/// Takes the person's next page to read, unless one of theirs is being read: their
/// pages are read one after another. Reads a stopped replica left are taken up again,
/// and failed after TRIES.
async fn claim(pool: &PgPool, owner: Uuid) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query(&format!(
        "UPDATE notebook_page_reads SET status = 'failed', claimed_at = NULL, finished_at = NOW(),
             error = 'Reading stopped before it finished. Try again.'
         WHERE owner_id = $1 AND status = 'reading' AND claimed_at < NOW() - INTERVAL '{CLAIM}' AND attempts >= {TRIES}"
    ))
    .bind(owner)
    .execute(pool)
    .await?;
    let row: Option<(Uuid, Uuid, Uuid, Option<Uuid>, bool, String)> = sqlx::query_as(&format!(
        "UPDATE notebook_page_reads SET claimed_at = NOW(), attempts = attempts + 1
         WHERE id = (SELECT id FROM notebook_page_reads
                     WHERE owner_id = $1 AND status = 'reading' AND (claimed_at IS NULL OR claimed_at < NOW() - INTERVAL '{CLAIM}')
                       AND NOT EXISTS (SELECT 1 FROM notebook_page_reads r WHERE r.owner_id = $1 AND r.status = 'reading'
                                       AND r.claimed_at >= NOW() - INTERVAL '{CLAIM}')
                     ORDER BY requested_at, id LIMIT 1 FOR UPDATE SKIP LOCKED)
         RETURNING id, project_id, collection_id, document_id, again, url"
    ))
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.map(|(id, project, collection, document, again, url)| Job {
            id,
            project,
            collection,
            document,
            again,
            url,
        }),
    )
}

/// Reads the person's waiting pages, one after another, until none is left.
pub async fn drain(s: AppState, owner: Uuid) {
    let pool = &s.store.pool;
    loop {
        let job = match claim(pool, owner).await {
            Ok(Some(job)) => job,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!("notebook pages unavailable: {error}");
                return;
            }
        };
        let done = read(&s, owner, &job).await;
        let finished = match &done {
            Ok(document) => {
                sqlx::query(
                    "UPDATE notebook_page_reads SET status = 'done', document_id = $2, error = NULL, finished_at = NOW()
                     WHERE id = $1",
                )
                .bind(job.id)
                .bind(document)
                .execute(pool)
                .await
            }
            Err(why) => {
                tracing::debug!(read = %job.id, site = %site(&job.url), "a notebook page was not read: {why}");
                let why: String = why.chars().take(300).collect();
                sqlx::query(
                    "UPDATE notebook_page_reads SET status = 'failed', error = $2, claimed_at = NULL, finished_at = NOW()
                     WHERE id = $1",
                )
                .bind(job.id)
                .bind(why)
                .execute(pool)
                .await
            }
        };
        if let Err(error) = finished {
            tracing::warn!("could not record a notebook page read: {error}");
            return;
        }
    }
}

/// Reads one page in the person's browser and keeps it. Errors are for the person.
async fn read(s: &AppState, owner: Uuid, job: &Job) -> Result<Uuid, String> {
    let pool = &s.store.pool;
    let knowledge = s
        .knowledge
        .as_ref()
        .ok_or("Knowledge is not enabled on this server.")?;
    let user = crate::background::identity_of(pool, owner)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Your account is turned off.")?;
    let browsers = browsers(s, &user).map_err(|e| e.1)?;
    // Still a collection of theirs in a project they may use.
    usable(pool, owner, job.project)
        .await
        .map_err(|_| "The project is gone, or no longer shared with you.".to_owned())?;
    let fits: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_collections pc JOIN knowledge_collections c ON c.id = pc.collection_id
                       WHERE pc.project_id = $1 AND c.id = $2 AND c.owner_id = $3)",
    )
    .bind(job.project)
    .bind(job.collection)
    .bind(owner)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if !fits {
        return Err("The collection it was going to is no longer in the project.".into());
    }
    if job.again && job.document.is_none() {
        return Err("The source was removed.".into());
    }
    let read = tokio::time::timeout(READ_TIME, browsers.read_address(owner, &job.url, limits(s)))
        .await
        .map_err(|_| "Reading the page took longer than four minutes.".to_owned())??;
    let (name, kind, bytes) = document(read);
    let files = &s.store.files;
    match job.document.filter(|_| job.again) {
        Some(document) => {
            knowledge
                .replace_page(files, owner, document, &name, kind, bytes)
                .await
                .map_err(|e| e.1)?;
            Ok(document)
        }
        None => {
            let document = knowledge
                .store_page(files, owner, job.collection, &name, kind, bytes, &job.url)
                .await
                .map_err(|e| e.1)?;
            if let Err(error) = tick(pool, owner, job.project, &[document], true).await {
                tracing::warn!("could not tick an added page: {}", error.1);
            }
            Ok(document)
        }
    }
}

/// Starts reading the waiting pages of people whose reads nobody is reading: queued
/// on a replica that stopped, or left by one. Clears reads done two days ago.
pub async fn recover(s: &AppState) -> Result<(), sqlx::Error> {
    let pool = &s.store.pool;
    // Done reads count for a day; a failed reading again of a source since removed
    // has nothing left to show on.
    sqlx::query(
        "DELETE FROM notebook_page_reads WHERE (status = 'done' AND requested_at < NOW() - INTERVAL '2 days')
             OR (again AND document_id IS NULL AND status = 'failed')",
    )
    .execute(pool)
    .await?;
    let owners: Vec<Uuid> = sqlx::query_scalar(&format!(
        "SELECT DISTINCT owner_id FROM notebook_page_reads
         WHERE status = 'reading' AND requested_at < NOW() - INTERVAL '30 seconds'
           AND (claimed_at IS NULL OR claimed_at < NOW() - INTERVAL '{CLAIM}')
         LIMIT 50"
    ))
    .fetch_all(pool)
    .await?;
    for owner in owners {
        tokio::spawn(drain(s.clone(), owner));
    }
    Ok(())
}

/// A page added from a chat's browser tab.
#[derive(Debug)]
pub struct Added {
    pub document: Uuid,
    pub name: String,
    pub url: String,
}

/// Keeps the page a chat's tab shows now, as it shows it, in the person's notebook in
/// the project. Only the person's own browser and only that chat's tab are read.
pub async fn add_from_tab(
    s: &AppState,
    user: &Identity,
    conversation: Uuid,
    project: Uuid,
) -> Result<Added, ApiError> {
    let pool = &s.store.pool;
    let knowledge = s
        .knowledge
        .as_ref()
        .ok_or_else(|| ApiError::feature_off("Knowledge is not configured"))?;
    let browsers = browsers(s, user)?;
    if !browsers.is_on(user.id, conversation).await {
        return Err(ApiError::conflict("The browser is off in this chat"));
    }
    let collection = destination(pool, user.id, project).await?;
    {
        let mut tx = pool.begin().await?;
        room(&mut tx, user.id, project, 1).await?;
    }
    let read = tokio::time::timeout(
        READ_TIME,
        browsers.read_tab(user.id, conversation, limits(s)),
    )
    .await
    .map_err(|_| ApiError::upstream("Reading the page took longer than four minutes"))?
    .map_err(ApiError::bad)?;
    let url = match &read {
        Read::Page { url, .. } | Read::Pdf { url, .. } => url.clone(),
    };
    let url = address(&url).map_err(ApiError::bad)?;
    let (name, kind, bytes) = document(read);
    let document = knowledge
        .store_page(
            &s.store.files,
            user.id,
            collection,
            &name,
            kind,
            bytes,
            &url,
        )
        .await?;
    // Counted with the project's reads for the day.
    sqlx::query(
        "INSERT INTO notebook_page_reads (id, owner_id, project_id, collection_id, document_id, url, status, finished_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'done', NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(project)
    .bind(collection)
    .bind(document)
    .bind(&url)
    .execute(pool)
    .await?;
    if let Err(error) = tick(pool, user.id, project, &[document], true).await {
        tracing::warn!("could not tick an added page: {}", error.1);
    }
    Ok(Added {
        document,
        name,
        url,
    })
}

// ---- The model's tool ----

pub fn definition() -> Value {
    json!({"type":"function","function":{"name":TOOL,
        "description":"Add the page open in this chat's browser tab to this project's notebook as a source, as the page shows now (a PDF is added as the file). Call it only when the person asks for it, such as \"add this page to the notebook\" or \"keep it as a source\"; never on your own. Open the page with browser_open first.",
        "parameters":{"type":"object","properties":{},"additionalProperties":false}}})
}

/// Runs the tool for the chat's own tab: whether it worked, a short summary, and what
/// the model reads. It never sends the page's text back.
pub async fn run_tool(
    s: &AppState,
    user: &Identity,
    conversation: Uuid,
    project: Uuid,
) -> (bool, String, String) {
    let held = match &s.browsers {
        Some(browsers) => browsers.get_tab(user.id, conversation).await.ok().flatten(),
        None => None,
    };
    if held.is_some_and(|t| t.controller == "user") {
        return (false, "Page not added".into(), "Error: The person has control of the browser right now. Wait for them to hand it back.".into());
    }
    match add_from_tab(s, user, conversation, project).await {
        Ok(added) => {
            let title = added
                .name
                .strip_suffix(".md")
                .unwrap_or(&added.name)
                .to_owned();
            // The page's own words, its title included, reach the model only through
            // the notebook's sources, quoted as data.
            (
                true,
                format!("Added “{}” to the notebook", short(&title, 80)),
                format!("Added the page ({}) to this project’s notebook as a source, ticked for the person. It is indexed in the background and can be asked about in the notebook once it is ready.", site(&added.url)),
            )
        }
        Err(error) => (
            false,
            "Page not added".into(),
            format!("Error: {}", error.1),
        ),
    }
}

fn short(text: &str, n: usize) -> String {
    if text.chars().count() > n {
        format!("{}…", text.chars().take(n).collect::<String>())
    } else {
        text.to_owned()
    }
}

// ---- HTTP ----

fn knowledge_on(s: &AppState) -> Result<(), ApiError> {
    super::knowledge_on(s)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Addresses {
    addresses: Vec<String>,
}

pub(super) async fn add(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Addresses>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    knowledge_on(&s)?;
    browsers(&s, &user)?;
    if body.addresses.len() > 100 {
        return Err(ApiError::bad(format!(
            "Add up to {PAGES_PER_REQUEST} pages at a time"
        )));
    }
    add_pages(&s.store.pool, user.id, id, &body.addresses).await?;
    tokio::spawn(drain(s.clone(), user.id));
    Ok((StatusCode::ACCEPTED, Json(view(&s, &user, id).await?)))
}

pub(super) async fn again(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, document)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    knowledge_on(&s)?;
    browsers(&s, &user)?;
    read_again(&s.store.pool, user.id, id, document).await?;
    tokio::spawn(drain(s.clone(), user.id));
    Ok((StatusCode::ACCEPTED, Json(view(&s, &user, id).await?)))
}

pub(super) async fn try_again(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    knowledge_on(&s)?;
    browsers(&s, &user)?;
    let project = retry(&s.store.pool, user.id, id).await?;
    tokio::spawn(drain(s.clone(), user.id));
    Ok((StatusCode::ACCEPTED, Json(view(&s, &user, project).await?)))
}

pub(super) async fn forget(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    remove(&s.store.pool, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub(super) struct FromTab {
    project_id: Option<Uuid>,
}

/// "Add this page to a notebook" beside a chat: the page its tab shows now, into the
/// chat's project or the one the person chose.
pub(super) async fn from_tab(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<FromTab>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let user = s.auth.require(&headers, true).await?;
    knowledge_on(&s)?;
    let project = match body.project_id {
        Some(project) => project,
        None => crate::projects::for_conversation(&s.store.pool, user.id, id)
            .await
            .map(|p| p.id)
            .ok_or_else(|| ApiError::bad("Choose the project whose notebook the page goes to"))?,
    };
    let added = add_from_tab(&s, &user, id, project).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"projectId":project,"documentId":added.document,"name":added.name})),
    ))
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
