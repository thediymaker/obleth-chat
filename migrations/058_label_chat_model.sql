-- The model a labelled chat used most, so administrators can see what each model's tokens
-- went to. A deleted chat keeps it with its labels.
ALTER TABLE conversation_labels ADD COLUMN chat_model TEXT CHECK (CHAR_LENGTH(chat_model) <= 256);
UPDATE conversation_labels l SET chat_model = (
    SELECT g.model FROM messages m JOIN generation_usage g ON g.generation_id = m.generation_id
    WHERE m.conversation_id = l.conversation_id AND g.model IS NOT NULL
    GROUP BY g.model ORDER BY COALESCE(SUM(g.total_tokens), 0) DESC, COUNT(*) DESC, g.model LIMIT 1)
WHERE l.conversation_id IS NOT NULL;
