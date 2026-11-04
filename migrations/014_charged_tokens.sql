-- Admission-only charge for finished turns without reported provider usage.
-- Reports keep showing reported totals; this never fills the token columns.
ALTER TABLE generation_usage ADD COLUMN charged_tokens BIGINT CHECK (charged_tokens >= 0);
