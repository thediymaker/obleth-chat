-- The project log: one entry a day for each person's own work in a project, written
-- by a model from that day's chats, research runs and commits once the work has gone
-- quiet or the day has ended. Each line cites the items it came from. A person's own
-- notes are kept apart from what the model wrote, and a line they edited is kept as
-- they left it, so writing a day again never loses either. An administrator turns
-- the log on and picks its model; off as installed.
CREATE TABLE project_log_settings (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    -- The model that writes the days.
    model TEXT CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 160),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO project_log_settings (id) VALUES (TRUE);

-- Each person's own switch (on unless they turn it off), and the time zone their
-- browser reported, which decides where their days begin and end (UTC until then).
CREATE TABLE project_log_choices (
    owner_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    time_zone TEXT CHECK (time_zone ~ '^[A-Za-z][A-Za-z0-9_+/-]{0,63}$'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One person's day in one project. A row exists once the day has work in it; it is
-- written when that work has gone quiet or the day has ended, and again, a bounded
-- number of times, when more work arrives.
CREATE TABLE project_log_days (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day DATE NOT NULL,
    -- The zone the day was cut in, so its hours stay put if the person moves.
    time_zone TEXT NOT NULL CHECK (time_zone ~ '^[A-Za-z][A-Za-z0-9_+/-]{0,63}$'),
    headline TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(headline) <= 200),
    -- The newest work seen in the day, and the newest work the written entry includes.
    work_at TIMESTAMPTZ NOT NULL,
    written_through TIMESTAMPTZ,
    written_at TIMESTAMPTZ,
    model TEXT CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 160),
    -- Times the day was written on its own, which is bounded.
    writes INTEGER NOT NULL DEFAULT 0 CHECK (writes BETWEEN 0 AND 1000),
    claimed_until TIMESTAMPTZ,
    failed_at TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX project_log_days_project_id_owner_id_day_idx ON project_log_days (project_id, owner_id, day);
CREATE INDEX project_log_days_owner_id_day_idx ON project_log_days (owner_id, day);
-- Days with work the entry does not include yet, which the worker looks through.
CREATE INDEX project_log_days_work_at_idx ON project_log_days (work_at)
    WHERE written_through IS NULL OR written_through < work_at;

-- What the model wrote for a day: timed steps and decisions, and what was still open
-- at its end. refs lists what a line came from, as {"kind", "id", "label"}: a chat, a
-- research run or a commit, each checked against the day's items before it is kept.
CREATE TABLE project_log_lines (
    id UUID PRIMARY KEY,
    day_id UUID NOT NULL REFERENCES project_log_days(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('step', 'decision', 'open')),
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 1000),
    happened_at TIMESTAMPTZ,
    text TEXT NOT NULL CHECK (CHAR_LENGTH(text) BETWEEN 1 AND 1000),
    refs JSONB NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(refs) = 'array' AND jsonb_array_length(refs) <= 12),
    -- Set when the person changed the text; such a line stays when the day is written again.
    edited_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX project_log_lines_day_id_position_idx ON project_log_lines (day_id, position);

-- The person's own notes on a day.
CREATE TABLE project_log_notes (
    id UUID PRIMARY KEY,
    day_id UUID NOT NULL REFERENCES project_log_days(id) ON DELETE CASCADE,
    text TEXT NOT NULL CHECK (CHAR_LENGTH(text) BETWEEN 1 AND 2000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX project_log_notes_day_id_created_at_idx ON project_log_notes (day_id, created_at);

-- Runs that finished lately count as work of their day.
CREATE INDEX research_runs_finished_at_idx ON research_runs (finished_at) WHERE finished_at IS NOT NULL;

-- Writing a day is a request of its own in the usage ledger, inside a lease. The list
-- keeps 'compare', which another change adds in migration 091, and 'notebook' from 093.
ALTER TABLE generation_usage DROP CONSTRAINT generation_usage_purpose_check;
ALTER TABLE generation_usage ADD CONSTRAINT generation_usage_purpose_check
    CHECK (purpose IN ('chat', 'follow_up', 'agent', 'compare', 'notebook', 'log')) NOT VALID;
ALTER TABLE generation_usage VALIDATE CONSTRAINT generation_usage_purpose_check;
