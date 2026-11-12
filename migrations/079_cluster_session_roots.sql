-- The directories a file session's job was started with. When the cluster's setting
-- changes, the next call ends the session and starts one with the new directories,
-- since a running job keeps those it was given.
ALTER TABLE cluster_sessions ADD COLUMN roots TEXT;
