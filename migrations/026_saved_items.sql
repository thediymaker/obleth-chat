-- Things a person saves while they work: functions, notes, links and excerpts from
-- replies. Each is one searchable unit, embedded like a knowledge passage, so a later
-- chat can find "that throughput function".
CREATE FUNCTION saved_tags_text(tags TEXT[]) RETURNS TEXT
    LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$ SELECT array_to_string(tags, ' ') $$;

CREATE TABLE saved_items (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('code', 'note', 'link', 'excerpt')),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    body TEXT NOT NULL DEFAULT '' CHECK (octet_length(body) <= 65536),
    language TEXT NOT NULL DEFAULT '' CHECK (char_length(language) <= 40),
    url TEXT CHECK (char_length(url) <= 2048),
    tags TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 12),
    -- Whether chats may find it with search_knowledge.
    searchable BOOLEAN NOT NULL DEFAULT TRUE,
    source_conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    source_message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    -- Filled when saved, or by the knowledge worker if the embedding model was busy.
    embedding halfvec(1024),
    embedding_attempts SMALLINT NOT NULL DEFAULT 0,
    tsv tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english', title), 'A')
        || setweight(to_tsvector('simple', saved_tags_text(tags)), 'B')
        || to_tsvector('english', left(body, 20000))
    ) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX saved_items_owner_idx ON saved_items(owner_id, created_at DESC);
CREATE INDEX saved_items_tags_idx ON saved_items USING gin (tags);
CREATE INDEX saved_items_tsv_idx ON saved_items USING gin (tsv);
CREATE INDEX saved_items_embedding_idx ON saved_items USING hnsw (embedding halfvec_cosine_ops);
CREATE INDEX saved_items_pending_idx ON saved_items(created_at)
    WHERE embedding IS NULL AND searchable;
