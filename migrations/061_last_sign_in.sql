-- When each person last signed in, for the People page. Sessions are deleted on
-- sign-out and expiry, so this keeps it; existing accounts start from their newest session.
ALTER TABLE users ADD COLUMN last_sign_in_at TIMESTAMPTZ;
UPDATE users u SET last_sign_in_at = s.at
FROM (SELECT user_id, MAX(created_at) AS at FROM auth_sessions GROUP BY user_id) s
WHERE s.user_id = u.id;
