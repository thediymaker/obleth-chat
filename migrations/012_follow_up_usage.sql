-- Suggestions share account limits and actual provider token accounting, while
-- using their own short lease rather than taking over a conversation generation.
ALTER TABLE generation_usage ADD COLUMN purpose TEXT NOT NULL DEFAULT 'chat'
    CHECK (purpose IN ('chat','follow_up'));
ALTER TABLE generation_usage ADD COLUMN reservation_expires TIMESTAMPTZ;
ALTER TABLE generation_usage ADD CONSTRAINT generation_usage_follow_up_lease
    CHECK (purpose <> 'follow_up' OR reservation_expires IS NOT NULL);
ALTER TABLE messages ADD COLUMN follow_ups_expires_at TIMESTAMPTZ;
