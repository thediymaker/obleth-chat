-- Second opinions on a reply Auto chose. Now and then Chat also puts the message to
-- the models Auto ranked next, in the background, and keeps their answers as versions
-- of the reply, so the person can read them side by side and keep one. An
-- administrator chooses when: Auto's pick was close (the runner-up scored within
-- close_gap of it), the request read as hard, or one Auto reply in sample_every (0 is
-- never). daily_limit is how many a person gets in a day. All off as installed.
CREATE TABLE compare_settings (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    close_call BOOLEAN NOT NULL DEFAULT FALSE,
    close_gap DOUBLE PRECISION NOT NULL DEFAULT 0.05 CHECK (close_gap BETWEEN 0 AND 1),
    hard BOOLEAN NOT NULL DEFAULT FALSE,
    sample_every INTEGER NOT NULL DEFAULT 0 CHECK (sample_every BETWEEN 0 AND 1000),
    daily_limit INTEGER NOT NULL DEFAULT 3 CHECK (daily_limit BETWEEN 1 AND 100),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO compare_settings (id) VALUES (TRUE);

-- Those answers are requests of their own in the usage ledger, each inside a lease
-- like a follow-up's.
ALTER TABLE generation_usage DROP CONSTRAINT generation_usage_purpose_check;
ALTER TABLE generation_usage ADD CONSTRAINT generation_usage_purpose_check
    CHECK (purpose IN ('chat','follow_up','agent','compare')) NOT VALID;
ALTER TABLE generation_usage VALIDATE CONSTRAINT generation_usage_purpose_check;
