-- A chat can be shared, read-only and live, with one group or with every signed-in
-- account that has the link. Deleting a group unshares its chats first.
ALTER TABLE conversations
    ADD COLUMN share_scope TEXT NOT NULL DEFAULT 'none' CHECK (share_scope IN ('none', 'group', 'signed_in')),
    ADD COLUMN share_group_id UUID REFERENCES user_groups(id) ON DELETE SET NULL,
    ADD CONSTRAINT conversations_share_group_named CHECK (share_scope <> 'group' OR share_group_id IS NOT NULL);
CREATE INDEX conversations_share_group_idx ON conversations(share_group_id, updated_at DESC) WHERE share_scope = 'group';
