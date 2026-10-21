-- Projects: what a chat starts with. Instructions, collections and saved items that
-- every chat in the project gets, shared with a group if the owner chooses. Each
-- person's chats in a project stay their own.
CREATE TABLE projects (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 1000),
    instructions TEXT NOT NULL DEFAULT '' CHECK (char_length(instructions) <= 20000),
    color TEXT NOT NULL DEFAULT 'blue' CHECK (color IN ('blue', 'green', 'gold', 'rose', 'violet', 'gray')),
    -- personal: the owner; group: the owner and members of group_id, who can use it.
    scope TEXT NOT NULL DEFAULT 'personal' CHECK (scope IN ('personal', 'group')),
    group_id UUID REFERENCES user_groups(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX projects_owner_idx ON projects(owner_id, updated_at DESC);
CREATE INDEX projects_group_idx ON projects(group_id) WHERE scope = 'group';

CREATE TABLE project_collections (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    collection_id UUID NOT NULL REFERENCES knowledge_collections(id) ON DELETE CASCADE,
    PRIMARY KEY (project_id, collection_id)
);

-- A conversation belongs to at most one project. Deleting the project keeps its chats.
ALTER TABLE conversations ADD COLUMN project_id UUID REFERENCES projects(id) ON DELETE SET NULL;
CREATE INDEX conversations_project_idx ON conversations(project_id, owner_id) WHERE project_id IS NOT NULL;

-- Saved items can belong to a project; members of a shared project see its items.
ALTER TABLE saved_items ADD COLUMN project_id UUID REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE saved_items ADD COLUMN pinned BOOLEAN NOT NULL DEFAULT FALSE;
CREATE INDEX saved_items_project_idx ON saved_items(project_id, created_at DESC) WHERE project_id IS NOT NULL;
