-- Any file can be attached: archives, data, binaries, and text too large to read
-- whole are kept as kind 'file'. The model is told about them, never sent them; a
-- chat's workspace gets a copy. Their largest size is set by the server's
-- configuration, so the table keeps only the limit an INTEGER size allows; images
-- and text read whole keep their own limits.
ALTER TABLE attachments DROP CONSTRAINT attachments_kind_check;
ALTER TABLE attachments ADD CONSTRAINT attachments_kind_check
    CHECK (kind IN ('text', 'image', 'document', 'file'));
ALTER TABLE attachments DROP CONSTRAINT attachments_size_check;
ALTER TABLE attachments ADD CONSTRAINT attachments_size_check CHECK (size > 0);
-- For a file, extracted_text holds what the model is shown about it: an archive's
-- list of entries or the start of a text file. parts is an archive's entry count.
