-- Measured size of each user's workspace volume, refreshed while the pod runs.
ALTER TABLE sandbox_pods
    ADD COLUMN disk_bytes BIGINT CHECK (disk_bytes >= 0),
    ADD COLUMN disk_checked_at TIMESTAMPTZ;
