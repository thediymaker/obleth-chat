-- Files can keep their bytes in object storage (OBLETH_CHAT_S3_*): the row then holds
-- the object's key instead of the bytes. Exactly one of the two is set.
ALTER TABLE attachments ALTER COLUMN data DROP NOT NULL;
ALTER TABLE attachments ADD COLUMN object_key TEXT UNIQUE;
-- Finds an identical upload without reading the bytes back.
ALTER TABLE attachments ADD COLUMN sha256 BYTEA;
UPDATE attachments SET sha256 = sha256(data);
ALTER TABLE attachments ALTER COLUMN sha256 SET NOT NULL;
ALTER TABLE attachments DROP CONSTRAINT attachments_check;
ALTER TABLE attachments ADD CONSTRAINT attachments_bytes_check CHECK (
    (data IS NOT NULL AND object_key IS NULL AND OCTET_LENGTH(data) = size)
    OR (data IS NULL AND object_key IS NOT NULL));
CREATE INDEX attachments_owner_sha256_idx ON attachments(owner_id, sha256);

ALTER TABLE generated_images ALTER COLUMN data DROP NOT NULL;
ALTER TABLE generated_images ADD COLUMN object_key TEXT UNIQUE;
ALTER TABLE generated_images DROP CONSTRAINT generated_images_check;
ALTER TABLE generated_images ADD CONSTRAINT generated_images_bytes_check CHECK (
    (data IS NOT NULL AND object_key IS NULL AND OCTET_LENGTH(data) = size)
    OR (data IS NULL AND object_key IS NOT NULL));

-- Objects waiting to be deleted from the bucket, for every kind of file. A row's
-- object is trashed when the row is deleted, including by cascades. An object is
-- also recorded here, as pending, just before it is uploaded, and a row naming it
-- clears that; a pending object no row ever claimed is deleted after a grace period.
CREATE TABLE object_trash (
    object_key TEXT PRIMARY KEY,
    pending BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO object_trash (object_key, created_at)
    SELECT object_key, created_at FROM knowledge_object_trash ON CONFLICT DO NOTHING;

CREATE FUNCTION trash_object() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.object_key IS NOT NULL THEN
        INSERT INTO object_trash (object_key) VALUES (OLD.object_key)
            ON CONFLICT (object_key) DO UPDATE SET pending = FALSE;
    END IF;
    RETURN OLD;
END $$;

CREATE FUNCTION claim_object() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.object_key IS NOT NULL THEN
        DELETE FROM object_trash WHERE object_key = NEW.object_key;
    END IF;
    RETURN NEW;
END $$;

DROP TRIGGER knowledge_documents_trash ON knowledge_documents;
DROP FUNCTION knowledge_trash_object();
DROP TABLE knowledge_object_trash;
CREATE TRIGGER knowledge_documents_trash AFTER DELETE ON knowledge_documents
    FOR EACH ROW EXECUTE FUNCTION trash_object();

CREATE TRIGGER attachments_trash AFTER DELETE ON attachments
    FOR EACH ROW EXECUTE FUNCTION trash_object();
CREATE TRIGGER attachments_claim AFTER INSERT OR UPDATE OF object_key ON attachments
    FOR EACH ROW EXECUTE FUNCTION claim_object();
CREATE TRIGGER generated_images_trash AFTER DELETE ON generated_images
    FOR EACH ROW EXECUTE FUNCTION trash_object();
CREATE TRIGGER generated_images_claim AFTER INSERT OR UPDATE OF object_key ON generated_images
    FOR EACH ROW EXECUTE FUNCTION claim_object();
