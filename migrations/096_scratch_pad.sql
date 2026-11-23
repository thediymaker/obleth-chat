-- The scratch pad: things a person jots down from anywhere in the app, to keep or to
-- turn into a note, a card or a chat later. Each jotting is its owner's alone, even
-- when it is tagged with a project others share. The API keeps at most 500 a person.
CREATE TABLE scratch_jottings (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The project it belongs with, if any; it stays on the pad when the project goes.
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    text TEXT NOT NULL CHECK (CHAR_LENGTH(text) BETWEEN 1 AND 4000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX scratch_jottings_owner_id_created_at_idx ON scratch_jottings (owner_id, created_at DESC);
-- For the ON DELETE SET NULL when a project is deleted.
CREATE INDEX scratch_jottings_project_id_idx ON scratch_jottings (project_id) WHERE project_id IS NOT NULL;
