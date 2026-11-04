-- Each passage keeps the headings it sits under ("2 Method › 2.1 Schedule"). Keyword
-- search reads them as part of the text; passages indexed before this have none and
-- rank as they did.
ALTER TABLE knowledge_passages
    ADD COLUMN section TEXT CHECK (char_length(section) <= 1000),
    DROP COLUMN tsv;
ALTER TABLE knowledge_passages
    ADD COLUMN tsv tsvector GENERATED ALWAYS AS (
        to_tsvector('english', coalesce(section, '') || ' ' || content)
    ) STORED;
CREATE INDEX knowledge_passages_tsv_idx ON knowledge_passages USING gin (tsv);
