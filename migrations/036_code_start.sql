-- A chat started from an earlier commit: its workspace begins there, not at main.
ALTER TABLE conversation_code ADD COLUMN start TEXT CHECK (start ~ '^[0-9a-f]{40}$');
