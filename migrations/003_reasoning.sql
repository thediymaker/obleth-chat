-- Additive defaults keep existing chats and messages readable during rollout.
ALTER TABLE conversations ADD COLUMN reasoning_effort TEXT NOT NULL DEFAULT 'medium'
    CHECK (reasoning_effort IN ('none', 'low', 'medium', 'high'));
ALTER TABLE messages ADD COLUMN reasoning_content TEXT NOT NULL DEFAULT '';
ALTER TABLE messages ADD CONSTRAINT messages_combined_response_limit
    CHECK (OCTET_LENGTH(content) + OCTET_LENGTH(reasoning_content) <= 1048576);
ALTER TABLE messages ADD CONSTRAINT messages_assistant_reasoning
    CHECK (role = 'assistant' OR reasoning_content = '');
