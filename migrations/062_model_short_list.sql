-- The models the chooser shows first. With none listed, it shows every model; with some,
-- the rest are a click away. Kept apart from model_profiles so a model with no saved
-- profile can be listed too.
CREATE TABLE model_short_list (
    model_id TEXT PRIMARY KEY CHECK (OCTET_LENGTH(model_id) BETWEEN 1 AND 160),
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
