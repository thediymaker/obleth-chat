-- Stop is an explicit, shared request; closing a browser no longer cancels work.
ALTER TABLE conversations ADD COLUMN generation_cancel_requested BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE messages ADD COLUMN follow_ups JSONB NOT NULL DEFAULT '[]'
    CHECK (jsonb_typeof(follow_ups) = 'array');
ALTER TABLE messages ADD COLUMN follow_ups_status TEXT NOT NULL DEFAULT 'idle'
    CHECK (follow_ups_status IN ('idle','pending','completed','error'));

-- Progress is checkpointed frequently. Index the terminal response once,
-- including a partial answer preserved after interruption, rather than on each
-- streamed checkpoint.
CREATE OR REPLACE FUNCTION refresh_message_search_chunks() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.status='pending' THEN RETURN NULL; END IF;
  IF TG_OP='UPDATE' AND NEW.content IS NOT DISTINCT FROM OLD.content
     AND OLD.status<>'pending' THEN RETURN NULL; END IF;
  DELETE FROM message_search_chunks WHERE message_id=NEW.id;
  INSERT INTO message_search_chunks(message_id,chunk_no,document)
    SELECT NEW.id,chunk_no,document FROM message_search_documents(NEW.content);
  RETURN NULL;
END
$$;
DROP TRIGGER messages_refresh_search ON messages;
CREATE TRIGGER messages_refresh_search AFTER INSERT OR UPDATE OF content,status ON messages
  FOR EACH ROW EXECUTE FUNCTION refresh_message_search_chunks();
