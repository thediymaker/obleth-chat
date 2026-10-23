-- A project's code: one line of history (main) that the project's chats sync with.
-- It is kept here as a chain of git bundles, each adding the commits since the one
-- before, so it is backed up with everything else and every replica sees the same.
CREATE TABLE project_code (
    project_id UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    main TEXT CHECK (main ~ '^[0-9a-f]{40}$'),
    commits INTEGER NOT NULL DEFAULT 0,
    bytes BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL
);

CREATE TABLE project_code_bundles (
    project_id UUID NOT NULL REFERENCES project_code(project_id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    tip TEXT NOT NULL CHECK (tip ~ '^[0-9a-f]{40}$'),
    data BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (project_id, seq)
);

-- Every main the project has had, with its commit count, so how far a chat is behind
-- is a lookup rather than a rebuild.
CREATE TABLE project_code_tips (
    project_id UUID NOT NULL REFERENCES project_code(project_id) ON DELETE CASCADE,
    tip TEXT NOT NULL CHECK (tip ~ '^[0-9a-f]{40}$'),
    commits INTEGER NOT NULL,
    PRIMARY KEY (project_id, tip)
);

-- Where each chat's workspace last synced with its project.
CREATE TABLE conversation_code (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    synced TEXT CHECK (synced ~ '^[0-9a-f]{40}$'),
    synced_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX conversation_code_project_idx ON conversation_code(project_id);

-- Where a project's code was sent: a GitHub repository, for now.
CREATE TABLE project_code_exports (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    kind TEXT NOT NULL CHECK (kind IN ('github')),
    target TEXT NOT NULL CHECK (CHAR_LENGTH(target) BETWEEN 1 AND 300),
    url TEXT NOT NULL CHECK (CHAR_LENGTH(url) BETWEEN 1 AND 500),
    tip TEXT NOT NULL CHECK (tip ~ '^[0-9a-f]{40}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX project_code_exports_project_idx ON project_code_exports(project_id, created_at DESC);
