-- The browser session a personal API token was made in, so signing that session out
-- from another device also revokes the tokens made in it. Tokens made before this
-- column existed have none.
ALTER TABLE api_tokens ADD COLUMN session_id UUID;
CREATE INDEX api_tokens_session_id_idx ON api_tokens(session_id) WHERE session_id IS NOT NULL;
