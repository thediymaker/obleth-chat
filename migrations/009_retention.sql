-- Explicit opt-in only: existing conversations, files and audit events live forever.
CREATE TABLE retention_policy (
  id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
  policy JSONB NOT NULL DEFAULT '{"conversationDays":null,"unusedFileDays":null,"auditDays":null}'
    CHECK (jsonb_typeof(policy)='object'),
  last_run TIMESTAMPTZ,
  last_result JSONB,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO retention_policy(id) VALUES(TRUE);
CREATE INDEX conversations_retention_idx ON conversations(updated_at,owner_id,id);
CREATE INDEX attachments_retention_idx ON attachments(created_at,owner_id,id);
