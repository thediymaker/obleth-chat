ALTER TABLE conversations
  ADD COLUMN settings JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(settings) = 'object'),
  ADD COLUMN archived BOOLEAN NOT NULL DEFAULT FALSE,
  ADD COLUMN pinned BOOLEAN NOT NULL DEFAULT FALSE,
  ADD COLUMN parent_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
  ADD COLUMN branch_point_id UUID;
ALTER TABLE messages
  ADD COLUMN finish_reason TEXT CHECK (char_length(finish_reason) <= 80),
  ADD COLUMN error_message TEXT CHECK (char_length(error_message) <= 1000);
CREATE INDEX conversations_owner_history_idx ON conversations(owner_id, archived, pinned DESC, updated_at DESC, id);
CREATE INDEX conversations_parent_idx ON conversations(parent_id) WHERE parent_id IS NOT NULL;
-- A legal 1 MiB message can produce a tsvector larger than PostgreSQL's 1 MiB
-- vector limit. Index all content in bounded chunks instead of rejecting the
-- message or silently indexing only its beginning. Chunk sizes count Unicode
-- characters: each 8,192-character input is at most 32 KiB in UTF-8. Overlap
-- preserves search phrases up to the API's 200-character query limit.
CREATE TABLE message_search_chunks (
  message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  chunk_no INTEGER NOT NULL CHECK (chunk_no >= 0),
  document TSVECTOR NOT NULL,
  PRIMARY KEY(message_id, chunk_no)
);
CREATE INDEX message_search_chunks_document_idx ON message_search_chunks USING GIN(document);
CREATE FUNCTION message_search_documents(input TEXT)
RETURNS TABLE(chunk_no INTEGER, document TSVECTOR)
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
  SELECT ((start_at-1)/7992)::integer,
         to_tsvector('simple',substring(input FROM start_at FOR 8192))
  FROM generate_series(1,char_length(input),7992) AS positions(start_at)
$$;
CREATE FUNCTION refresh_message_search_chunks() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP='UPDATE' AND NEW.content IS NOT DISTINCT FROM OLD.content THEN
    RETURN NULL;
  END IF;
  DELETE FROM message_search_chunks WHERE message_id=NEW.id;
  INSERT INTO message_search_chunks(message_id,chunk_no,document)
    SELECT NEW.id,chunk_no,document FROM message_search_documents(NEW.content);
  RETURN NULL;
END
$$;
CREATE TRIGGER messages_refresh_search AFTER INSERT OR UPDATE OF content ON messages
  FOR EACH ROW EXECUTE FUNCTION refresh_message_search_chunks();
INSERT INTO message_search_chunks(message_id,chunk_no,document)
  SELECT m.id,parts.chunk_no,parts.document FROM messages m
  CROSS JOIN LATERAL message_search_documents(m.content) parts;

-- Existing conversations already had a per-chat effort control. Preserve that choice.
UPDATE conversations SET settings=jsonb_build_object('reasoningEffort',reasoning_effort);
