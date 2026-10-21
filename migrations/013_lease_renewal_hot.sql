-- Lease renewals rewrite only generation_expires. Keeping that column out of
-- every index (and leaving page space) lets PostgreSQL apply renewals as
-- heap-only tuple updates. Active-lease lookups still filter by owner here.
DROP INDEX conversations_active_owner_idx;
CREATE INDEX conversations_active_owner_idx ON conversations(owner_id)
    WHERE active_generation IS NOT NULL;
ALTER TABLE conversations SET (fillfactor = 90);
