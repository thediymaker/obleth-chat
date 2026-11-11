-- Secrets: named values a person saves for the model to use without ever
-- seeing them, such as a device's console password. Values are sealed with the
-- installation's secret key and bound to the row; only the owner's own browser
-- can read one back, when a tool it carries out needs it.
CREATE TABLE secrets (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (name ~ '^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$'),
    value BYTEA NOT NULL CHECK (OCTET_LENGTH(value) <= 8192),
    note TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(note) <= 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (owner_id, name)
);
