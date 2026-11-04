-- Shares left pointing at a deleted group go back to their owner alone, and a group
-- share always names its group. Deleting a group unshares what it held first.
UPDATE projects SET scope = 'personal' WHERE scope = 'group' AND group_id IS NULL;
UPDATE knowledge_collections SET scope = 'personal' WHERE scope = 'group' AND group_id IS NULL;
ALTER TABLE projects ADD CONSTRAINT projects_group_named CHECK (scope <> 'group' OR group_id IS NOT NULL);
ALTER TABLE knowledge_collections ADD CONSTRAINT knowledge_collections_group_named CHECK (scope <> 'group' OR group_id IS NOT NULL);
