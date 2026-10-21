-- Research in a project: an experiment tree whose nodes are git branches in one chat's
-- workspace, and runs of a fixed command on a node's committed code.
CREATE TABLE research (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The chat whose workspace holds the repository and runs the experiments.
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    goal TEXT NOT NULL CHECK (char_length(goal) BETWEEN 1 AND 2000),
    metric TEXT NOT NULL CHECK (metric ~ '^[A-Za-z_][A-Za-z0-9_.]{0,63}$'),
    direction TEXT NOT NULL CHECK (direction IN ('max', 'min')),
    run_command TEXT NOT NULL CHECK (char_length(run_command) BETWEEN 1 AND 2000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE experiments (
    id UUID PRIMARY KEY,
    research_id UUID NOT NULL REFERENCES research(id) ON DELETE CASCADE,
    parent_id UUID REFERENCES experiments(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 4000),
    branch TEXT NOT NULL,
    -- A node freezes once a run answers it; later ideas branch a child.
    frozen BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (research_id, branch)
);
CREATE INDEX experiments_research_idx ON experiments(research_id, created_at);

CREATE TABLE research_runs (
    id UUID PRIMARY KEY,
    research_id UUID NOT NULL REFERENCES research(id) ON DELETE CASCADE,
    experiment_id UUID NOT NULL REFERENCES experiments(id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    commit_sha TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed', 'cancelled', 'lost')),
    exit_code INTEGER,
    metric DOUBLE PRECISION,
    metrics JSONB NOT NULL DEFAULT '{}',
    log_tail TEXT NOT NULL DEFAULT '',
    diff TEXT NOT NULL DEFAULT '',
    diff_stat TEXT NOT NULL DEFAULT '',
    -- Reported to the model by experiment_wait, so a finish is reported once.
    reported BOOLEAN NOT NULL DEFAULT FALSE,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    checked_at TIMESTAMPTZ,
    UNIQUE (research_id, number)
);
CREATE INDEX research_runs_running_idx ON research_runs(checked_at) WHERE status = 'running';
CREATE INDEX research_runs_experiment_idx ON research_runs(experiment_id, number);
