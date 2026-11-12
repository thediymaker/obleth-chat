-- A device plugged into the person's computer is a fourth kind of connection.
ALTER TABLE connections DROP CONSTRAINT connections_kind_check;
ALTER TABLE connections ADD CONSTRAINT connections_kind_check
    CHECK (kind IN ('mcp', 'slurm', 'kubernetes', 'device'));

-- Tool calls a device connection sends to the person's browser, which carries
-- them out against the device plugged into their computer. The replica running
-- the response writes the row and polls it; any replica can record the result.
CREATE TABLE device_calls (
    generation_id UUID NOT NULL,
    call_id TEXT NOT NULL CHECK (CHAR_LENGTH(call_id) BETWEEN 1 AND 128),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    arguments JSONB NOT NULL DEFAULT '{}'::jsonb,
    ok BOOLEAN,
    output TEXT,
    detail TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (generation_id, call_id)
);
CREATE INDEX device_calls_pending ON device_calls (conversation_id) WHERE ok IS NULL;
