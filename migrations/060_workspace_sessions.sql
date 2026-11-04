-- When each workspace and browser pod started, became ready and stopped, so administrators
-- can see pod hours, the busiest moments and starts turned away because every slot was in
-- use. Only times and who, never what ran. A trigger on each pod table keeps it, so every
-- way a pod starts or stops is counted.
CREATE TABLE workspace_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind TEXT NOT NULL CHECK (kind IN ('workspace', 'browser')),
    owner_id UUID REFERENCES users(id) ON DELETE SET NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ready_at TIMESTAMPTZ,
    stopped_at TIMESTAMPTZ,
    -- How it ended: stopped (idle, asked or removed), error, or full when no slot was free.
    outcome TEXT CHECK (outcome IN ('stopped', 'error', 'full'))
);
CREATE INDEX workspace_sessions_open_idx ON workspace_sessions (kind, owner_id) WHERE stopped_at IS NULL;
CREATE INDEX workspace_sessions_started_idx ON workspace_sessions (started_at);

CREATE FUNCTION workspace_sessions_track() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    pod_kind TEXT := CASE TG_TABLE_NAME WHEN 'browsers' THEN 'browser' ELSE 'workspace' END;
BEGIN
    IF TG_OP = 'DELETE' THEN
        UPDATE workspace_sessions SET stopped_at = NOW(), outcome = 'stopped'
        WHERE kind = pod_kind AND owner_id = OLD.owner_id AND stopped_at IS NULL;
        RETURN OLD;
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.status = OLD.status THEN
        RETURN NEW;
    END IF;
    IF NEW.status = 'starting' THEN
        UPDATE workspace_sessions SET stopped_at = NOW(), outcome = 'stopped'
        WHERE kind = pod_kind AND owner_id = NEW.owner_id AND stopped_at IS NULL;
        INSERT INTO workspace_sessions (kind, owner_id) VALUES (pod_kind, NEW.owner_id);
    ELSIF NEW.status = 'running' THEN
        UPDATE workspace_sessions SET ready_at = NOW()
        WHERE kind = pod_kind AND owner_id = NEW.owner_id AND stopped_at IS NULL AND ready_at IS NULL;
        IF NOT FOUND AND NOT EXISTS (SELECT 1 FROM workspace_sessions
                WHERE kind = pod_kind AND owner_id = NEW.owner_id AND stopped_at IS NULL) THEN
            INSERT INTO workspace_sessions (kind, owner_id, ready_at) VALUES (pod_kind, NEW.owner_id, NOW());
        END IF;
    ELSIF NEW.status IN ('stopped', 'error') THEN
        UPDATE workspace_sessions SET stopped_at = NOW(), outcome = CASE
            WHEN NEW.status = 'stopped' THEN 'stopped'
            WHEN NEW.error_message LIKE 'All % slots are in use%' THEN 'full'
            ELSE 'error' END
        WHERE kind = pod_kind AND owner_id = NEW.owner_id AND stopped_at IS NULL;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workspace_sessions_track AFTER INSERT OR UPDATE OF status OR DELETE ON sandbox_pods
    FOR EACH ROW EXECUTE FUNCTION workspace_sessions_track();
CREATE TRIGGER workspace_sessions_track AFTER INSERT OR UPDATE OF status OR DELETE ON browsers
    FOR EACH ROW EXECUTE FUNCTION workspace_sessions_track();

-- Pods running now start their history here.
INSERT INTO workspace_sessions (kind, owner_id, started_at, ready_at)
SELECT 'workspace', owner_id, NOW(), CASE WHEN status = 'running' THEN NOW() END FROM sandbox_pods WHERE status IN ('starting', 'running')
UNION ALL
SELECT 'browser', owner_id, NOW(), CASE WHEN status = 'running' THEN NOW() END FROM browsers WHERE status IN ('starting', 'running');
