-- Batch rows are now expired by the hourly retention sweep across every owner,
-- so the sweep's deletes need the time columns alone.
CREATE INDEX batch_sources_created_idx ON batch_sources (created_at);
CREATE INDEX batch_labels_updated_idx ON batch_labels (updated_at);
