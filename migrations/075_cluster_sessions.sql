-- A file session on a Slurm cluster: a small job Chat submits as the person, on
-- demand, which connects out to Chat and carries out the chat's file operations
-- under the roots the cluster allows, until it has been idle for a while or the job
-- ends. One open session per person and cluster; a new one starts when it is gone.
CREATE TABLE cluster_sessions (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The Slurm job, once submitted.
    job_id BIGINT,
    -- SHA-256 of the token the job presents; the token itself is only in the job's
    -- environment.
    token_hash BYTEA NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('starting', 'live', 'ended')),
    -- The Slurm user the job must run as; the job's hello must name the same one.
    username TEXT NOT NULL,
    host TEXT,
    agent_version INTEGER,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ,
    ends_at TIMESTAMPTZ NOT NULL
);
CREATE UNIQUE INDEX cluster_sessions_open ON cluster_sessions (connection_id, user_id) WHERE state <> 'ended';

-- File operations a chat sends to a session. The replica running the response writes
-- the row and polls it; the job claims it, carries it out and posts the result.
CREATE TABLE cluster_calls (
    id UUID PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES cluster_sessions(id) ON DELETE CASCADE,
    generation_id UUID NOT NULL,
    op TEXT NOT NULL,
    arguments JSONB NOT NULL DEFAULT '{}'::jsonb,
    claimed_at TIMESTAMPTZ,
    ok BOOLEAN,
    output TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX cluster_calls_pending ON cluster_calls (session_id, created_at) WHERE ok IS NULL;
