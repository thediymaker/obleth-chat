-- The identity provider's tokens for a sign-in, kept when Chat forwards people's
-- own access tokens to the gateway (OBLETH_CHAT_GATEWAY_AUTH=forward). Both tokens
-- are sealed with the installation's secret key and bound to this row and person.
-- A grant ends with its browser session.
CREATE TABLE oidc_grants (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id UUID NOT NULL REFERENCES auth_sessions(id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    access_token BYTEA NOT NULL CHECK (OCTET_LENGTH(access_token) <= 16384),
    refresh_token BYTEA NOT NULL CHECK (OCTET_LENGTH(refresh_token) <= 16384),
    access_expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    refreshed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX oidc_grants_session_idx ON oidc_grants(session_id);
CREATE INDEX oidc_grants_user_idx ON oidc_grants(user_id, refreshed_at DESC);

-- Refresh tokens of sign-ins that ended, waiting to be revoked at the provider.
-- Every way a grant ends (sign-out, a session revoked or expired, an account
-- disabled or removed) deletes its row, so a trigger queues the token here.
CREATE TABLE oidc_revocations (
    grant_id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    issuer TEXT NOT NULL,
    refresh_token BYTEA NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    claimed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE FUNCTION oidc_grant_ended() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO oidc_revocations (grant_id, user_id, issuer, refresh_token)
    VALUES (OLD.id, OLD.user_id, OLD.issuer, OLD.refresh_token)
    ON CONFLICT (grant_id) DO NOTHING;
    RETURN OLD;
END
$$;
CREATE TRIGGER oidc_grants_ended AFTER DELETE ON oidc_grants
    FOR EACH ROW EXECUTE FUNCTION oidc_grant_ended();
