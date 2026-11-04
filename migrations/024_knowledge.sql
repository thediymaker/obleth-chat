-- Knowledge: collections of documents the model can search and cite. Original files
-- live in object storage; their text, embeddings and full-text index live here.
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE knowledge_collections (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 1000),
    -- personal: the owner; group: the owner and members of group_id; everyone: all users.
    scope TEXT NOT NULL DEFAULT 'personal' CHECK (scope IN ('personal', 'group', 'everyone')),
    group_id UUID REFERENCES user_groups(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX knowledge_collections_owner_idx ON knowledge_collections(owner_id, updated_at DESC);
CREATE INDEX knowledge_collections_group_idx ON knowledge_collections(group_id) WHERE scope = 'group';

CREATE TABLE knowledge_documents (
    id UUID PRIMARY KEY,
    collection_id UUID NOT NULL REFERENCES knowledge_collections(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 255),
    mime_type TEXT NOT NULL CHECK (char_length(mime_type) BETWEEN 1 AND 120),
    size BIGINT NOT NULL CHECK (size > 0),
    object_key TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'processing', 'ready', 'needs_ocr', 'failed')),
    -- While processing: reading, embedding or indexing, with progress over passages.
    stage TEXT NOT NULL DEFAULT '' CHECK (char_length(stage) <= 40),
    progress_done INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER NOT NULL DEFAULT 0,
    pages INTEGER,
    passages INTEGER NOT NULL DEFAULT 0,
    error TEXT CHECK (char_length(error) <= 1000),
    attempts INTEGER NOT NULL DEFAULT 0,
    claimed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX knowledge_documents_collection_idx ON knowledge_documents(collection_id, created_at DESC);
CREATE INDEX knowledge_documents_owner_idx ON knowledge_documents(owner_id);
CREATE INDEX knowledge_documents_queue_idx ON knowledge_documents(created_at)
    WHERE status IN ('queued', 'processing');

CREATE TABLE knowledge_passages (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    document_id UUID NOT NULL REFERENCES knowledge_documents(id) ON DELETE CASCADE,
    collection_id UUID NOT NULL REFERENCES knowledge_collections(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    page INTEGER,
    content TEXT NOT NULL CHECK (char_length(content) BETWEEN 1 AND 8000),
    embedding halfvec(1024) NOT NULL,
    tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED,
    UNIQUE (document_id, ordinal)
);
CREATE INDEX knowledge_passages_collection_idx ON knowledge_passages(collection_id);
CREATE INDEX knowledge_passages_embedding_idx ON knowledge_passages
    USING hnsw (embedding halfvec_cosine_ops);
CREATE INDEX knowledge_passages_tsv_idx ON knowledge_passages USING gin (tsv);

-- Files whose rows are gone, waiting to be removed from object storage. A trigger
-- fills it so deleting a collection or a user cleans up storage too.
CREATE TABLE knowledge_object_trash (
    object_key TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE FUNCTION knowledge_trash_object() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO knowledge_object_trash(object_key) VALUES (OLD.object_key) ON CONFLICT DO NOTHING;
    RETURN OLD;
END $$;
CREATE TRIGGER knowledge_documents_trash AFTER DELETE ON knowledge_documents
    FOR EACH ROW EXECUTE FUNCTION knowledge_trash_object();

-- Collections a conversation searches.
CREATE TABLE conversation_collections (
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    collection_id UUID NOT NULL REFERENCES knowledge_collections(id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, collection_id)
);
CREATE INDEX conversation_collections_collection_idx ON conversation_collections(collection_id);
