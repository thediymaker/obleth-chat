-- Images a model generated in a reply (the gateway's generate_image tool). They
-- arrive as data URLs; Chat keeps the bytes here and the reply links to them.
CREATE TABLE generated_images (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    mime_type TEXT NOT NULL CHECK (mime_type IN ('image/png', 'image/jpeg', 'image/webp', 'image/gif')),
    size INTEGER NOT NULL CHECK (size > 0 AND size <= 8388608),
    data BYTEA NOT NULL CHECK (OCTET_LENGTH(data) = size),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX generated_images_owner_idx ON generated_images(owner_id, created_at);
CREATE INDEX generated_images_conversation_idx ON generated_images(conversation_id);
