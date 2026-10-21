CREATE TABLE conversations (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (CHAR_LENGTH(title) BETWEEN 1 AND 160),
    model TEXT NOT NULL CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    active_generation UUID,
    generation_expires TIMESTAMPTZ,
    CHECK ((active_generation IS NULL) = (generation_expires IS NULL))
);
CREATE INDEX conversations_owner_updated_idx ON conversations(owner_id, updated_at DESC, id);
CREATE INDEX conversations_active_owner_idx ON conversations(owner_id, generation_expires)
    WHERE active_generation IS NOT NULL;

CREATE TABLE messages (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0 AND ordinal < 200),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL CHECK (OCTET_LENGTH(content) <= 1048576),
    status TEXT NOT NULL CHECK (status IN ('pending', 'completed', 'interrupted', 'error')),
    generation_id UUID UNIQUE,
    settings JSONB,
    usage JSONB,
    request_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (conversation_id, ordinal),
    CHECK ((role = 'assistant') = (generation_id IS NOT NULL)),
    CHECK (role <> 'user' OR status = 'completed')
);
-- In addition to the distributed lease, enforce one unfinished assistant per chat.
CREATE UNIQUE INDEX messages_one_pending_idx ON messages(conversation_id) WHERE status = 'pending';
