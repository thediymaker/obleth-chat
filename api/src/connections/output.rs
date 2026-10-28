//! Job output a Slurm cluster brings back. slurmrestd can't serve files, so on a
//! cluster set to bring output back, each job Chat submits gets a link that can upload
//! one object to Chat's object storage, and uploads the end of its output there as it
//! ends. Job output reads it. The compute nodes must reach the storage.
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
    let mut start = all.len() - SHOWN;
    while !all.is_char_boundary(start) {
        start += 1;
    }
    let tail = &all[start..];
    let tail = tail.find('\n').map_or(tail, |n| &tail[n + 1..]);
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
    fn long_output_shows_its_end_from_a_line_start() {
        assert_eq!(shown(5, b"  \n"), "Job 5 printed nothing.");
        assert_eq!(shown(5, b"ok\n"), "Output of job 5:\nok\n");
        let long = format!("{}\nlast line\n", "x".repeat(SHOWN * 2));
        let out = shown(5, long.as_bytes());
        assert!(out.ends_with("\nlast line\n"), "{out}");
        assert!(out.starts_with("Output of job 5, the last "), "{out}");
    }
}
