-- A chat delete cascades to its messages, and each deleted message fires the ON
-- DELETE SET NULL check on every column that points at messages. None of those
-- columns led an index, so each check was a full scan of its table: about five
-- scans per message, under the owner's row lock and the retention worker's 10 s
-- statement timeout. The indexes are partial: only rows that point somewhere.
--
-- Built the ordinary way, not CONCURRENTLY: a concurrent build waits for every
-- other session's transaction, and a second replica starting at the same time
-- waits for sqlx's migration lock, which deadlocks (and leaves an invalid index
-- behind). The plain build holds a share lock on each table for the seconds the
-- index takes, during which writes to that table wait.
CREATE INDEX messages_parent_idx
    ON messages (parent_message_id) WHERE parent_message_id IS NOT NULL;
CREATE INDEX messages_selected_child_idx
    ON messages (selected_child_id) WHERE selected_child_id IS NOT NULL;
CREATE INDEX conversations_selected_leaf_idx
    ON conversations (selected_leaf_id) WHERE selected_leaf_id IS NOT NULL;
CREATE INDEX saved_items_source_message_idx
    ON saved_items (source_message_id) WHERE source_message_id IS NOT NULL;
CREATE INDEX memories_source_message_idx
    ON memories (source_message_id) WHERE source_message_id IS NOT NULL;
-- The file-moving worker looks for one row still holding its bytes every pass;
-- once every file is in the bucket that is a look at an empty index instead of a
-- scan of the whole table.
CREATE INDEX attachments_unmoved_idx ON attachments (id) WHERE data IS NOT NULL;
CREATE INDEX generated_images_unmoved_idx ON generated_images (id) WHERE data IS NOT NULL;
