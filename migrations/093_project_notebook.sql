-- A project's notebook: each person ticks which of the project's documents a
-- notebook chat answers from, and an administrator can name a Notebook model that
-- writes a short guide for each new document in a project's collections.

-- The Notebook model, unset as installed.
CREATE TABLE notebook_settings (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    model TEXT CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 160),
    -- When the model was named; documents indexed since then get guides.
    model_set_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO notebook_settings DEFAULT VALUES;

-- A person's notebook in a project: made the first time they open it, when every
-- document then in the project is ticked for them.
CREATE TABLE project_notebooks (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- The chat the notebook shows.
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX project_notebooks_owner_id_project_id_idx ON project_notebooks(owner_id, project_id);
CREATE INDEX project_notebooks_project_id_idx ON project_notebooks(project_id);
CREATE INDEX project_notebooks_conversation_id_idx ON project_notebooks(conversation_id);

-- The documents a person ticked in a project's notebook.
CREATE TABLE notebook_sources (
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    document_id UUID NOT NULL REFERENCES knowledge_documents(id) ON DELETE CASCADE,
    ticked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (owner_id, project_id, document_id)
);
CREATE INDEX notebook_sources_project_id_idx ON notebook_sources(project_id);
CREATE INDEX notebook_sources_document_id_idx ON notebook_sources(document_id);

-- Chats started in a notebook: their knowledge search is limited to the documents
-- their owner ticked in the project, and with only_sources the model answers from
-- those passages alone.
CREATE TABLE notebook_chats (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    only_sources BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX notebook_chats_project_id_idx ON notebook_chats(project_id);

-- A document's text size, which the notebook's meter adds up. Set when indexing
-- finishes; documents indexed earlier get it the first time a meter counts them.
ALTER TABLE knowledge_documents ADD COLUMN text_chars BIGINT CHECK (text_chars >= 0);

-- A short guide to a document, made once by the Notebook model: a few sentences and
-- three questions it answers. Charged to the document's owner.
CREATE TABLE notebook_guides (
    document_id UUID PRIMARY KEY REFERENCES knowledge_documents(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'ready', 'failed')),
    summary TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(summary) <= 1200),
    questions TEXT[] NOT NULL DEFAULT '{}'
        CHECK (cardinality(questions) <= 3 AND CHAR_LENGTH(array_to_string(questions, '')) <= 600),
    model TEXT CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 160),
    error TEXT CHECK (CHAR_LENGTH(error) <= 300),
    attempts SMALLINT NOT NULL DEFAULT 0,
    -- A replica works on it until then; a guide left running past it is tried again.
    claimed_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX notebook_guides_owner_id_idx ON notebook_guides(owner_id);
CREATE INDEX notebook_guides_claimed_until_idx ON notebook_guides(claimed_until) WHERE status = 'running';

-- Notebook work is charged on its own leased usage rows, like workers.
ALTER TABLE generation_usage DROP CONSTRAINT generation_usage_purpose_check;
ALTER TABLE generation_usage ADD CONSTRAINT generation_usage_purpose_check
    CHECK (purpose IN ('chat','follow_up','agent','notebook')) NOT VALID;
ALTER TABLE generation_usage VALIDATE CONSTRAINT generation_usage_purpose_check;
