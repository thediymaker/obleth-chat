-- Notes a person sends while a reply is working; the tool loop reads them at the
-- next step boundary.
CREATE TABLE steer_notes (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    generation_id UUID NOT NULL,
    text TEXT NOT NULL CHECK (char_length(text) BETWEEN 1 AND 2000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    consumed_at TIMESTAMPTZ
);
CREATE INDEX steer_notes_pending ON steer_notes (generation_id) WHERE consumed_at IS NULL;
