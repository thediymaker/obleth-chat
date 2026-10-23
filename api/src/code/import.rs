//! Starting a project from a repository: Chat fetches its default branch once, with
//! all its history, and makes that the project's main line, so Sync, the Code tab,
//! Undo and research build on it. A history too big to keep comes in as one commit of
//! the latest version. A token for a private repository is used for that fetch and
//! never stored.

use super::repo::{self, conflict, Repo, MAX_TRANSFER};
use super::{author, workspace::Author};
use crate::{agents, connections::mcp::is_public, error::ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::net::IpAddr;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/projects/{id}/code/import", post(import))
}

#[derive(Deserialize)]
struct ImportInput {
    url: String,
    #[serde(default)]
    token: Option<String>,
}

/// Where the code comes from and how git reaches it.
pub struct Source {
    /// What the project shows as its origin.
    pub url: String,
    /// What git fetches; for an https address, the one Chat checked.
    pub fetch: String,
    /// Settings for git, such as a pinned address and an Authorization header,
    /// passed through the environment so they never appear on a command line.
    pub config: Vec<(String, String)>,
    /// Removed from anything git says before it is shown or logged.
    pub secret: Option<String>,
}

#[derive(Debug)]
pub struct Imported {
    pub main: String,
    pub commits: i64,
    /// Whether the repository's history came along, or only its latest version.
    pub history: bool,
}

async fn import(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ImportInput>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    let owned: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM projects WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(user.id)
            .fetch_optional(pool)
            .await?;
    if owned.is_none() {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only the project's owner can start its code from a repository".into(),
        ));
    }
    let token = body
        .token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if token
        .is_some_and(|t| t.len() > 400 || t.chars().any(|c| c.is_whitespace() || c.is_control()))
    {
        return Err(ApiError::bad("That token has spaces or is too long"));
    }
    let source = checked(&body.url, token).await?;
    let done = start(pool, id, user.id, &author(&user), &source).await?;
    Ok(Json(
        json!({"sha":done.main,"commits":done.commits,"history":done.history,"url":source.url}),
    ))
}

/// An https repository address on a public host, pinned to the address Chat checked
/// so git can't be sent anywhere else, with redirects off.
async fn checked(raw: &str, token: Option<&str>) -> Result<Source, ApiError> {
    let (url, _) = agents::repo_url(raw).map_err(ApiError::bad)?;
    let parsed =
        url::Url::parse(&url).map_err(|_| ApiError::bad("That is not a repository address"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| ApiError::bad("That is not a repository address"))?
        .to_owned();
    let port = parsed.port_or_known_default().unwrap_or(443);
    let found: Vec<IpAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| ApiError::bad(format!("Chat could not find {host}")))?
        .map(|a| a.ip())
        .filter(|ip| is_public(*ip))
        .collect();
    let ip = found
        .iter()
        .find(|ip| ip.is_ipv4())
        .or(found.first())
        .copied()
        .ok_or_else(|| ApiError::bad(format!("{host} is not a public address Chat can reach")))?;
    let pinned = match ip {
        IpAddr::V4(v4) => format!("{host}:{port}:{v4}"),
        IpAddr::V6(v6) => format!("{host}:{port}:[{v6}]"),
    };
    let mut config = vec![
        ("http.curloptResolve".to_owned(), pinned),
        ("http.followRedirects".to_owned(), "false".to_owned()),
    ];
    if let Some(token) = token {
        let basic =
            base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
        config.push((
            "http.extraHeader".to_owned(),
            format!("Authorization: Basic {basic}"),
        ));
    }
    Ok(Source {
        fetch: url.clone(),
        url,
        config,
        secret: token.map(str::to_owned),
    })
}

/// Fills an empty project's code from `source`. Its history comes along when it
/// fits; otherwise its latest version becomes one commit by `who`.
pub async fn start(
    pool: &PgPool,
    project: Uuid,
    user: Uuid,
    who: &Author,
    source: &Source,
) -> Result<Imported, ApiError> {
    start_within(pool, project, user, who, source, MAX_TRANSFER).await
}

/// As `start`, keeping the history only while its bundle is under `history_bytes`.
pub async fn start_within(
    pool: &PgPool,
    project: Uuid,
    user: Uuid,
    who: &Author,
    source: &Source,
    history_bytes: usize,
) -> Result<Imported, ApiError> {
    if repo::head(&mut *pool.acquire().await?, project)
        .await?
        .is_some()
    {
        return Err(conflict("This project already has code"));
    }
    let mut env: Vec<(String, String)> =
        vec![("GIT_CONFIG_COUNT".into(), source.config.len().to_string())];
    for (i, (key, value)) in source.config.iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), key.clone()));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), value.clone()));
    }
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let repo = Repo::empty().await?;
    let mut fetched = None;
    // Some hosts serve a repository only at its .git address, and redirects are off.
    let mut tries = vec![source.fetch.clone()];
    if source.fetch.starts_with("https://") && !source.fetch.ends_with(".git") {
        tries.push(format!("{}.git", source.fetch));
    }
    let mut said = String::new();
    for address in &tries {
        let out = repo
            .git_env(
                &[
                    "-c",
                    "protocol.https.allow=always",
                    "fetch",
                    "-q",
                    "--no-tags",
                    "--no-write-fetch-head",
                    address,
                    "+HEAD:refs/heads/main",
                ],
                &env,
            )
            .await?;
        if out.ok {
            fetched = Some(());
            break;
        }
        said = match &source.secret {
            Some(secret) => out.stderr.replace(secret.as_str(), "[token]"),
            None => out.stderr,
        };
    }
    if fetched.is_none() {
        tracing::info!(url = %source.url, said = %said.trim(), "could not fetch a repository");
        let lower = said.to_lowercase();
        return Err(ApiError::bad(
            if lower.contains("authentication")
                || lower.contains("could not read username")
                || lower.contains("403")
                || lower.contains("401")
                || lower.contains("not found")
                || lower.contains("does not appear")
            {
                if source.secret.is_some() {
                    "Chat could not read that repository with the token. Check the address and that the token can read it"
                } else {
                    "Chat could not read that repository. Check the address, or add a token if it is private"
                }
            } else if lower.contains("timed out") {
                "The repository took too long to fetch"
            } else {
                "Chat could not fetch that repository"
            },
        ));
    }
    let mut repo = repo;
    let tip = repo
        .resolve("refs/heads/main")
        .await?
        .ok_or_else(|| ApiError::bad("That repository has no commits yet"))?;
    repo.main = Some(tip.clone());
    let mut history = true;
    let mut bundle = repo.bundle(&["HEAD"]).await?;
    if bundle.len() > history_bytes {
        // Too much history to keep: the latest version, as the project's first commit.
        history = false;
        let message = format!(
            "Start from {}\n\nThe latest version of {} at {}, without its history.",
            source.url,
            source.url,
            &tip[..12]
        );
        let env = [
            ("GIT_AUTHOR_NAME", who.name.as_str()),
            ("GIT_AUTHOR_EMAIL", who.email.as_str()),
            ("GIT_COMMITTER_NAME", who.name.as_str()),
            ("GIT_COMMITTER_EMAIL", who.email.as_str()),
        ];
        let tree = format!("{tip}^{{tree}}");
        let one = repo
            .git_env(&["commit-tree", &tree, "-m", &message], &env)
            .await?;
        if !one.ok {
            return Err(ApiError::internal());
        }
        let one = one.text().trim().to_owned();
        repo.git(&["update-ref", "refs/heads/main", &one]).await?;
        bundle = repo.bundle(&["HEAD"]).await?;
        if bundle.len() > MAX_TRANSFER {
            return Err(ApiError::bad(
                "That repository is over 64 MB even without its history, more than a project keeps",
            ));
        }
    }
    let (main, commits) = match repo::advance(pool, project, user, None, &bundle).await? {
        repo::Advance::Moved { main, commits } => (main, commits),
        repo::Advance::Stale { .. } => return Err(conflict("This project already has code")),
    };
    sqlx::query("UPDATE project_code SET origin_url = $2, origin_sha = $3, origin_history = $4 WHERE project_id = $1")
        .bind(project)
        .bind(&source.url)
        .bind(&tip)
        .bind(history)
        .execute(pool)
        .await?;
    Ok(Imported {
        main,
        commits,
        history,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_public_hosts_pinned_to_the_checked_address() {
        for private in [
            "https://10.1.2.3/a/b",
            "https://127.0.0.1/a/b",
            "https://169.254.169.254/a/b",
        ] {
            let refused = checked(private, None).await.err().expect(private);
            assert!(refused.1.contains("not a public address"), "{}", refused.1);
        }
        let source = checked("https://140.82.112.3/owner/name", Some("secret-token"))
            .await
            .unwrap();
        assert_eq!(source.fetch, "https://140.82.112.3/owner/name");
        assert!(source.config.contains(&(
            "http.curloptResolve".into(),
            "140.82.112.3:443:140.82.112.3".into()
        )));
        assert!(source
            .config
            .contains(&("http.followRedirects".into(), "false".into())));
        assert!(source
            .config
            .iter()
            .any(|(k, v)| k == "http.extraHeader" && v.starts_with("Authorization: Basic ")));
        assert!(!source
            .config
            .iter()
            .any(|(_, v)| v.contains("secret-token")));
    }
}
