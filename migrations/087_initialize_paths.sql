-- Chat's own imports and branch, fork and shared-fork copies wrote messages without
-- their parent, selected-child and leaf links, and every read of a chat checked for
-- that and linked it then. Those writers now set the links, so this links the chats
-- still left unlinked, in message order as the read did, and reads stop checking.
-- Only those chats are touched; a second run finds none.
WITH unlinked AS (
  SELECT c.id FROM conversations c
  WHERE c.selected_leaf_id IS NULL
    AND EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id = c.id)
), links AS (
  SELECT m.id,
    lag(m.id) OVER (PARTITION BY m.conversation_id ORDER BY m.ordinal) AS parent,
    lead(m.id) OVER (PARTITION BY m.conversation_id ORDER BY m.ordinal) AS child
  FROM messages m JOIN unlinked u ON u.id = m.conversation_id
)
UPDATE messages m SET parent_message_id = links.parent, selected_child_id = links.child
  FROM links WHERE m.id = links.id;

UPDATE conversations c SET selected_leaf_id = (
  SELECT m.id FROM messages m WHERE m.conversation_id = c.id ORDER BY m.ordinal DESC LIMIT 1
)
WHERE c.selected_leaf_id IS NULL
  AND EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id = c.id);
