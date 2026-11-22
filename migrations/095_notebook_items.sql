-- What a person makes from the ticked sources of a project's notebook: a briefing,
-- FAQ, study guide or timeline the Notebook model writes in the background from the
-- sources' full text. Its quotations are checked against the sources; the verified
-- ones are kept as citations, and the others are only counted.
CREATE TABLE notebook_items (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('briefing', 'faq', 'study_guide', 'timeline')),
    title TEXT NOT NULL CHECK (CHAR_LENGTH(title) BETWEEN 1 AND 200),
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'ready', 'failed')),
    -- The sources it reads, as ticked when it was started, and their names then.
    document_ids UUID[] NOT NULL CHECK (cardinality(document_ids) BETWEEN 1 AND 1000),
    sources JSONB NOT NULL DEFAULT '[]'
        CHECK (jsonb_typeof(sources) = 'array' AND OCTET_LENGTH(sources::text) <= 400000),
    -- Markdown, with each verified quotation as [n].
    body TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(body) <= 100000),
    -- One entry per [n]: its document, name, page, section and the words quoted.
    citations JSONB NOT NULL DEFAULT '[]'
        CHECK (jsonb_typeof(citations) = 'array' AND OCTET_LENGTH(citations::text) <= 600000),
    unverified INTEGER NOT NULL DEFAULT 0 CHECK (unverified >= 0),
    model TEXT CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 160),
    error TEXT CHECK (CHAR_LENGTH(error) <= 500),
    attempts SMALLINT NOT NULL DEFAULT 0,
    -- A replica works on it until then; one left running past it is tried again once.
    claimed_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX notebook_items_owner_id_project_id_idx ON notebook_items(owner_id, project_id, created_at DESC);
CREATE INDEX notebook_items_project_id_idx ON notebook_items(project_id);
CREATE INDEX notebook_items_claimed_until_idx ON notebook_items(claimed_until) WHERE status = 'running';
