-- Usage records name the model that served them, so usage can be shown per model.
-- They outlive the conversations whose messages also held it.
ALTER TABLE generation_usage ADD COLUMN model TEXT CHECK (CHAR_LENGTH(model) <= 256);

-- Earlier records learn their model from the reply, or for suggestions the message they
-- followed, where it still exists.
UPDATE generation_usage g SET model = m.settings->>'model'
FROM messages m
WHERE g.model IS NULL AND g.purpose = 'chat' AND m.generation_id = g.generation_id
  AND CHAR_LENGTH(m.settings->>'model') <= 256;
UPDATE generation_usage g SET model = m.settings->>'model'
FROM messages m
WHERE g.model IS NULL AND g.purpose = 'follow_up' AND m.id = g.generation_id
  AND CHAR_LENGTH(m.settings->>'model') <= 256;
