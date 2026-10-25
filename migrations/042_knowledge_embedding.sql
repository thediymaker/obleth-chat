-- Knowledge's embedding model and the prefixes it expects, as an administrator chose
-- them; NULL keeps the environment's default. `indexed_with` records the settings that
-- made the stored vectors. When the settings in use differ, every document and saved
-- item is indexed again, since vectors from different models can't be compared.
CREATE TABLE knowledge_embedding (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    model TEXT CHECK (char_length(model) BETWEEN 1 AND 160),
    query_prefix TEXT CHECK (char_length(query_prefix) <= 500),
    document_prefix TEXT CHECK (char_length(document_prefix) <= 500),
    indexed_with JSONB,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO knowledge_embedding (id) VALUES (TRUE);
