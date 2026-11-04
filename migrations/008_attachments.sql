-- Private files remain in PostgreSQL so every application replica sees the same data.
CREATE TABLE attachments (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 200),
    mime_type TEXT NOT NULL CHECK (CHAR_LENGTH(mime_type) BETWEEN 1 AND 100),
    kind TEXT NOT NULL CHECK (kind IN ('text', 'image')),
    size INTEGER NOT NULL CHECK (size > 0 AND size <= 5242880),
    data BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (OCTET_LENGTH(data) = size),
    CHECK (kind <> 'text' OR size <= 262144)
);
CREATE INDEX attachments_owner_created_idx ON attachments(owner_id, created_at DESC, id);

CREATE TABLE message_attachments (
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    attachment_id UUID NOT NULL REFERENCES attachments(id) DEFERRABLE INITIALLY DEFERRED,
    position SMALLINT NOT NULL CHECK (position >= 0 AND position < 4),
    PRIMARY KEY(message_id, attachment_id),
    UNIQUE(message_id, position)
);
CREATE INDEX message_attachments_attachment_idx ON message_attachments(attachment_id);
