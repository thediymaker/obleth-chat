-- A temporary chat stays out of history and search and can't be shared or moved into
-- a project. It is deleted when its owner leaves it (left_at marks one whose reply was
-- still running then) and, in any case, a day after its last activity.
ALTER TABLE conversations
    ADD COLUMN temporary BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN left_at TIMESTAMPTZ,
    ADD CONSTRAINT conversations_left_temporary CHECK (temporary OR left_at IS NULL),
    ADD CONSTRAINT conversations_temporary_private CHECK (NOT temporary OR (share_scope = 'none' AND project_id IS NULL));
CREATE INDEX conversations_temporary_idx ON conversations (updated_at) WHERE temporary;
