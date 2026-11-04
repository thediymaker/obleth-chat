CREATE TABLE model_profiles (
    model_id TEXT PRIMARY KEY CHECK (OCTET_LENGTH(model_id) BETWEEN 1 AND 160),
    name TEXT NOT NULL CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(description) <= 1000),
    tags JSONB NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(tags) = 'array'),
    sort_order INTEGER NOT NULL DEFAULT 0 CHECK (sort_order BETWEEN -10000 AND 10000),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    published BOOLEAN NOT NULL DEFAULT TRUE,
    visibility TEXT NOT NULL DEFAULT 'all' CHECK (visibility IN ('all','admins','groups')),
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    defaults JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(defaults) = 'object' AND OCTET_LENGTH(defaults::text) <= 16384),
    allowed_overrides JSONB NOT NULL DEFAULT '["systemPrompt","temperature","topP","maxTokens","reasoningEffort"]'
        CHECK (jsonb_typeof(allowed_overrides) = 'array'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (NOT is_default OR (enabled AND published))
);
CREATE UNIQUE INDEX model_profiles_one_default ON model_profiles(is_default) WHERE is_default;
CREATE TABLE model_profile_groups (
    model_id TEXT NOT NULL REFERENCES model_profiles(model_id) ON DELETE CASCADE,
    group_id UUID NOT NULL REFERENCES user_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (model_id, group_id)
);
CREATE INDEX model_profile_groups_group ON model_profile_groups(group_id, model_id);
CREATE TABLE user_preferences (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    preferences JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(preferences) = 'object' AND OCTET_LENGTH(preferences::text) <= 65536),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE TABLE user_presets (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 80),
    model TEXT NOT NULL CHECK (OCTET_LENGTH(model) BETWEEN 1 AND 160),
    parameters JSONB NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(parameters) = 'object' AND OCTET_LENGTH(parameters::text) <= 16384),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, name)
);
CREATE INDEX user_presets_owner ON user_presets(user_id, updated_at DESC, id);
