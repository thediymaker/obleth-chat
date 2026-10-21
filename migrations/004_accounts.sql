ALTER TABLE users ADD COLUMN password_change_required BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE auth_sessions ADD COLUMN id UUID NOT NULL DEFAULT gen_random_uuid();
ALTER TABLE auth_sessions ADD COLUMN user_agent TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX auth_sessions_id_idx ON auth_sessions(id);

CREATE TABLE user_groups (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX user_groups_name_idx ON user_groups(lower(name));
CREATE TABLE user_group_members (
    group_id UUID NOT NULL REFERENCES user_groups(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    managed_oidc BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX user_group_members_user_idx ON user_group_members(user_id, group_id);

CREATE TABLE audit_events (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
    action TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX audit_events_page_idx ON audit_events(created_at DESC, id DESC);

CREATE TABLE account_access_tokens (
    token_hash BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('invite', 'reset')),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX account_access_tokens_user_idx ON account_access_tokens(user_id);
CREATE INDEX account_access_tokens_expiry_idx ON account_access_tokens(expires_at);
