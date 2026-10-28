-- Where a Slurm job's output comes back to, for clusters that bring output back: the
-- job uploads the end of its output to this object as it ends. A row goes after 30
-- days, or with its cluster or person, and its object goes with it.
CREATE TABLE slurm_job_outputs (
    connection_id UUID NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    job_id BIGINT NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (connection_id, user_id, job_id)
);
CREATE INDEX slurm_job_outputs_created_idx ON slurm_job_outputs (created_at);
CREATE TRIGGER slurm_job_outputs_trash AFTER DELETE ON slurm_job_outputs
    FOR EACH ROW EXECUTE FUNCTION trash_object();
CREATE TRIGGER slurm_job_outputs_claim AFTER INSERT ON slurm_job_outputs
    FOR EACH ROW EXECUTE FUNCTION claim_object();
