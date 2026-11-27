-- Up to 1.9, switching to a version of a reply that Auto had written copied the
-- version's whole record into the chat's settings: that Auto chose it and why,
-- beside the parameters. Nothing reads those as settings, and their being there made
-- the chat's export, its next message over MCP and Auto mode fail. A chat keeps its
-- parameters only.
UPDATE conversations c
SET settings = COALESCE(
    (SELECT jsonb_object_agg(e.key, e.value)
     FROM jsonb_each(c.settings) AS e
     WHERE e.key IN ('systemPrompt', 'temperature', 'topP', 'maxTokens', 'reasoningEffort')),
    '{}'::jsonb)
WHERE jsonb_typeof(c.settings) = 'object'
  AND EXISTS (
    SELECT 1 FROM jsonb_object_keys(c.settings) AS k
    WHERE k NOT IN ('systemPrompt', 'temperature', 'topP', 'maxTokens', 'reasoningEffort'));
