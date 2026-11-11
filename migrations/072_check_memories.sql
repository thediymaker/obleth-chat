-- Decisions ask whether each memory a chat would teach is worth keeping.
ALTER TABLE decisions ADD COLUMN check_memories BOOLEAN NOT NULL DEFAULT TRUE;
