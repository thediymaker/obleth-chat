//! A project's code on the server. Postgres keeps it as a chain of git bundles, each
//! adding the commits since the one before; any replica rebuilds a bare repository in a
//! temporary folder when it needs one. Writes hold the project's row lock, so main only
//! ever moves forward from the value a workspace merged with.

use super::workspace::Author;
use crate::error::ApiError;
use axum::http::StatusCode;
use sqlx::{PgConnection, Row};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

/// The most a single sync may add, and the most a project may hold.
pub const MAX_TRANSFER: usize = 64 * 1024 * 1024;
pub const MAX_PROJECT_BYTES: i64 = 256 * 1024 * 1024;
/// After this many bundles the chain is folded into one.
const COMPACT_AFTER: i64 = 30;
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

pub fn conflict(message: &str) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

fn failed(what: &str, detail: &str) -> ApiError {
    tracing::warn!(what, detail, "git failed");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Could not {what} the project's code"),
    )
}

pub struct Output {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    fn check(self, what: &str) -> Result<Self, ApiError> {
        if self.ok {
            Ok(self)
        } else {
            Err(failed(what, &self.stderr))
        }
    }
}

/// A bare repository in a temporary folder, removed when dropped.
pub struct Repo {
    dir: PathBuf,
    pub main: Option<String>,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Repo {
    pub(crate) async fn empty() -> Result<Self, ApiError> {
        let dir = std::env::temp_dir().join(format!("obleth-code-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| failed("prepare", &e.to_string()))?;
        let repo = Self { dir, main: None };
        repo.git(&["init", "-q", "--bare", "-b", "main", "."])
            .await?
            .check("prepare")?;
        Ok(repo)
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Runs git in the repository with no user or system configuration, no hooks and
    /// no network, only local files (bundles).
    pub async fn git(&self, args: &[&str]) -> Result<Output, ApiError> {
        self.git_env(args, &[]).await
    }

    pub async fn git_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<Output, ApiError> {
        let mut command = Command::new("git");
        command
            .current_dir(&self.dir)
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
            )
            .env("HOME", &self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "protocol.allow=never",
                "-c",
                "protocol.file.allow=always",
                "-c",
                "core.quotepath=off",
            ])
            .args(args)
            .kill_on_drop(true);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
            .await
            .map_err(|_| failed("read", "git timed out"))?
            .map_err(|e| {
                ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    if e.kind() == std::io::ErrorKind::NotFound {
                        "Git is not installed on this server".into()
                    } else {
                        format!("Could not run git: {e}")
                    },
                )
            })?;
        Ok(Output {
            ok: output.status.success(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Adds a bundle's commits; `refspec` names where its tip goes.
    async fn fetch(&self, data: &[u8], refspec: &str) -> Result<(), ApiError> {
        let file = self.dir.join(format!("in-{}.bundle", Uuid::new_v4()));
        tokio::fs::write(&file, data)
            .await
            .map_err(|e| failed("read", &e.to_string()))?;
        let path = file.to_string_lossy().into_owned();
        let result = self
            .git(&[
                "fetch",
                "-q",
                "--no-tags",
                "--no-write-fetch-head",
                &path,
                refspec,
            ])
            .await;
        let _ = tokio::fs::remove_file(&file).await;
        result?.check("read")?;
        Ok(())
    }

    /// A bundle of `revs` (ref names and exclusions, as for git bundle create).
    pub async fn bundle(&self, revs: &[&str]) -> Result<Vec<u8>, ApiError> {
        let file = self.dir.join(format!("out-{}.bundle", Uuid::new_v4()));
        let path = file.to_string_lossy().into_owned();
        let mut args = vec!["bundle", "create", "-q", path.as_str()];
        args.extend_from_slice(revs);
        let result = self.git(&args).await?;
        let data = if result.ok {
            tokio::fs::read(&file)
                .await
                .map_err(|e| failed("package", &e.to_string()))
        } else {
            Err(failed("package", &result.stderr))
        };
        let _ = tokio::fs::remove_file(&file).await;
        data
    }

    /// The project's code as it is now, or an empty repository before the first sync.
    pub async fn load(db: &mut PgConnection, project: Uuid) -> Result<Self, ApiError> {
        let repo = Self::empty().await?;
        let rows =
            sqlx::query("SELECT data FROM project_code_bundles WHERE project_id = $1 ORDER BY seq")
                .bind(project)
                .fetch_all(&mut *db)
                .await?;
        for row in rows {
            let data: Vec<u8> = row.get("data");
            repo.fetch(&data, "+refs/heads/main:refs/heads/main")
                .await?;
        }
        let mut repo = repo;
        repo.main = repo.resolve("refs/heads/main").await?;
        Ok(repo)
    }

    pub async fn resolve(&self, rev: &str) -> Result<Option<String>, ApiError> {
        let out = self
            .git(&["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")])
            .await?;
        Ok(out.ok.then(|| out.text().trim().to_owned()))
    }

    pub async fn count(&self, rev: &str) -> Result<i64, ApiError> {
        let out = self
            .git(&["rev-list", "--count", rev])
            .await?
            .check("read")?;
        Ok(out.text().trim().parse().unwrap_or(0))
    }

    /// Whether `sha` is in main's history.
    pub async fn on_main(&self, sha: &str) -> Result<bool, ApiError> {
        Ok(self.main.is_some()
            && self
                .git(&["merge-base", "--is-ancestor", sha, "main"])
                .await?
                .ok)
    }

    /// A commit on top of main that takes back what `sha` changed, as a bundle for
    /// `advance` (this copy's main moves to it). The later history stays as it is.
    pub async fn revert(&mut self, sha: &str, who: &Author) -> Result<Vec<u8>, ApiError> {
        let old = self.main.clone().ok_or_else(ApiError::not_found)?;
        let parents = self
            .git(&["rev-list", "--parents", "-n", "1", sha])
            .await?
            .check("read")?
            .text();
        let parents: Vec<&str> = parents.split_whitespace().skip(1).collect();
        let parent = match parents.as_slice() {
            [one] => *one,
            [] => {
                return Err(ApiError::bad(
                    "The first commit cannot be undone on its own. Undo later commits instead",
                ))
            }
            _ => return Err(ApiError::bad(
                "This commit combined two lines of work. Undo the commits it brought in instead",
            )),
        };
        let merged = self
            .git(&[
                "merge-tree",
                "--write-tree",
                "--no-messages",
                &format!("--merge-base={sha}"),
                "main",
                parent,
            ])
            .await?;
        if !merged.ok {
            return Err(conflict(
                "Later commits changed the same lines, so this cannot be undone by itself",
            ));
        }
        let tree = merged.text().lines().next().unwrap_or("").trim().to_owned();
        let current = self.resolve_tree("main").await?;
        if tree == current {
            return Err(ApiError::bad("Its changes are already gone from the code"));
        }
        let subject = self
            .git(&["show", "-s", "--format=%s", sha])
            .await?
            .check("read")?
            .text();
        let message = format!(
            "Undo “{}”\n\nThis takes back commit {}.",
            subject.trim(),
            &sha[..sha.len().min(12)]
        );
        let env = [
            ("GIT_AUTHOR_NAME", who.name.as_str()),
            ("GIT_AUTHOR_EMAIL", who.email.as_str()),
            ("GIT_COMMITTER_NAME", who.name.as_str()),
            ("GIT_COMMITTER_EMAIL", who.email.as_str()),
        ];
        let commit = self
            .git_env(&["commit-tree", &tree, "-p", &old, "-m", &message], &env)
            .await?
            .check("save")?
            .text()
            .trim()
            .to_owned();
        self.git(&["update-ref", "refs/heads/main", &commit, &old])
            .await?
            .check("save")?;
        self.main = Some(commit);
        self.bundle(&["HEAD", &format!("^{old}")]).await
    }

    async fn resolve_tree(&self, rev: &str) -> Result<String, ApiError> {
        Ok(self
            .git(&["rev-parse", &format!("{rev}^{{tree}}")])
            .await?
            .check("read")?
            .text()
            .trim()
            .to_owned())
    }
}

/// What a sync from a workspace did to main.
pub enum Advance {
    /// Main is now `main`.
    Moved { main: String, commits: i64 },
    /// Someone synced first: main is `main`, which the workspace has to bring in.
    Stale { main: Option<String> },
}

/// Main as stored, with its commit count; None before the first sync.
pub async fn head(db: &mut PgConnection, project: Uuid) -> Result<Option<(String, i64)>, ApiError> {
    let row: Option<(Option<String>, i32)> =
        sqlx::query_as("SELECT main, commits FROM project_code WHERE project_id = $1")
            .bind(project)
            .fetch_optional(&mut *db)
            .await?;
    Ok(row.and_then(|(main, commits)| main.map(|m| (m, commits as i64))))
}

/// How many commits `tip` had when it was main, if it ever was.
pub async fn commits_at(
    db: &mut PgConnection,
    project: Uuid,
    tip: &str,
) -> Result<Option<i64>, ApiError> {
    let n: Option<i32> = sqlx::query_scalar(
        "SELECT commits FROM project_code_tips WHERE project_id = $1 AND tip = $2",
    )
    .bind(project)
    .bind(tip)
    .fetch_optional(&mut *db)
    .await?;
    Ok(n.map(i64::from))
}

/// Moves main to the tip of `bundle` (a workspace's commits on top of `expected`),
/// when main is still `expected` and the move only adds commits.
pub async fn advance(
    pool: &sqlx::PgPool,
    project: Uuid,
    user: Uuid,
    expected: Option<&str>,
    bundle: &[u8],
) -> Result<Advance, ApiError> {
    if bundle.len() > MAX_TRANSFER {
        return Err(ApiError::bad(
            "These changes are over 64 MB. Leave large data and build output out of the project",
        ));
    }
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO project_code (project_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(project)
        .execute(&mut *tx)
        .await?;
    let (main, bytes): (Option<String>, i64) =
        sqlx::query_as("SELECT main, bytes FROM project_code WHERE project_id = $1 FOR UPDATE")
            .bind(project)
            .fetch_one(&mut *tx)
            .await?;
    if main.as_deref() != expected {
        return Ok(Advance::Stale { main });
    }
    let repo = Repo::load(&mut tx, project).await?;
    repo.fetch(bundle, "+HEAD:refs/heads/incoming").await?;
    let tip = repo
        .resolve("refs/heads/incoming")
        .await?
        .ok_or_else(|| failed("read", "bundle had no HEAD"))?;
    if let Some(old) = &main {
        let forward = repo
            .git(&["merge-base", "--is-ancestor", old, &tip])
            .await?
            .ok;
        if !forward {
            return Err(conflict(
                "These changes do not include the project's latest code. Sync again to bring it in first",
            ));
        }
        if *old == tip {
            return Ok(Advance::Moved {
                main: tip,
                commits: repo.count("refs/heads/main").await?,
            });
        }
    }
    repo.git(&["update-ref", "refs/heads/main", &tip])
        .await?
        .check("save")?;
    let exclude = main.as_ref().map(|old| format!("^{old}"));
    let mut revs = vec!["main"];
    if let Some(exclude) = &exclude {
        revs.push(exclude);
    }
    let data = repo.bundle(&revs).await?;
    if bytes + data.len() as i64 > MAX_PROJECT_BYTES {
        return Err(ApiError::bad(
            "The project's code would pass 256 MB. Leave large data and build output out of it",
        ));
    }
    let commits = repo.count("refs/heads/main").await?;
    let seq: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM project_code_bundles WHERE project_id = $1",
    )
    .bind(project)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO project_code_bundles (project_id, seq, tip, data) VALUES ($1,$2,$3,$4)",
    )
    .bind(project)
    .bind(seq)
    .bind(&tip)
    .bind(&data)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO project_code_tips (project_id, tip, commits) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(project)
        .bind(&tip)
        .bind(commits as i32)
        .execute(&mut *tx)
        .await?;
    let mut total = bytes + data.len() as i64;
    if i64::from(seq) >= COMPACT_AFTER {
        let full = repo.bundle(&["main"]).await?;
        sqlx::query("DELETE FROM project_code_bundles WHERE project_id = $1")
            .bind(project)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO project_code_bundles (project_id, seq, tip, data) VALUES ($1,1,$2,$3)",
        )
        .bind(project)
        .bind(&tip)
        .bind(&full)
        .execute(&mut *tx)
        .await?;
        total = full.len() as i64;
    }
    sqlx::query(
        "UPDATE project_code SET main = $2, commits = $3, bytes = $4, updated_at = NOW(), updated_by = $5 WHERE project_id = $1",
    )
    .bind(project)
    .bind(&tip)
    .bind(commits as i32)
    .bind(total)
    .bind(user)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Advance::Moved { main: tip, commits })
}
