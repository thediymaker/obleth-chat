-- MCP connections: servers a person adds for their own chats, or that an
-- administrator shares with everyone (owner_id NULL). Chat encrypts tokens before
-- storing them and caches each server's tool list.
CREATE TABLE connections (
    id UUID PRIMARY KEY,
    owner_id UUID REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 40),
    url TEXT NOT NULL CHECK (CHAR_LENGTH(url) BETWEEN 1 AND 2048),
    token BYTEA CHECK (OCTET_LENGTH(token) <= 8192),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    confirm_actions BOOLEAN NOT NULL DEFAULT TRUE,
    tools JSONB NOT NULL DEFAULT '[]'::jsonb,
    server_name TEXT CHECK (CHAR_LENGTH(server_name) <= 200),
    checked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX connections_owner_name
    ON connections (COALESCE(owner_id, '00000000-0000-0000-0000-000000000000'::uuid), LOWER(name));

-- People who switched a shared connection off for their own chats.
CREATE TABLE connection_opt_outs (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, connection_id)
);

-- Tool calls waiting for the person's Allow or Deny. The replica running the
-- response writes the row and polls it; any replica can record the decision.
CREATE TABLE tool_approvals (
    generation_id UUID NOT NULL,
    call_id TEXT NOT NULL CHECK (CHAR_LENGTH(call_id) BETWEEN 1 AND 128),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    decision TEXT CHECK (decision IN ('allow', 'deny')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (generation_id, call_id)
);
CREATE INDEX tool_approvals_pending ON tool_approvals (conversation_id, call_id) WHERE decision IS NULL;
