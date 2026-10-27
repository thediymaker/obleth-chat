-- Usernames taken from the OIDC provider's preferred_username
-- (OBLETH_CHAT_OIDC_PROVIDER_USERNAMES). They follow the provider on each sign-in;
-- names an administrator or bootstrap chose are never changed.
ALTER TABLE users ADD COLUMN username_from_provider BOOLEAN NOT NULL DEFAULT FALSE;
