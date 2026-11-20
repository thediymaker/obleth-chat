-- The admin Workspaces page reads the sessions that overlap its period: the open ones
-- and those stopped since it began. Without an index on stopped_at that read every
-- session ever recorded.
CREATE INDEX workspace_sessions_stopped_idx ON workspace_sessions (stopped_at);
