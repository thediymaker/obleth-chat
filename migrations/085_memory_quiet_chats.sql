-- Memory reads a chat once it has been quiet for a few minutes, looking among the
-- chats whose newest message is from the last day.
CREATE INDEX conversations_last_message_idx ON conversations (last_message_at)
    WHERE last_message_at IS NOT NULL;
