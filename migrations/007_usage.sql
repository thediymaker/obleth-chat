-- Actual Chat generations only. Imported conversations never populate this ledger.
CREATE TABLE generation_usage (
    generation_id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'reserved' CHECK (status IN ('reserved','completed','interrupted','error')),
    reserved_output_tokens BIGINT NOT NULL CHECK (reserved_output_tokens BETWEEN 1 AND 32768),
    context_bytes BIGINT NOT NULL CHECK (context_bytes >= 0),
    prompt_tokens BIGINT CHECK (prompt_tokens >= 0),
    completion_tokens BIGINT CHECK (completion_tokens >= 0),
    total_tokens BIGINT CHECK (total_tokens >= 0),
    usage_complete BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX generation_usage_owner_day ON generation_usage(user_id,created_at DESC);
CREATE INDEX generation_usage_day ON generation_usage(created_at DESC);
CREATE INDEX generation_usage_pending ON generation_usage(user_id) WHERE status='reserved';
CREATE INDEX conversations_active_generation_idx ON conversations(active_generation)
    WHERE active_generation IS NOT NULL;
CREATE TABLE site_limits (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    limits JSONB NOT NULL DEFAULT '{"dailyRequests":null,"dailyTokens":null,"concurrentRequests":3}'
        CHECK (jsonb_typeof(limits)='object'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO site_limits(id) VALUES(TRUE);
CREATE TABLE user_limit_overrides (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    limits JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(limits)='object'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
