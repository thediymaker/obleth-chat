-- "Allow for this chat": a connection tool that runs without asking for the rest of
-- one conversation. Approval rows record which tool they ask about.
ALTER TABLE tool_approvals
    ADD COLUMN connection_id UUID REFERENCES connections(id) ON DELETE CASCADE,
    ADD COLUMN tool_name TEXT CHECK (CHAR_LENGTH(tool_name) <= 128);

CREATE TABLE tool_grants (
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL CHECK (CHAR_LENGTH(tool_name) BETWEEN 1 AND 128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (conversation_id, connection_id, tool_name)
);
