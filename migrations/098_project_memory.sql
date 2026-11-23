-- Project memories: a memory may belong to one of its owner's projects, and then only
-- that person's chats in the project are given it. It stays its owner's alone, even
-- in a project shared with a group. Without a project a memory is general and every
-- chat of theirs may be given it, as all memories were before.
-- When the project is deleted, its memories become general.
ALTER TABLE memories ADD COLUMN project_id UUID REFERENCES projects(id) ON DELETE SET NULL;
-- For the ON DELETE SET NULL when a project is deleted.
CREATE INDEX memories_project_id_idx ON memories (project_id) WHERE project_id IS NOT NULL;
