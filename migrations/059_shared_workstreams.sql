-- One workstream list for every period and group, in place of a report per view. Chat finds
-- the list on its own, keeps names that still fit, and places each labelled chat in it.
CREATE TABLE workstreams (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(description) <= 300),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    retired_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX workstreams_current_name_idx ON workstreams (LOWER(name)) WHERE retired_at IS NULL;

-- placed_at is when a chat's topic was last placed; NULL means it waits for a place, and
-- workstream_id NULL once placed means it fits none.
ALTER TABLE conversation_labels
    ADD COLUMN workstream_id UUID REFERENCES workstreams(id) ON DELETE SET NULL,
    ADD COLUMN placed_at TIMESTAMPTZ;
CREATE INDEX conversation_labels_unplaced_idx ON conversation_labels (labeled_through)
    WHERE placed_at IS NULL AND topic IS NOT NULL;

-- Finding and placing run as an administrator: the one who last saved the settings or
-- asked for the list again.
ALTER TABLE insights
    ADD COLUMN run_as UUID REFERENCES users(id) ON DELETE SET NULL,
    ADD COLUMN streams_status TEXT NOT NULL DEFAULT 'none'
        CHECK (streams_status IN ('none', 'building', 'ready', 'failed')),
    ADD COLUMN streams_requested BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN streams_claimed_until TIMESTAMPTZ,
    ADD COLUMN streams_tried_at TIMESTAMPTZ,
    ADD COLUMN streams_found_at TIMESTAMPTZ,
    ADD COLUMN streams_model TEXT CHECK (CHAR_LENGTH(streams_model) <= 200),
    ADD COLUMN streams_error TEXT CHECK (CHAR_LENGTH(streams_error) <= 500),
    ADD COLUMN placing_until TIMESTAMPTZ;
UPDATE insights SET run_as = (
    SELECT a.actor_id FROM audit_events a JOIN users u ON u.id = a.actor_id
    WHERE a.action IN ('insights.update', 'insights.report') AND u.role = 'admin'
    ORDER BY a.created_at DESC LIMIT 1);

DROP TABLE usage_reports;
