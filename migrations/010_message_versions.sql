-- Preserve the existing linear transcript as the initially selected message path.
-- Alternatives share prior messages and attachments rather than duplicating chats.
ALTER TABLE messages
  ADD COLUMN parent_message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
  ADD COLUMN selected_child_id UUID REFERENCES messages(id) ON DELETE SET NULL;
ALTER TABLE conversations
  ADD COLUMN selected_leaf_id UUID REFERENCES messages(id) ON DELETE SET NULL;
WITH links AS (
  SELECT id,lag(id) OVER (PARTITION BY conversation_id ORDER BY ordinal) AS parent,
    lead(id) OVER (PARTITION BY conversation_id ORDER BY ordinal) AS child FROM messages
)
UPDATE messages m SET parent_message_id=links.parent,selected_child_id=links.child
  FROM links WHERE m.id=links.id;
UPDATE conversations c SET selected_leaf_id=(
  SELECT m.id FROM messages m WHERE m.conversation_id=c.id ORDER BY ordinal DESC LIMIT 1
);
ALTER TABLE messages DROP CONSTRAINT messages_ordinal_check;
ALTER TABLE messages ADD CONSTRAINT messages_ordinal_check CHECK (ordinal>=0 AND ordinal<1000);
CREATE INDEX messages_versions_idx ON messages(conversation_id,parent_message_id,role,ordinal);
