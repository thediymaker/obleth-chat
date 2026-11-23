-- Web pages as notebook sources. A page is read in its person's own browser and kept
-- as a document in the project's collection; the document keeps the address it came
-- from and when it was last read.
ALTER TABLE knowledge_documents ADD COLUMN source_url TEXT
    CHECK (CHAR_LENGTH(source_url) BETWEEN 1 AND 2000 AND source_url ~ '^https?://');
ALTER TABLE knowledge_documents ADD COLUMN read_at TIMESTAMPTZ;

-- Each page someone asked a notebook to read: while it is read, when reading failed
-- (until they try again or remove it), and for a day after it was done, which is what
-- the reads a project may make in a day are counted from.
CREATE TABLE notebook_page_reads (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- Where the page goes: a collection of the owner's in the project.
    collection_id UUID NOT NULL REFERENCES knowledge_collections(id) ON DELETE CASCADE,
    -- The document the page became, or with again the one it reads again.
    document_id UUID REFERENCES knowledge_documents(id) ON DELETE SET NULL,
    again BOOLEAN NOT NULL DEFAULT FALSE,
    url TEXT NOT NULL CHECK (CHAR_LENGTH(url) BETWEEN 1 AND 2000 AND url ~ '^https?://'),
    status TEXT NOT NULL DEFAULT 'reading' CHECK (status IN ('reading', 'failed', 'done')),
    error TEXT CHECK (CHAR_LENGTH(error) BETWEEN 1 AND 300),
    attempts SMALLINT NOT NULL DEFAULT 0,
    -- When a replica took it up; one taken up too long ago is taken up again.
    claimed_at TIMESTAMPTZ,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX notebook_page_reads_project_id_requested_at_idx ON notebook_page_reads(project_id, requested_at);
CREATE INDEX notebook_page_reads_owner_id_status_idx ON notebook_page_reads(owner_id, status);
CREATE INDEX notebook_page_reads_collection_id_idx ON notebook_page_reads(collection_id);
CREATE INDEX notebook_page_reads_document_id_idx ON notebook_page_reads(document_id);
