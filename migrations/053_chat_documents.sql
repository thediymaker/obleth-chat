-- Documents in a chat: PDF, Word, PowerPoint and Excel files attached to a message.
-- Their text is read once, when they are uploaded, and kept with the file.
ALTER TABLE attachments DROP CONSTRAINT attachments_kind_check;
ALTER TABLE attachments ADD CONSTRAINT attachments_kind_check
    CHECK (kind IN ('text', 'image', 'document'));
ALTER TABLE attachments DROP CONSTRAINT attachments_size_check;
ALTER TABLE attachments ADD CONSTRAINT attachments_size_check
    CHECK (size > 0 AND size <= 20971520);
ALTER TABLE attachments ADD CONSTRAINT attachments_image_size_check
    CHECK (kind <> 'image' OR size <= 5242880);
-- The text a model reads, with a marker where each page, slide or sheet starts. NULL
-- for a document too long to read whole, which is searched instead.
ALTER TABLE attachments ADD COLUMN extracted_text TEXT;
-- How many pages, slides or sheets, for showing next to the name.
ALTER TABLE attachments ADD COLUMN parts INTEGER;
ALTER TABLE attachments ADD COLUMN searched BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE attachments ADD CONSTRAINT attachments_document_check
    CHECK (kind <> 'document' OR extracted_text IS NOT NULL OR searched);

-- A collection holding one long attached document, so a chat can search it. It is
-- made when the document is uploaded, so indexing starts at once, and the first chat
-- the document is sent in takes it; it goes with that chat, or with the file if it is
-- never sent. These collections are not listed in Knowledge.
ALTER TABLE knowledge_collections
    ADD COLUMN attachment_id UUID REFERENCES attachments(id) ON DELETE CASCADE,
    ADD COLUMN conversation_id UUID REFERENCES conversations(id) ON DELETE CASCADE;
CREATE INDEX knowledge_collections_attachment_idx ON knowledge_collections(attachment_id)
    WHERE attachment_id IS NOT NULL;
CREATE INDEX knowledge_collections_conversation_idx ON knowledge_collections(conversation_id)
    WHERE conversation_id IS NOT NULL;
-- Its document reads the attachment's bytes instead of keeping an object of its own.
ALTER TABLE knowledge_documents ALTER COLUMN object_key DROP NOT NULL;
