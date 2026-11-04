-- Apps: a project turned into an API. The id is a fixed code that never changes, so
-- apps never collide and a rename never breaks a caller; the slug is only for people.
-- The draft is what the owner edits and tries; live is the published copy that
-- answers calls, NULL until the app is published.
CREATE TABLE apps (
    id TEXT PRIMARY KEY CHECK (id ~ '^[a-z0-9]{8}$'),
    project_id UUID NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
    owner_id UUID REFERENCES users(id) ON DELETE SET NULL,
    name TEXT NOT NULL CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 120),
    slug TEXT NOT NULL CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND CHAR_LENGTH(slug) <= 60),
    description TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(description) <= 1000),
    draft JSONB NOT NULL CHECK (jsonb_typeof(draft) = 'object' AND OCTET_LENGTH(draft::text) <= 131072),
    live JSONB CHECK (live IS NULL OR jsonb_typeof(live) = 'object'),
    version INTEGER NOT NULL DEFAULT 0,
    published_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Keys that call an app, stored only as a hash. A call runs as the key's maker.
CREATE TABLE app_keys (
    id UUID PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 80),
    token_hash BYTEA NOT NULL UNIQUE,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);
CREATE INDEX app_keys_app ON app_keys(app_id) WHERE revoked_at IS NULL;

-- Each run, kept for a week with its inputs and step-by-step trace. Version 0 is a
-- test of the draft.
CREATE TABLE app_runs (
    id UUID PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    trigger TEXT NOT NULL CHECK (trigger IN ('api', 'test')),
    actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
    key_id UUID REFERENCES app_keys(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'completed', 'failed')),
    inputs JSONB NOT NULL DEFAULT '{}',
    output JSONB,
    trace JSONB NOT NULL DEFAULT '[]',
    error TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX app_runs_recent ON app_runs(app_id, started_at DESC);
