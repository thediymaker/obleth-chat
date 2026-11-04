-- When a workspace or browser began starting. The replica that starts it records
-- the outcome; if that replica stops first, the reconciler settles the record once
-- the start is overdue, so it can't stay 'starting' for good.
ALTER TABLE sandbox_pods ADD COLUMN starting_since TIMESTAMPTZ;
ALTER TABLE browsers ADD COLUMN starting_since TIMESTAMPTZ;
UPDATE sandbox_pods SET starting_since=last_used_at WHERE status='starting';
UPDATE browsers SET starting_since=last_used_at WHERE status='starting';
