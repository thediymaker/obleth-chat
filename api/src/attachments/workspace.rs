//! A chat's attached files in its workspace. Before a turn, each file sent in the
//! conversation, of any kind, is copied to uploads/<name> in the project folder
//! unless a file of that name and size is already there. Archives stay packed: the
//! model unpacks what it needs. Names and contents only ever reach the workspace as
//! arguments and input, never as part of a script.

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use crate::{code::workspace::Shell, files::Files};

/// The folder in the project that holds attached files.
pub const FOLDER: &str = "uploads";
/// Files listed per conversation; more than this is not a chat's attachments.
const MAX_FILES: i64 = 1000;
/// Longest name kept in the workspace, in bytes, within what file systems allow.
const MAX_NAME_BYTES: usize = 200;

/// Lists the folder's files as `size/name` lines; names hold no `/` or newline.
const LIST: &str = "mkdir -p uploads && find uploads -maxdepth 1 -type f -printf '%s/%f\\n'";
/// Writes standard input to uploads/$1, whole or not at all.
const WRITE: &str = "part=\"uploads/.$1.part\"; mkdir -p uploads && cat > \"$part\" && mv -f -- \"$part\" \"uploads/$1\" || { rm -f -- \"$part\"; exit 1; }";

/// Where a conversation's files go in its workspace.
#[derive(Default)]
pub struct Uploads {
    /// Each attachment's path in the project folder, such as `uploads/site.zip`.
    pub paths: HashMap<Uuid, String>,
    /// One attachment per name: its id, its name in the folder and its size.
    copies: Vec<(Uuid, String, i64)>,
}

impl Uploads {
    /// The conversation's files in the order they were sent. The same file sent again
    /// (same name and bytes) has one copy; another file with a name already taken is
    /// numbered (`data-2.csv`), so names never change once given.
    pub async fn of(pool: &PgPool, owner: Uuid, conversation: Uuid) -> Result<Self, sqlx::Error> {
        let rows: Vec<(Uuid, String, i32, Option<Vec<u8>>)> = sqlx::query_as(
            "SELECT a.id,a.name,a.size,a.sha256 FROM attachments a JOIN message_attachments ma ON ma.attachment_id=a.id JOIN messages m ON m.id=ma.message_id JOIN conversations c ON c.id=m.conversation_id WHERE c.id=$2 AND c.owner_id=$1 AND a.owner_id=$1 GROUP BY a.id ORDER BY MIN(m.created_at),MIN(ma.position),a.id LIMIT $3",
        )
        .bind(owner)
        .bind(conversation)
        .bind(MAX_FILES)
        .fetch_all(pool)
        .await?;
        Ok(Self::named(rows))
    }

    fn named(rows: Vec<(Uuid, String, i32, Option<Vec<u8>>)>) -> Self {
        let mut uploads = Self::default();
        // Names given so far, with the file each holds.
        let mut taken: HashMap<String, (String, i32, Option<Vec<u8>>)> = HashMap::new();
        for (id, name, size, sha) in rows {
            let base = safe_name(&name);
            let mut n = 1;
            let chosen = loop {
                let candidate = if n == 1 {
                    base.clone()
                } else {
                    numbered(&base, n)
                };
                match taken.get(&candidate) {
                    Some(held) if *held == (name.clone(), size, sha.clone()) && sha.is_some() => {
                        break candidate
                    }
                    Some(_) => n += 1,
                    None => {
                        taken.insert(candidate.clone(), (name.clone(), size, sha.clone()));
                        uploads
                            .copies
                            .push((id, candidate.clone(), i64::from(size)));
                        break candidate;
                    }
                }
            };
            uploads.paths.insert(id, format!("{FOLDER}/{chosen}"));
        }
        uploads
    }

    /// The path of each file's copy, for the model.
    pub fn path(&self, id: Uuid) -> Option<&str> {
        self.paths.get(&id).map(String::as_str)
    }

    /// Copies into the workspace each file not already there at its size, reading one
    /// at a time from storage within the server's memory budget. Returns how many were
    /// copied; a file that fails is left for the next turn.
    pub async fn copy(
        &self,
        sh: &impl Shell,
        pool: &PgPool,
        files: &Files,
    ) -> Result<usize, String> {
        if self.copies.is_empty() {
            return Ok(0);
        }
        let listed = sh.exec(LIST, &[], None, 1024 * 1024).await?;
        if listed.code != Some(0) {
            return Err(format!(
                "could not list {FOLDER}/: {}",
                listed.stderr.trim()
            ));
        }
        let there: HashMap<String, i64> = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .filter_map(|line| {
                let (size, name) = line.split_once('/')?;
                Some((name.to_owned(), size.parse().ok()?))
            })
            .collect();
        let mut copied = 0;
        for (id, name, size) in &self.copies {
            if there.get(name) == Some(size) {
                continue;
            }
            let _held = super::hold(*size as usize).await.map_err(|error| error.1)?;
            let row: Option<(Option<Vec<u8>>, Option<String>)> =
                sqlx::query_as("SELECT data,object_key FROM attachments WHERE id=$1")
                    .bind(id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|error| error.to_string())?;
            let Some((data, key)) = row else { continue };
            let bytes = match files.read(data, key, super::MAX_FILE_BYTES).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!("could not read a file for a workspace: {}", error.1);
                    continue;
                }
            };
            match sh.exec(WRITE, &[name], Some(&bytes), 64 * 1024).await {
                Ok(out) if out.code == Some(0) => copied += 1,
                Ok(out) => tracing::warn!(
                    "could not copy a file into a workspace: {}",
                    out.stderr.trim()
                ),
                Err(error) => tracing::warn!("could not copy a file into a workspace: {error}"),
            }
        }
        Ok(copied)
    }
}

/// A name that is one plain file name: no path separators, control characters, or
/// `.`/`..`, and short enough for any file system.
fn safe_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() || clean.chars().all(|c| c == '.') {
        return "file".into();
    }
    if clean.len() <= MAX_NAME_BYTES {
        return clean.to_owned();
    }
    // Keep the extension and cut the rest.
    let (stem, extension) = split(clean);
    let extension = if extension.len() > 20 { "" } else { extension };
    let mut end = MAX_NAME_BYTES - extension.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{extension}", &stem[..end])
}

/// `data.csv` as `data-2.csv`, `site.tar.gz` as `site-2.tar.gz`.
fn numbered(name: &str, n: usize) -> String {
    let (stem, extension) = split(name);
    format!("{stem}-{n}{extension}")
}

/// A name's stem and its extension, with its dot; a name starting with its only dot
/// (`.env`) has none.
fn split(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for double in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"] {
        if lower.ends_with(double) && name.len() > double.len() {
            return name.split_at(name.len() - double.len());
        }
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_and_collisions_numbered_predictably() {
        assert_eq!(safe_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(safe_name("a\nb\\c.txt"), "a_b_c.txt");
        assert_eq!(safe_name(".."), "file");
        assert_eq!(safe_name("  "), "file");
        let long = format!("{}.csv", "é".repeat(150));
        let short = safe_name(&long);
        assert!(short.len() <= MAX_NAME_BYTES && short.ends_with(".csv"));
        assert_eq!(numbered("data.csv", 2), "data-2.csv");
        assert_eq!(numbered("site.tar.gz", 3), "site-3.tar.gz");
        assert_eq!(numbered(".env", 2), ".env-2");
        let (a, b, c, d) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let uploads = Uploads::named(vec![
            (a, "data.csv".into(), 10, Some(vec![1])),
            (b, "data.csv".into(), 12, Some(vec![2])),
            (c, "data.csv".into(), 10, Some(vec![1])),
            (d, "site/index.html".into(), 5, None),
        ]);
        assert_eq!(uploads.path(a), Some("uploads/data.csv"));
        assert_eq!(uploads.path(b), Some("uploads/data-2.csv"));
        // The same file sent again shares the first copy.
        assert_eq!(uploads.path(c), Some("uploads/data.csv"));
        assert_eq!(uploads.path(d), Some("uploads/site_index.html"));
        assert_eq!(uploads.copies.len(), 3);
    }
}
