-- Usage insights: what kind of work chats are, without keeping what was said. Once a
-- chat has been quiet for a while, Chat asks a model about it and keeps only labels and,
-- for most chats, one generic line naming the task. Off until an administrator turns it on.
CREATE TABLE insights (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    model TEXT CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 200),
    report_model TEXT CHECK (CHAR_LENGTH(report_model) BETWEEN 1 AND 200),
    CHECK (NOT enabled OR model IS NOT NULL)
);
INSERT INTO insights (id) VALUES (TRUE);

-- One row per chat, claimed by a replica while it asks. labeled_through is the time of the
-- last message the labels describe; a newer message makes the chat due again.
CREATE TABLE conversation_labels (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    claimed_until TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0,
    failed_at TIMESTAMPTZ,
    labeled_through TIMESTAMPTZ,
    labeled_at TIMESTAMPTZ,
    kind TEXT CHECK (CHAR_LENGTH(kind) <= 40),
    activity TEXT CHECK (CHAR_LENGTH(activity) <= 40),
    field TEXT CHECK (CHAR_LENGTH(field) <= 40),
    output TEXT CHECK (CHAR_LENGTH(output) <= 40),
    topic TEXT CHECK (CHAR_LENGTH(topic) <= 160),
    flags TEXT[] NOT NULL DEFAULT '{}',
    turns INTEGER NOT NULL DEFAULT 0,
    tokens BIGINT NOT NULL DEFAULT 0,
    model TEXT CHECK (CHAR_LENGTH(model) <= 200)
);
CREATE INDEX conversation_labels_through_idx ON conversation_labels (labeled_through) WHERE labeled_at IS NOT NULL;
CREATE INDEX conversation_labels_owner_idx ON conversation_labels (owner_id);

-- Workstreams a model found in a period's topic lines. A report keeps its own copy of the
-- counts and topic lines it shows, never a chat or a person.
CREATE TABLE usage_reports (
    id UUID PRIMARY KEY,
    requested_by UUID REFERENCES users(id) ON DELETE SET NULL,
    group_id UUID REFERENCES user_groups(id) ON DELETE CASCADE,
    days INTEGER NOT NULL CHECK (days BETWEEN 1 AND 365),
    status TEXT NOT NULL CHECK (status IN ('building', 'ready', 'failed')),
    done INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    error TEXT CHECK (CHAR_LENGTH(error) <= 500),
    model TEXT CHECK (CHAR_LENGTH(model) <= 200),
    workstreams JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX usage_reports_scope_idx ON usage_reports (days, group_id, created_at DESC);
