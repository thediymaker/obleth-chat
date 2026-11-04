-- Apps answer on a chat page too: each message is a run of the published version, as
-- the person chatting.
ALTER TABLE app_runs DROP CONSTRAINT app_runs_trigger_check;
ALTER TABLE app_runs ADD CONSTRAINT app_runs_trigger_check CHECK (trigger IN ('api', 'test', 'chat'));
