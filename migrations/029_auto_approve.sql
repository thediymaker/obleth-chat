-- Auto mode: a chat whose tool calls run without asking for Allow or Deny.
ALTER TABLE conversations ADD COLUMN auto_approve BOOLEAN NOT NULL DEFAULT FALSE;
