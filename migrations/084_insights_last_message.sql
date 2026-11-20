-- The insights worker looked for chats with messages newer than their labels by
-- taking the newest message of every chat active in the last 30 days, every minute.
-- Each chat now keeps the time of its newest message, set by a trigger on every
-- message insert, so the worker compares two columns instead.
ALTER TABLE conversations ADD COLUMN last_message_at TIMESTAMPTZ;

CREATE FUNCTION conversations_last_message() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE conversations c SET last_message_at = n.at
    FROM (SELECT conversation_id, MAX(created_at) AS at FROM inserted GROUP BY conversation_id) n
    WHERE c.id = n.conversation_id AND (c.last_message_at IS NULL OR c.last_message_at < n.at);
    RETURN NULL;
END $$;

CREATE TRIGGER messages_last_message AFTER INSERT ON messages
    REFERENCING NEW TABLE AS inserted
    FOR EACH STATEMENT EXECUTE FUNCTION conversations_last_message();

-- Chats the worker can still label; older ones are past its 30-day look-back.
UPDATE conversations c SET last_message_at = m.at
FROM (SELECT conversation_id, MAX(created_at) AS at FROM messages GROUP BY conversation_id) m
WHERE c.id = m.conversation_id AND c.updated_at > NOW() - INTERVAL '31 days';

-- The admin summary's newest-label time.
CREATE INDEX conversation_labels_labeled_at_idx ON conversation_labels (labeled_at)
    WHERE labeled_at IS NOT NULL;
