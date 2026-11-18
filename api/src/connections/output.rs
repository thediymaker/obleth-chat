//! Job output a Slurm cluster brings back. slurmrestd can't serve files, so on a
//! cluster set to bring output back, each job Chat submits gets a link that can upload
//! one object to Chat's object storage, and uploads the end of its output there as it
//! ends. Job output reads it. The compute nodes must reach the storage.
//!
//! Also the one place a connection result is cut to what the model reads: [`head`]
//! for most results, [`tail`] for log-like ones, where the newest lines matter.
use super::slurm;
use crate::knowledge::ObjectStore;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// How long a job's upload link works: the most S3 allows, so a job that waits in
/// the queue and runs for days can still send its output.
const UPLOAD_SECONDS: u64 = 7 * 24 * 3600;
/// How much of the output the model reads at once.
const SHOWN: usize = 40 * 1024;
/// How much of any connection result the model reads.
pub(super) const MODEL_OUTPUT: usize = 48 * 1024;
/// How much of it the chat shows as the step's detail.
pub(super) const DETAIL_OUTPUT: usize = 4 * 1024;
/// A cut tail skips a partial first line when the next line starts within this.
const LINE_SKIP: usize = 1024;

/// The first `limit` bytes of `text`, whole characters only, and how much follows.
pub(super) fn head(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} more bytes)", &text[..end], text.len() - end)
}

/// The last `limit` bytes of `text`, from a line start when one is near, and how
/// much came before: for logs and device output, where the newest lines matter.
pub(super) fn tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let kept = last(text, limit);
    format!("… ({} earlier bytes)\n{kept}", text.len() - kept.len())
}

/// The last `limit` bytes of `text`, whole characters only, from the next line
/// start when one is near.
fn last(text: &str, limit: usize) -> &str {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let kept = &text[start..];
    match kept.find('\n') {
        Some(n) if n < LINE_SKIP && n + 1 < kept.len() => &kept[n + 1..],
        _ => kept,
    }
}

pub(super) struct Outputs {
    pub pool: PgPool,
    pub store: ObjectStore,
    pub user: Uuid,
}

impl Outputs {
    /// Submits the job so its output comes back, and records where it will land.
    pub async fn submit(
        &self,
        client: &mut slurm::Client,
        connection: Uuid,
        args: &Value,
    ) -> Result<(bool, String), String> {
        if !slurm::output_followed(args) {
            let (text, _) = client.submit(args, None).await?;
            return Ok((true, format!("{text} Its output will not come back to Chat: Chat follows an output file name only when it is a plain path whose patterns are %j, %x, %u, %A, %a or %N.")));
        }
        let key = format!(
            "job-output/{}/{connection}/{}",
            self.user,
            Uuid::new_v4().simple()
        );
        // Recorded as pending first, so an object no row claims is cleaned up.
        sqlx::query(
            "INSERT INTO object_trash (object_key, pending) VALUES ($1, TRUE) ON CONFLICT DO NOTHING",
        )
        .bind(&key)
        .execute(&self.pool)
        .await
        .map_err(|_| "Chat could not prepare the job's output; try again".to_string())?;
        let upload = self.store.presign_put(&key, UPLOAD_SECONDS);
        let (text, id) = client.submit(args, Some(&upload)).await?;
        let recorded = sqlx::query(
            "INSERT INTO slurm_job_outputs (connection_id, user_id, job_id, object_key) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(connection)
        .bind(self.user)
        .bind(id)
        .bind(&key)
        .execute(&self.pool)
        .await
        .is_ok_and(|r| r.rows_affected() == 1);
        Ok((
            true,
            if recorded {
                format!("{text} When it ends, read what it printed with Job output.")
            } else {
                format!("{text} Its output will not come back to Chat.")
            },
        ))
    }

    /// The end of what the job printed, or why there is none yet.
    pub async fn read(
        &self,
        client: &mut slurm::Client,
        connection: Uuid,
        args: &Value,
    ) -> Result<(bool, String), String> {
        let id = slurm::job_id(args)?;
        let key: Option<String> = sqlx::query_scalar(
            "SELECT object_key FROM slurm_job_outputs WHERE connection_id = $1 AND user_id = $2 AND job_id = $3",
        )
        .bind(connection)
        .bind(self.user)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| "Chat could not look up the job's output; try again".to_string())?;
        let Some(key) = key else {
            return Ok((false, format!("Chat has no output for job {id}. Only jobs submitted from Chat while this cluster brings output back send it, and Chat keeps it for 30 days.")));
        };
        let bytes = self
            .store
            .get_optional(&key, slurm::RETURNED_OUTPUT + 1024)
            .await
            .map_err(|_| "Chat's file storage could not be read; try again shortly".to_string())?;
        let Some(bytes) = bytes else {
            let state = client.state_of(id).await.unwrap_or_default();
            return Ok((
                false,
                if state.contains("PENDING") || state.contains("RUNNING") || state.is_empty() {
                    format!(
                        "Job {id} has not ended yet{}. Its output comes back when it ends.",
                        if state.is_empty() {
                            String::new()
                        } else {
                            format!(" ({state})")
                        }
                    )
                } else {
                    format!("Job {id} ended ({state}) but its output never arrived. The compute node may not reach Chat's file storage, or the job was killed before it could send it.")
                },
            ));
        };
        Ok((true, shown(id, &bytes)))
    }
}

/// The last `SHOWN` bytes of the output, from a line start when one is near.
fn shown(id: i64, bytes: &[u8]) -> String {
    let all = String::from_utf8_lossy(bytes);
    if all.trim().is_empty() {
        return format!("Job {id} printed nothing.");
    }
    if all.len() <= SHOWN {
        return format!("Output of job {id}:\n{all}");
    }
    let tail = last(&all, SHOWN);
    format!(
        "Output of job {id}, the last {} KB of {} KB:\n{tail}",
        tail.len() / 1024,
        bytes.len().div_ceil(1024)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_tail_keep_whole_characters_and_say_what_was_cut() {
        assert_eq!(head("short", 10), "short");
        assert_eq!(tail("short", 10), "short");
        let cut = head("ééééé", 3);
        assert!(cut.starts_with("é… (8 more bytes)"), "{cut}");
        let cut = tail("ééééé", 3);
        assert!(cut.ends_with("é"), "{cut}");
        assert!(cut.starts_with("… (8 earlier bytes)\n"), "{cut}");
    }

    #[test]
    fn a_cut_tail_starts_at_a_line_and_keeps_the_last_line() {
        // 200 log lines of 300 bytes: 60 KB, past what the model reads.
        let lines: Vec<String> = (0..200)
            .map(|n| format!("{n:03} {}", "x".repeat(296)))
            .collect();
        let logs = lines.join("\n");
        let cut = tail(&logs, MODEL_OUTPUT);
        assert!(cut.len() <= MODEL_OUTPUT + 64, "{}", cut.len());
        assert!(cut.ends_with(&lines[199]), "the last line is gone");
        let body = cut.split_once('\n').unwrap().1;
        assert!(
            body.starts_with(|c: char| c.is_ascii_digit()),
            "starts mid-line: {:?}",
            &body[..12]
        );
        assert!(lines.contains(&body.split('\n').next().unwrap().to_string()));
    }

    #[test]
    fn long_output_shows_its_end_from_a_line_start() {
        assert_eq!(shown(5, b"  \n"), "Job 5 printed nothing.");
        assert_eq!(shown(5, b"ok\n"), "Output of job 5:\nok\n");
        let long = format!("{}\nlast line\n", "x".repeat(SHOWN * 2));
        let out = shown(5, long.as_bytes());
        assert!(out.ends_with("\nlast line\n"), "{out}");
        assert!(out.starts_with("Output of job 5, the last "), "{out}");
    }
}
