CREATE TABLE users (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'admin')),
    password_hash TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (username = LOWER(username))
);

-- Raw browser cookies are never persisted. Revocation takes effect on all replicas.
CREATE TABLE auth_sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX auth_sessions_user_id_idx ON auth_sessions(user_id);
CREATE INDEX auth_sessions_expiry_idx ON auth_sessions(expires_at);

-- No automatic linking by email: the issuer + subject pair is the identity.
CREATE TABLE oidc_identities (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (issuer, subject)
);
CREATE INDEX oidc_identities_user_id_idx ON oidc_identities(user_id);

CREATE TABLE oidc_transactions (
    state_hash BYTEA PRIMARY KEY,
    browser_hash BYTEA NOT NULL,
    nonce TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX oidc_transactions_expiry_idx ON oidc_transactions(expires_at);

-- Counters are shared by replicas. Keys contain a hash of the normalized username.
CREATE TABLE login_throttles (
    key TEXT PRIMARY KEY,
    window_start BIGINT NOT NULL,
    attempts INTEGER NOT NULL
);
CREATE INDEX login_throttles_window_idx ON login_throttles(window_start);
