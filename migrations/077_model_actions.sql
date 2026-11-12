-- How far a model is trusted with changes through connections: decided from its
-- certification score unless an administrator sets it, and what certification found.
ALTER TABLE model_profiles ADD COLUMN actions TEXT NOT NULL DEFAULT 'auto'
    CHECK (actions IN ('auto', 'certified', 'provisional', 'blocked'));
ALTER TABLE model_profiles ADD COLUMN certification JSONB
    CHECK (certification IS NULL OR (jsonb_typeof(certification) = 'object' AND OCTET_LENGTH(certification::text) <= 4096));
