-- Signing in with the provider goes back to the page it started from, such as an
-- app's chat page, instead of always the home page.
ALTER TABLE oidc_transactions ADD COLUMN return_to TEXT CHECK (return_to IS NULL OR CHAR_LENGTH(return_to) <= 512);
