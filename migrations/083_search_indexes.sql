-- Saved-item and memory search read the person's own rows once and rank them by
-- meaning and by words in the same statement, so Postgres scans that small set and
-- never used the vector or full-text indexes on these tables. Each person has at
-- most 5,000 saved items, so an exact scan stays cheap; the unused indexes only
-- cost work on every insert and update.
DROP INDEX IF EXISTS saved_items_embedding_idx;
DROP INDEX IF EXISTS saved_items_tsv_idx;
DROP INDEX IF EXISTS memories_tsv_idx;

-- The memory worker looks for memories still waiting for an embedding, oldest first.
CREATE INDEX memories_pending_idx ON memories (created_at)
    WHERE embedding IS NULL AND embedding_attempts < 3;
