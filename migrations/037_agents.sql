-- Worker models: administrators mark which models the main model may hand tasks to.
ALTER TABLE model_profiles ADD COLUMN worker BOOLEAN NOT NULL DEFAULT FALSE;

-- A worker's requests are charged on their own row, leased like suggestions.
ALTER TABLE generation_usage DROP CONSTRAINT generation_usage_purpose_check;
ALTER TABLE generation_usage ADD CONSTRAINT generation_usage_purpose_check
    CHECK (purpose IN ('chat','follow_up','agent'));
ALTER TABLE generation_usage DROP CONSTRAINT generation_usage_follow_up_lease;
ALTER TABLE generation_usage ADD CONSTRAINT generation_usage_follow_up_lease
    CHECK (purpose = 'chat' OR reservation_expires IS NOT NULL);
