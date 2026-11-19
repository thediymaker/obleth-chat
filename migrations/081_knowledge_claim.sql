-- Knowledge uploads now store their object through the same pending-trash protocol
-- as attachments: a trash row marked pending is written before the object, and the
-- document row's insert claims it. Without this trigger the sweep would delete every
-- new knowledge object after the grace period.
CREATE TRIGGER knowledge_documents_claim AFTER INSERT OR UPDATE OF object_key ON knowledge_documents
    FOR EACH ROW EXECUTE FUNCTION claim_object();
