-- Personal API tokens for the MCP endpoint. A token is shown once and stored only
-- as a hash; it expires within 30 days and only reads or runs its owner's chats.
CREATE TABLE api_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE,
    -- The first characters after the obc_ marker, so a person can tell tokens apart.
    prefix TEXT NOT NULL,
    scopes TEXT[] NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    -- Calls in the current one-minute window, shared by replicas.
    window_start TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    window_calls INTEGER NOT NULL DEFAULT 0,
    CHECK (scopes <@ ARRAY['chats:read','chats:write']::TEXT[] AND cardinality(scopes) > 0)
);
CREATE INDEX api_tokens_user_id_idx ON api_tokens(user_id);
