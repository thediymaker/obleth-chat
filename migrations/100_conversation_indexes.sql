-- Deleting a chat, by hand or by retention, makes Postgres look in every table that
-- refers to it. These six had no index it could use for that (two had only an index
-- over their pending rows), so each delete read them from end to end.
CREATE INDEX tool_approvals_conversation_id_idx ON tool_approvals (conversation_id);
CREATE INDEX device_calls_conversation_id_idx ON device_calls (conversation_id);
CREATE INDEX steer_notes_conversation_id_idx ON steer_notes (conversation_id);
CREATE INDEX research_conversation_id_idx ON research (conversation_id)
    WHERE conversation_id IS NOT NULL;
CREATE INDEX saved_items_source_conversation_id_idx ON saved_items (source_conversation_id)
    WHERE source_conversation_id IS NOT NULL;
CREATE INDEX memories_source_conversation_id_idx ON memories (source_conversation_id)
    WHERE source_conversation_id IS NOT NULL;
