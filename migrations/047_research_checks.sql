-- Two more research checks, each switched on its own under Decisions: where a new
-- experiment goes in the tree, and whether a report's statements match the runs.
ALTER TABLE decisions
    ADD COLUMN check_tree BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN check_reports BOOLEAN NOT NULL DEFAULT TRUE;

-- What the parent established that a child keeps and builds on, and what the
-- placement check found: {"model", "fits", "confirmed"}, or {"skipped": why}.
ALTER TABLE experiments
    ADD COLUMN builds_on TEXT NOT NULL DEFAULT '' CHECK (char_length(builds_on) <= 1000),
    ADD COLUMN placement JSONB;

-- Problems the last report check found, shown to the model in the next research turn:
-- {"checked", "problems": [{"text", "problem"}]}. NULL when there were none.
ALTER TABLE research ADD COLUMN report_check JSONB;
