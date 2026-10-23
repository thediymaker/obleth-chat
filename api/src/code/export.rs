//! Taking a project's code out: a zip (the code, optionally its history as a git bundle
//! and the person's chats as Markdown), or a push to a GitHub repository, created when
//! it does not exist yet. The GitHub token is used for that push and never stored.

use super::history::visible;
use super::repo::{conflict, Repo};
use crate::{error::ApiError, AppState};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{id}/code/export.zip", get(zip))
        .route("/api/projects/{id}/code/github", post(github))
}

/// A folder and file name from the project's name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 60 {
            break;
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

#[derive(Deserialize)]
struct ZipQuery {
    history: Option<bool>,
    chats: Option<bool>,
}

/// A chat as Markdown: what was asked and answered, without tool steps.
fn markdown(c: &crate::store::Conversation) -> String {
    let mut out = format!("# {}\n\n", c.title);
    for m in &c.messages {
        let who = if m.role == "user" {
            "You".to_string()
        } else {
            match m
                .settings
                .as_ref()
                .and_then(|s| s.get("resolvedModel").or_else(|| s.get("model")))
                .and_then(Value::as_str)
            {
                Some(model) if model != "auto" => format!("Assistant · {model}"),
                _ => "Assistant".into(),
            }
        };
        out.push_str(&format!("## {who}\n\n{}\n\n", m.content.trim()));
        if !m.attachments.is_empty() {
            let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
            out.push_str(&format!("_Attached: {}_\n\n", names.join(", ")));
        }
    }
    out
}

async fn zip(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<ZipQuery>,
) -> Result<Response, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    let pool = &s.store.pool;
    let name = visible(pool, user.id, id).await?;
    let repo = {
        let mut db = pool.acquire().await?;
        Repo::load(&mut db, id).await?
    };
    if repo.main.is_none() {
        return Err(ApiError::bad(
            "There is no code to export yet. Sync from a chat first",
        ));
    }
    let folder = slug(&name);
    let extra = repo.path().join(format!("export-{}", Uuid::new_v4()));
    let chats_dir = extra.join("chats");
    tokio::fs::create_dir_all(&chats_dir)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut args: Vec<String> = vec!["archive".into(), "--format=zip".into()];
    if q.chats.unwrap_or(false) {
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM conversations WHERE project_id = $1 AND owner_id = $2 ORDER BY created_at LIMIT 100",
        )
        .bind(id)
        .bind(user.id)
        .fetch_all(pool)
        .await?;
        let mut files = Vec::new();
        for (n, chat) in ids.iter().enumerate() {
            let Ok(c) = s.store.get(user.id, *chat).await else {
                continue;
            };
            let file = chats_dir.join(format!("{:02}-{}.md", n + 1, slug(&c.title)));
            tokio::fs::write(&file, markdown(&c))
                .await
                .map_err(|_| ApiError::internal())?;
            files.push(file);
        }
        if !files.is_empty() {
            args.push(format!("--prefix={folder}/chats/"));
            args.extend(files.iter().map(|f| format!("--add-file={}", f.display())));
        }
    }
    if q.history.unwrap_or(false) {
        let bundle = repo.bundle(&["HEAD", "main"]).await?;
        let file = extra.join(format!("{folder}.bundle"));
        tokio::fs::write(&file, bundle)
            .await
            .map_err(|_| ApiError::internal())?;
        let readme = extra.join("HISTORY.txt");
        tokio::fs::write(
            &readme,
            format!("The full history is in {folder}.bundle. To work with it:\n\n    git clone {folder}.bundle {folder}\n"),
        )
        .await
        .map_err(|_| ApiError::internal())?;
        args.push(format!("--prefix={folder}/"));
        args.push(format!("--add-file={}", file.display()));
        args.push(format!("--add-file={}", readme.display()));
    }
    args.push(format!("--prefix={folder}/"));
    args.push("main".into());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = repo.git(&refs).await?;
    if !out.ok {
        tracing::warn!(stderr = %out.stderr, "git archive failed");
        return Err(ApiError::internal());
    }
    let mut response = Response::new(Body::from(out.stdout));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{folder}.zip\""))
            .map_err(|_| ApiError::internal())?,
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubInput {
    owner: Option<String>,
    name: String,
    private: Option<bool>,
    token: String,
}

fn valid_name(name: &str, max: usize) -> bool {
    !name.is_empty()
        && name.len() <= max
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn github_error(message: &str) -> ApiError {
    ApiError(StatusCode::BAD_GATEWAY, message.into())
}

/// Where GitHub is; tests point these at stand-ins.
fn github_api() -> String {
    std::env::var("OBLETH_CHAT_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into())
}
fn github_git() -> String {
    std::env::var("OBLETH_CHAT_GITHUB_GIT").unwrap_or_else(|_| "https://github.com".into())
}

async fn github(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<GithubInput>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let pool = &s.store.pool;
    let project_name = visible(pool, user.id, id).await?;
    let token = body.token.trim();
    if token.len() < 10
        || token.len() > 400
        || token.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(ApiError::bad("Paste a GitHub token"));
    }
    if !valid_name(&body.name, 100) {
        return Err(ApiError::bad(
            "Repository names use letters, numbers, - _ and .",
        ));
    }
    if let Some(owner) = &body.owner {
        if !valid_name(owner, 39) {
            return Err(ApiError::bad(
                "That is not a GitHub account or organization",
            ));
        }
    }
    let repo = {
        let mut db = pool.acquire().await?;
        Repo::load(&mut db, id).await?
    };
    let Some(main) = repo.main.clone() else {
        return Err(ApiError::bad(
            "There is no code to export yet. Sync from a chat first",
        ));
    };
    let api = github_api();
    let client = reqwest::Client::builder()
        .user_agent("obleth-chat")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| ApiError::internal())?;
    let get = |url: String| {
        client
            .get(url)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .send()
    };
    let me = get(format!("{api}/user"))
        .await
        .map_err(|_| github_error("Could not reach GitHub"))?;
    if me.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ApiError::bad("GitHub did not accept that token"));
    }
    let me: Value = me
        .json()
        .await
        .map_err(|_| github_error("GitHub sent an unexpected answer"))?;
    let login = me["login"]
        .as_str()
        .ok_or_else(|| github_error("GitHub sent an unexpected answer"))?
        .to_owned();
    let owner = body.owner.clone().unwrap_or_else(|| login.clone());
    let existing = get(format!("{api}/repos/{owner}/{}", body.name))
        .await
        .map_err(|_| github_error("Could not reach GitHub"))?;
    let html_url = if existing.status().is_success() {
        let v: Value = existing.json().await.unwrap_or_default();
        v["html_url"].as_str().map(str::to_owned)
    } else if existing.status() == reqwest::StatusCode::NOT_FOUND {
        let create = if owner.eq_ignore_ascii_case(&login) {
            format!("{api}/user/repos")
        } else {
            format!("{api}/orgs/{owner}/repos")
        };
        let made = client
            .post(create)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .json(&json!({"name":body.name,"private":body.private.unwrap_or(true),"description":project_name}))
            .send()
            .await
            .map_err(|_| github_error("Could not reach GitHub"))?;
        if !made.status().is_success() {
            let status = made.status();
            let detail: Value = made.json().await.unwrap_or_default();
            tracing::info!(%status, detail = %detail, "GitHub refused to create a repository");
            return Err(match status.as_u16() {
                401 | 403 => ApiError::bad("That token cannot create repositories there. Give it the repository permission"),
                404 => ApiError::bad("GitHub has no account or organization by that name"),
                422 => ApiError::bad("GitHub would not create a repository with that name"),
                _ => github_error("GitHub could not create the repository"),
            });
        }
        let v: Value = made.json().await.unwrap_or_default();
        v["html_url"].as_str().map(str::to_owned)
    } else {
        return Err(github_error("GitHub could not look up that repository"));
    };
    let url = html_url.unwrap_or_else(|| format!("https://github.com/{owner}/{}", body.name));
    let remote = format!("{}/{owner}/{}.git", github_git(), body.name);
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    let header = format!("Authorization: Basic {basic}");
    let pushed = repo
        .git_env(
            &[
                "-c",
                "protocol.https.allow=always",
                "push",
                "--quiet",
                &remote,
                "main:refs/heads/main",
            ],
            &[
                ("GIT_CONFIG_COUNT", "1"),
                ("GIT_CONFIG_KEY_0", "http.extraHeader"),
                ("GIT_CONFIG_VALUE_0", &header),
            ],
        )
        .await?;
    if !pushed.ok {
        let stderr = pushed.stderr.to_lowercase();
        tracing::info!(stderr = %pushed.stderr.replace(token, "[token]"), "push to GitHub failed");
        return Err(
            if stderr.contains("rejected")
                || stderr.contains("fetch first")
                || stderr.contains("non-fast-forward")
            {
                conflict("That repository has other history. Choose a new name, or one this project was pushed to before")
            } else if stderr.contains("403")
                || stderr.contains("permission")
                || stderr.contains("denied")
            {
                ApiError::bad("That token cannot push there. Give it write access to the repository's contents")
            } else {
                github_error("Could not push to GitHub")
            },
        );
    }
    let target = format!("{owner}/{}", body.name);
    sqlx::query("INSERT INTO project_code_exports (id, project_id, user_id, kind, target, url, tip) VALUES ($1,$2,$3,'github',$4,$5,$6)")
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(user.id)
        .bind(&target)
        .bind(&url)
        .bind(&main)
        .execute(pool)
        .await?;
    Ok(Json(json!({"url":url,"target":target})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_for_files_and_repositories() {
        assert_eq!(slug("Gaudi benchmarks!"), "gaudi-benchmarks");
        assert_eq!(slug("  ¿¿  "), "project");
        assert_eq!(slug("PHYS 241 · Engines"), "phys-241-engines");
        assert!(valid_name("gaudi-benchmarks", 100));
        assert!(!valid_name("../x", 100));
        assert!(!valid_name("a b", 100));
        assert!(!valid_name("..", 100));
    }
}
