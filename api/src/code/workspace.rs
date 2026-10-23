//! The workspace's side of a project's code: a git repository in the chat's folder.
//! Code moves between it and the server as bundles over the workspace exec channel,
//! so a workspace needs no network access or credentials. The project's main, as the
//! workspace last saw it, is refs/remotes/project/main.

use crate::error::ApiError;
use crate::sandbox::{tools, Sandbox, Sandboxes};
use axum::http::StatusCode;
use serde::Serialize;
use std::time::Duration;

use super::repo::MAX_TRANSFER;

const TIMEOUT: Duration = Duration::from_secs(90);
pub const BASE: &str = "refs/remotes/project/main";

/// Folders never committed, kept out without touching the project's own .gitignore.
const EXCLUDE: &str = "node_modules/\n.next/\ndist/\nbuild/\n.venv/\nvenv/\n__pycache__/\n.cache/\n.turbo/\n.DS_Store\n*.pyc\n";

/// Runs in the chat's folder. $1 and $2 are the author's name and email; the rest
/// are the operation's own arguments, from $1 after the shift.
const PRELUDE: &str = r#"OB_NAME=$1 OB_EMAIL=$2; shift 2
export GIT_TERMINAL_PROMPT=0 GIT_EDITOR=true GIT_MERGE_AUTOEDIT=no LC_ALL=C
g() { command git -c core.quotepath=off -c advice.detachedHead=false -c init.defaultBranch=main -c user.name="$OB_NAME" -c user.email="$OB_EMAIL" "$@"; }
"#;

pub struct Author {
    pub name: String,
    pub email: String,
}

fn gateway(error: String) -> ApiError {
    ApiError(StatusCode::BAD_GATEWAY, error)
}

/// What a script printed, and how it ended.
pub struct Raw {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: Option<i32>,
    pub truncated: bool,
}

/// Somewhere to run scripts in a chat's folder: its workspace pod, or a local folder
/// in tests. `args` are $1… and output past `limit` bytes is cut.
pub trait Shell: Sync {
    fn exec(
        &self,
        script: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
        limit: usize,
    ) -> impl std::future::Future<Output = Result<Raw, String>> + Send;
}

/// A chat's workspace pod.
pub struct Pod<'a> {
    pub sandboxes: &'a Sandboxes,
    pub sandbox: &'a Sandbox,
}

impl Shell for Pod<'_> {
    async fn exec(
        &self,
        script: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
        limit: usize,
    ) -> Result<Raw, String> {
        let script = tools::in_project(self.sandboxes, self.sandbox, script);
        let mut argv = vec!["sh", "-c", script.as_str(), "sh"];
        argv.extend_from_slice(args);
        let output = tokio::time::timeout(
            TIMEOUT,
            self.sandboxes.exec_with(self.sandbox, &argv, stdin, limit),
        )
        .await
        .map_err(|_| "The command did not finish in time".to_string())?
        .map_err(|error| format!("Could not run the command in the workspace: {error:#}"))?;
        Ok(Raw {
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.exit_code,
            truncated: output.truncated,
        })
    }
}

const TEXT_LIMIT: usize = 4 * 1024 * 1024;

async fn run(
    sh: &impl Shell,
    author: &Author,
    script: &str,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> Result<(String, String, Option<i32>), ApiError> {
    let script = format!("{PRELUDE}{script}");
    let mut all = vec![author.name.as_str(), author.email.as_str()];
    all.extend_from_slice(args);
    let raw = sh
        .exec(&script, &all, stdin, TEXT_LIMIT)
        .await
        .map_err(gateway)?;
    Ok((
        String::from_utf8_lossy(&raw.stdout).into_owned(),
        raw.stderr,
        raw.code,
    ))
}

/// Makes the chat's folder a git repository if it is not one yet. repos/ holds other
/// people's repositories checked out to read, which stay out of the project.
const ENSURE: &str = r#"if [ ! -d .git ]; then g init -q || exit 10; fi
mkdir -p .git/info && touch .git/info/exclude
grep -qx 'node_modules/' .git/info/exclude 2>/dev/null || printf '%s' "$OB_EXCLUDE" >> .git/info/exclude
grep -qx '/repos/' .git/info/exclude 2>/dev/null || echo '/repos/' >> .git/info/exclude
"#;

fn ensure() -> String {
    format!("OB_EXCLUDE='{EXCLUDE}'\n{ENSURE}")
}

#[derive(Serialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub path: String,
    /// added, modified, deleted, or conflict.
    pub status: String,
    pub added: Option<i64>,
    pub removed: Option<i64>,
}

#[derive(Serialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub time: i64,
    pub subject: String,
    pub chat: Option<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub repo: bool,
    pub head: Option<String>,
    pub base: Option<String>,
    pub merging: bool,
    pub ahead: i64,
    pub changes: Vec<Change>,
    pub log: Vec<Commit>,
}

const STATUS: &str = r#"[ -d .git ] || { echo norepo; exit 0; }
head=$(g rev-parse -q --verify HEAD 2>/dev/null)
base=$(g rev-parse -q --verify refs/remotes/project/main 2>/dev/null)
echo "head $head"; echo "base $base"
if [ -f .git/MERGE_HEAD ]; then echo merging; else g add -A -N . >/dev/null 2>&1; fi
if [ -n "$head" ]; then
  if [ -n "$base" ]; then echo "ahead $(g rev-list --count "$base..HEAD")"; else echo "ahead $(g rev-list --count HEAD)"; fi
fi
printf '@status\n'; g status --porcelain=v1 -z --untracked-files=all --no-renames; printf '\n'
printf '@numstat\n'; if [ -n "$head" ]; then g diff --numstat -z --no-renames HEAD; else g diff --numstat -z --no-renames; fi; printf '\n'
printf '@log\n'; [ -n "$head" ] && g log -n 20 --format='%H%x1f%an%x1f%at%x1f%s%x1f%(trailers:key=Obleth-Chat,valueonly,separator=)%x1e' HEAD
printf '\n@end\n'
"#;

fn section<'a>(out: &'a str, name: &str) -> &'a str {
    let marker = format!("@{name}\n");
    let Some(start) = out.find(&marker) else {
        return "";
    };
    let rest = &out[start + marker.len()..];
    let end = rest.find("\n@").unwrap_or(rest.len());
    &rest[..end]
}

pub fn parse_status(out: &str) -> Status {
    if out.trim() == "norepo" {
        return Status::default();
    }
    let mut status = Status {
        repo: true,
        ..Default::default()
    };
    for line in out.lines() {
        if let Some(v) = line.strip_prefix("head ") {
            status.head = Some(v.trim().to_owned()).filter(|v| !v.is_empty());
        } else if let Some(v) = line.strip_prefix("base ") {
            status.base = Some(v.trim().to_owned()).filter(|v| !v.is_empty());
        } else if let Some(v) = line.strip_prefix("ahead ") {
            status.ahead = v.trim().parse().unwrap_or(0);
        } else if line == "merging" {
            status.merging = true;
        } else if line.starts_with('@') {
            break;
        }
    }
    let mut changes: Vec<Change> = Vec::new();
    for entry in section(out, "status").split('\0') {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        let code = code.trim_end();
        let state = match code {
            "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU" => "conflict",
            c if c.contains('D') => "deleted",
            "??" => "added",
            c if c.contains('A') => "added",
            _ => "modified",
        };
        changes.push(Change {
            path: path.to_owned(),
            status: state.into(),
            ..Default::default()
        });
    }
    for entry in section(out, "numstat").split('\0') {
        let mut parts = entry.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = path.trim_start_matches('\n');
        if let Some(change) = changes.iter_mut().find(|c| c.path == path) {
            change.added = a.parse().ok();
            change.removed = r.parse().ok();
        }
    }
    changes
        .sort_by(|a, b| (a.status != "conflict", &a.path).cmp(&(b.status != "conflict", &b.path)));
    status.changes = changes;
    for record in section(out, "log").split('\x1e') {
        let fields: Vec<&str> = record.trim_matches('\n').split('\x1f').collect();
        if fields.len() < 4 || fields[0].len() != 40 {
            continue;
        }
        status.log.push(Commit {
            sha: fields[0].to_owned(),
            author: fields[1].to_owned(),
            time: fields[2].parse().unwrap_or(0),
            subject: fields[3].to_owned(),
            chat: fields
                .get(4)
                .map(|c| c.trim().to_owned())
                .filter(|c| !c.is_empty()),
        });
    }
    status
}

pub async fn status(sh: &impl Shell, author: &Author) -> Result<Status, ApiError> {
    let (out, err, code) = run(sh, author, STATUS, &[], None).await?;
    if code != Some(0) {
        return Err(gateway(format!(
            "Could not read the workspace's changes: {}",
            err.trim()
        )));
    }
    Ok(parse_status(&out))
}

/// Commits everything changed, when anything is. Returns whether it made a commit.
pub async fn commit(sh: &impl Shell, author: &Author, message: &str) -> Result<bool, ApiError> {
    let script = format!(
        "{}\n[ -f .git/MERGE_HEAD ] && exit 12\ng add -A || exit 11\nif g diff --cached --quiet 2>/dev/null; then echo nothing; exit 0; fi\ng commit -q -m \"$1\" || exit 13\necho committed",
        ensure()
    );
    let (out, err, code) = run(sh, author, &script, &[message], None).await?;
    match code {
        Some(0) => Ok(out.contains("committed")),
        Some(12) => Err(super::repo::conflict(
            "Finish or stop the sync in progress first",
        )),
        _ => Err(gateway(format!(
            "Could not save the changes: {}",
            err.trim()
        ))),
    }
}

/// Puts the project's main into the workspace as refs/remotes/project/main.
pub async fn receive(sh: &impl Shell, author: &Author, bundle: &[u8]) -> Result<(), ApiError> {
    let script = format!(
        "{}\nf=$(mktemp) && cat > \"$f\" && g fetch -q --no-tags \"$f\" '+refs/heads/main:{BASE}'; c=$?; rm -f \"$f\"; exit $c",
        ensure()
    );
    let (_, err, code) = run(sh, author, &script, &[], Some(bundle)).await?;
    if code != Some(0) {
        return Err(gateway(format!(
            "Could not bring the project's code into the workspace: {}",
            err.trim()
        )));
    }
    Ok(())
}

/// Fills a new, empty chat folder with the project's code at `rev` (in `bundle`'s
/// history), as if it had synced there. Returns false when the folder already had
/// files or a repository, which are left alone.
pub async fn start(
    sh: &impl Shell,
    author: &Author,
    bundle: &[u8],
    rev: &str,
) -> Result<bool, ApiError> {
    let script = format!(
        "[ -e .git ] && exit 0\n[ -z \"$(ls -A 2>/dev/null)\" ] || exit 0\n{}\nf=$(mktemp) && cat > \"$f\" && g fetch -q --no-tags \"$f\" '+refs/heads/main:{BASE}'; c=$?; rm -f \"$f\"; [ $c = 0 ] || exit $c\ng update-ref {BASE} \"$1\" && g checkout -q -B main \"$1\" || exit 14\necho started",
        ensure()
    );
    let (out, err, code) = run(sh, author, &script, &[rev], Some(bundle)).await?;
    if code != Some(0) {
        return Err(gateway(format!(
            "Could not put the project's code in the workspace: {}",
            err.trim()
        )));
    }
    Ok(out.contains("started"))
}

/// Takes back the workspace's last commit when it has not been synced: its changes
/// are unsaved again, as they were before it.
pub async fn uncommit(sh: &impl Shell, author: &Author) -> Result<(), ApiError> {
    let script = format!(
        r#"[ -d .git ] || exit 16
[ -f .git/MERGE_HEAD ] && exit 12
g rev-parse -q --verify HEAD >/dev/null || exit 16
b=$(g rev-parse -q --verify {BASE})
if [ -n "$b" ] && g merge-base --is-ancestor HEAD "$b"; then exit 17; fi
[ "$(g rev-list --parents -n 1 HEAD | wc -w)" -gt 2 ] && exit 18
if g rev-parse -q --verify HEAD^ >/dev/null; then g reset -q --soft HEAD^ || exit 13; else g update-ref -d HEAD || exit 13; fi"#
    );
    let (_, err, code) = run(sh, author, &script, &[], None).await?;
    match code {
        Some(0) => Ok(()),
        Some(12) => Err(super::repo::conflict(
            "Finish or stop the sync in progress first",
        )),
        Some(16) => Err(ApiError::bad("Nothing has been saved yet")),
        Some(17) => Err(super::repo::conflict(
            "That save is already in the project. Undo it from the project's history instead",
        )),
        Some(18) => Err(ApiError::bad(
            "The last save combined the project's changes, so it cannot be taken back here",
        )),
        _ => Err(gateway(format!("Could not undo the save: {}", err.trim()))),
    }
}

/// Moves the workspace's view of main, after a sync stored it.
pub async fn mark(sh: &impl Shell, author: &Author, sha: &str) -> Result<(), ApiError> {
    let (_, err, code) = run(
        sh,
        author,
        &format!("g update-ref {BASE} \"$1\""),
        &[sha],
        None,
    )
    .await?;
    if code != Some(0) {
        return Err(gateway(format!(
            "Could not record the sync: {}",
            err.trim()
        )));
    }
    Ok(())
}

/// Drops the workspace's view of main, when it came from another project.
pub async fn forget(sh: &impl Shell, author: &Author) -> Result<(), ApiError> {
    let (_, err, code) = run(
        sh,
        author,
        &format!("[ -d .git ] || exit 0\ng update-ref -d {BASE} 2>/dev/null; exit 0"),
        &[],
        None,
    )
    .await?;
    if code != Some(0) {
        return Err(gateway(format!("Could not reset the sync: {}", err.trim())));
    }
    Ok(())
}

pub enum Merge {
    /// Nothing to bring in, or it combined cleanly.
    Clean,
    /// These files changed on both sides.
    Conflicts(Vec<String>),
}

/// Brings main into the workspace's work: a fast-forward when the workspace has
/// nothing of its own, otherwise a merge commit.
pub async fn merge(sh: &impl Shell, author: &Author, message: &str) -> Result<Merge, ApiError> {
    let script = format!(
        r#"g rev-parse -q --verify {BASE} >/dev/null || exit 0
if ! g rev-parse -q --verify HEAD >/dev/null; then
  g checkout -q -B main {BASE} && exit 0; exit 14
fi
g merge-base --is-ancestor {BASE} HEAD && exit 0
if g merge-base --is-ancestor HEAD {BASE}; then g merge -q --ff-only {BASE} && exit 0; exit 14; fi
g merge -q --no-ff --allow-unrelated-histories -m "$1" {BASE} >/dev/null 2>&1 && exit 0
[ -f .git/MERGE_HEAD ] || exit 14
g diff --name-only -z --diff-filter=U; exit 20"#
    );
    let (out, err, code) = run(sh, author, &script, &[message], None).await?;
    match code {
        Some(0) => Ok(Merge::Clean),
        Some(20) => Ok(Merge::Conflicts(
            out.split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
        )),
        _ => Err(gateway(format!(
            "Could not combine with the project's code: {}",
            err.trim()
        ))),
    }
}

/// The workspace's commits that main does not have, as a bundle; None when there are none.
pub async fn send(sh: &impl Shell) -> Result<Option<Vec<u8>>, ApiError> {
    let script = format!(
        "g() {{ command git \"$@\"; }}\nh=$(g rev-parse -q --verify HEAD) || exit 0\nb=$(g rev-parse -q --verify {BASE})\n[ \"$h\" = \"$b\" ] && exit 0\nif [ -n \"$b\" ]; then g bundle create -q - HEAD \"^$b\"; else g bundle create -q - HEAD; fi"
    );
    let output = sh
        .exec(&script, &[], None, MAX_TRANSFER + 1)
        .await
        .map_err(gateway)?;
    if output.code != Some(0) {
        return Err(gateway(format!(
            "Could not package the changes: {}",
            output.stderr.trim()
        )));
    }
    if output.truncated || output.stdout.len() > MAX_TRANSFER {
        return Err(ApiError::bad(
            "These changes are over 64 MB. Leave large data and build output out of the project",
        ));
    }
    Ok((!output.stdout.is_empty()).then_some(output.stdout))
}

/// Finishes a paused sync once every conflict is settled: the merge commit.
pub async fn conclude(sh: &impl Shell, author: &Author) -> Result<(), ApiError> {
    // Files that were in conflict must not still hold both versions.
    let script = r#"[ -f .git/MERGE_HEAD ] || exit 0
left=""
for f in $(g diff --name-only --diff-filter=U); do
  if [ -f "$f" ] && grep -qE '^(<<<<<<<|>>>>>>>)( |$)' -- "$f"; then left="$left $f"; fi
done
[ -n "$left" ] && { printf '%s' "$left"; exit 21; }
g add -A || exit 11
g commit -q --no-edit --cleanup=strip || exit 13"#;
    let (out, err, code) = run(sh, author, script, &[], None).await?;
    match code {
        Some(0) => Ok(()),
        Some(21) => Err(super::repo::conflict(&format!(
            "Some files still have both versions in them: {}",
            out.split_whitespace().collect::<Vec<_>>().join(", ")
        ))),
        _ => Err(gateway(format!(
            "Could not finish combining: {}",
            err.trim()
        ))),
    }
}

/// Stops a paused sync and puts every file back as it was before it.
pub async fn abort(sh: &impl Shell, author: &Author) -> Result<(), ApiError> {
    let (_, err, code) = run(
        sh,
        author,
        "[ -f .git/MERGE_HEAD ] || exit 0\ng merge --abort",
        &[],
        None,
    )
    .await?;
    if code != Some(0) {
        return Err(gateway(format!("Could not stop the sync: {}", err.trim())));
    }
    Ok(())
}

/// Settles one conflicted file with one side: "ours" (this chat) or "theirs" (the project).
pub async fn choose(
    sh: &impl Shell,
    author: &Author,
    path: &str,
    side: &str,
) -> Result<(), ApiError> {
    let flag = match side {
        "ours" => "--ours",
        "theirs" => "--theirs",
        _ => return Err(ApiError::bad("Choose ours or theirs")),
    };
    // A side that deleted the file keeps it deleted.
    let script = r#"[ -f .git/MERGE_HEAD ] || exit 15
stage=2; [ "$1" = --theirs ] && stage=3
if g cat-file -e ":$stage:$2" 2>/dev/null; then g checkout -q "$1" -- "$2" && g add -- "$2"; else g rm -q --cached -- "$2" >/dev/null; rm -f -- "$2"; fi"#;
    let (_, err, code) = run(sh, author, script, &[flag, path], None).await?;
    match code {
        Some(0) => Ok(()),
        Some(15) => Err(super::repo::conflict("There is no sync in progress")),
        _ => Err(gateway(format!(
            "Could not keep that version: {}",
            err.trim()
        ))),
    }
}

/// One file's changes since the last commit (or, mid-sync, its conflict), as a unified diff.
pub async fn diff(sh: &impl Shell, author: &Author, path: &str) -> Result<String, ApiError> {
    let script = r#"[ -d .git ] || exit 0
if [ -f .git/MERGE_HEAD ] && g diff --name-only --diff-filter=U -- "$1" | grep -q .; then g diff -- "$1"; exit 0; fi
g add -N -- "$1" 2>/dev/null
if g rev-parse -q --verify HEAD >/dev/null; then g diff --no-color --no-renames HEAD -- "$1"; else g diff --no-color -- "$1"; fi"#;
    let (out, err, code) = run(sh, author, script, &[path], None).await?;
    if code != Some(0) {
        return Err(gateway(format!(
            "Could not read the changes: {}",
            err.trim()
        )));
    }
    Ok(out)
}

/// Puts files back as of the last commit: one path, or everything when `path` is None.
pub async fn undo(sh: &impl Shell, author: &Author, path: Option<&str>) -> Result<(), ApiError> {
    let script = r#"[ -d .git ] || exit 0
[ -f .git/MERGE_HEAD ] && exit 12
if ! g rev-parse -q --verify HEAD >/dev/null; then exit 16; fi
if [ -n "$1" ]; then
  if g cat-file -e "HEAD:$1" 2>/dev/null; then g restore -q --source=HEAD --staged --worktree -- "$1"; else g rm -q --cached -f -- "$1" >/dev/null 2>&1; rm -f -- "$1"; fi
else
  g reset -q --hard HEAD && g clean -q -fd
fi"#;
    let (_, err, code) = run(sh, author, script, &[path.unwrap_or("")], None).await?;
    match code {
        Some(0) => Ok(()),
        Some(12) => Err(super::repo::conflict(
            "Finish or stop the sync in progress first",
        )),
        Some(16) => Err(ApiError::bad(
            "Nothing has been saved yet, so there is nothing to go back to",
        )),
        _ => Err(gateway(format!("Could not undo: {}", err.trim()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_status_report() {
        let out = "head 1111111111111111111111111111111111111111\nbase \nahead 2\n@status\n M plots/a.py\0?? new.txt\0UU both.py\0 D gone.md\0\n@numstat\n3\t1\tplots/a.py\x005\t0\tnew.txt\0\n@log\n2222222222222222222222222222222222222222\x1fJen\x1f1700000000\x1fAdd it\x1fabc\x1e\n\n@end\n";
        let s = parse_status(out);
        assert!(s.repo && !s.merging);
        assert_eq!(s.ahead, 2);
        assert_eq!(s.base, None);
        assert_eq!(s.changes[0].path, "both.py");
        assert_eq!(s.changes[0].status, "conflict");
        let a = s.changes.iter().find(|c| c.path == "plots/a.py").unwrap();
        assert_eq!(
            (a.status.as_str(), a.added, a.removed),
            ("modified", Some(3), Some(1))
        );
        assert_eq!(
            s.changes
                .iter()
                .find(|c| c.path == "new.txt")
                .unwrap()
                .status,
            "added"
        );
        assert_eq!(
            s.changes
                .iter()
                .find(|c| c.path == "gone.md")
                .unwrap()
                .status,
            "deleted"
        );
        assert_eq!(s.log[0].chat.as_deref(), Some("abc"));
        assert!(!parse_status("norepo\n").repo);
    }
}
