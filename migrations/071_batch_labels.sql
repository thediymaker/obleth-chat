-- What a person adds to their own batches to keep them sorted: tags, and a name of
-- their own in place of the one the batch started with. Any batch the person can
-- see may have one, started from Chat or not.
CREATE TABLE batch_labels (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The gateway's batch id.
    batch_id TEXT NOT NULL CHECK (char_length(batch_id) <= 128),
    name TEXT CHECK (name IS NULL OR char_length(name) BETWEEN 1 AND 200),
    tags TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 12),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, batch_id)
);
