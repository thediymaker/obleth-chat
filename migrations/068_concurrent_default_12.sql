-- People may run 12 replies at once by default, up from 3, so several chats can run
-- side by side. A site still at the old default moves to the new
-- one; a site that chose another number keeps it.
ALTER TABLE site_limits ALTER COLUMN limits SET DEFAULT '{"dailyRequests":null,"dailyTokens":null,"concurrentRequests":12}';
UPDATE site_limits SET limits=jsonb_set(limits,'{concurrentRequests}','12'),updated_at=NOW()
    WHERE limits->'concurrentRequests'='3'::jsonb;
