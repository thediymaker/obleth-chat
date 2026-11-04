-- Chat's browser: one Chromium pod and profile volume per person, in the workspace
-- namespace, and one tab in it per conversation that has the browser on.
CREATE TABLE browsers (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    slug TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'stopped' CHECK (status IN ('stopped', 'starting', 'running', 'error')),
    pod_ip TEXT,
    error_message TEXT,
    search_engine TEXT NOT NULL DEFAULT 'duckduckgo' CHECK (search_engine IN ('duckduckgo', 'google', 'bing')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE browser_tabs (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    browser_id UUID NOT NULL REFERENCES browsers(id) ON DELETE CASCADE,
    -- The Chromium target; it is gone after the pod restarts and is then recreated.
    target_id TEXT,
    -- Who drives the tab: the model, or the person after Take over.
    controller TEXT NOT NULL DEFAULT 'model' CHECK (controller IN ('model', 'user')),
    url TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX browser_tabs_browser_idx ON browser_tabs(browser_id);

-- Sites where the person chose "Always on this site" for form submissions.
CREATE TABLE browser_site_grants (
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    host TEXT NOT NULL CHECK (char_length(host) BETWEEN 1 AND 253),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (owner_id, host)
);

ALTER TABLE conversations ADD COLUMN browser BOOLEAN NOT NULL DEFAULT FALSE;
