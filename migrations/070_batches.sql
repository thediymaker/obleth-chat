-- Batches people start from a spreadsheet: the gateway runs the requests (its
-- /v1/batches), and Chat keeps what it needs to hand the results back as the
-- same spreadsheet with the answers added: the columns, the rows, and the
-- prompt they were turned into.

CREATE TABLE batch_sources (
    -- The gateway's batch id.
    batch_id TEXT PRIMARY KEY CHECK (char_length(batch_id) <= 128),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) <= 200),
    model TEXT NOT NULL CHECK (char_length(model) <= 160),
    file_name TEXT NOT NULL DEFAULT '' CHECK (char_length(file_name) <= 255),
    columns JSONB NOT NULL,
    -- The rows as a gzipped JSON array of arrays, in the columns' order.
    rows BYTEA NOT NULL,
    row_count INTEGER NOT NULL,
    system_prompt TEXT NOT NULL DEFAULT '',
    template TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX batch_sources_user_idx ON batch_sources (user_id, created_at DESC);
