-- One Kubernetes workspace per conversation. Pods and volumes are reconciled from
-- these rows, so deleting a conversation also retires its cluster resources.
CREATE TABLE sandboxes (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slug TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9]{16}$'),
    status TEXT NOT NULL DEFAULT 'starting' CHECK (status IN ('starting', 'running', 'stopped', 'error')),
    pod_ip TEXT CHECK (CHAR_LENGTH(pod_ip) <= 64),
    preview_port INTEGER CHECK (preview_port BETWEEN 1024 AND 65535),
    error_message TEXT CHECK (CHAR_LENGTH(error_message) <= 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX sandboxes_owner ON sandboxes(owner_id);
CREATE INDEX sandboxes_last_used ON sandboxes(last_used_at);

-- Installation-wide random keys shared by every replica.
CREATE TABLE app_secrets (
    name TEXT PRIMARY KEY CHECK (CHAR_LENGTH(name) <= 64),
    value BYTEA NOT NULL CHECK (OCTET_LENGTH(value) BETWEEN 32 AND 256),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Workspace tool steps shown with an assistant response. Only the answer text is
-- replayed as model context; the workspace itself carries the state.
ALTER TABLE messages ADD COLUMN activity JSONB NOT NULL DEFAULT '[]'::jsonb
    CHECK (jsonb_typeof(activity) = 'array' AND jsonb_array_length(activity) <= 200);
