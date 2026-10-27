-- Where Chat sends decisions, short yes/no and pick-one questions in the verdicts format,
-- as an administrator chose: the gateway as each person, another endpoint with its own
-- key (sealed with the installation's secret key), or nowhere. NULL model leaves the
-- choice to the endpoint.
CREATE TABLE decisions (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    endpoint TEXT NOT NULL DEFAULT 'gateway' CHECK (endpoint IN ('gateway', 'custom', 'off')),
    url TEXT CHECK (char_length(url) BETWEEN 1 AND 2048),
    api_key BYTEA,
    model TEXT CHECK (char_length(model) BETWEEN 1 AND 160),
    check_runs BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (endpoint <> 'custom' OR url IS NOT NULL)
);
INSERT INTO decisions (id) VALUES (TRUE);

-- What the run check found: {"model", "tested", "confidence"}, or {"skipped": why}.
ALTER TABLE research_runs ADD COLUMN decision JSONB;
