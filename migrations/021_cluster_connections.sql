-- Slurm and Kubernetes connections beside MCP servers. options holds a cluster's CA
-- certificate and, for a person's own connection, their Slurm user or namespace.
ALTER TABLE connections
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'mcp' CHECK (kind IN ('mcp', 'slurm', 'kubernetes')),
    ADD COLUMN options JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (OCTET_LENGTH(options::text) <= 32768);

-- A person's own credentials for a shared Slurm or Kubernetes connection: the token
-- (encrypted by Chat) and their Slurm user or namespace.
CREATE TABLE connection_credentials (
    connection_id UUID NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token BYTEA NOT NULL CHECK (OCTET_LENGTH(token) <= 16384),
    options JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (OCTET_LENGTH(options::text) <= 4096),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (connection_id, user_id)
);
