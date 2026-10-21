-- Auto mode's watchdog: nudges sent in a row without progress, and when the last was.
ALTER TABLE conversations
    ADD COLUMN auto_nudges INT NOT NULL DEFAULT 0,
    ADD COLUMN auto_nudged_at TIMESTAMPTZ;
