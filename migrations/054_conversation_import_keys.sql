-- A chat brought over from another app remembers where it came from, so importing
-- the same export again adds only the chats that are not here yet.
ALTER TABLE conversations
    ADD COLUMN import_key TEXT CHECK (char_length(import_key) BETWEEN 1 AND 300);
CREATE UNIQUE INDEX conversations_import_key_idx ON conversations(owner_id, import_key)
    WHERE import_key IS NOT NULL;
