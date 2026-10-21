-- One workspace pod and volume per user; each conversation's workspace becomes a
-- project folder in it with its own preview host and dev server port. Existing
-- per-conversation workspaces are dropped (the reconciler then removes their pods
-- and volumes, which carry the old obleth.io/sandbox label).
DELETE FROM sandboxes;

CREATE TABLE sandbox_pods (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    slug TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9]{16}$'),
    status TEXT NOT NULL DEFAULT 'starting' CHECK (status IN ('starting', 'running', 'stopped', 'error')),
    pod_ip TEXT CHECK (CHAR_LENGTH(pod_ip) <= 64),
    error_message TEXT CHECK (CHAR_LENGTH(error_message) <= 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX sandbox_pods_last_used ON sandbox_pods(last_used_at);

ALTER TABLE sandboxes
    DROP COLUMN status,
    DROP COLUMN pod_ip,
    DROP COLUMN error_message,
    ADD COLUMN pod_id UUID NOT NULL REFERENCES sandbox_pods(id) ON DELETE CASCADE,
    ADD COLUMN port INTEGER NOT NULL CHECK (port BETWEEN 3000 AND 3999),
    ADD CONSTRAINT sandboxes_pod_port UNIQUE (pod_id, port);
