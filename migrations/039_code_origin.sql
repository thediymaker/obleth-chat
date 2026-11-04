-- Where a project's code started, when it began from someone else's repository: its
-- address, the commit fetched, and whether its history came along.
ALTER TABLE project_code
    ADD COLUMN origin_url TEXT CHECK (CHAR_LENGTH(origin_url) BETWEEN 1 AND 500),
    ADD COLUMN origin_sha TEXT CHECK (origin_sha ~ '^[0-9a-f]{40}$'),
    ADD COLUMN origin_history BOOLEAN;
