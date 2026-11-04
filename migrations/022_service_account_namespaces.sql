-- The token forms used to fill in "default" as the namespace. A service account token
-- usually works only in its own namespace, which is now the default when none is given.
UPDATE connection_credentials
SET options = jsonb_set(options, '{namespace}', to_jsonb(split_part(options->>'identity', ':', 3))),
    updated_at = NOW()
WHERE options->>'namespace' = 'default'
  AND options->>'identity' LIKE 'system:serviceaccount:%:%'
  AND split_part(options->>'identity', ':', 3) NOT IN ('', 'default');

UPDATE connections
SET options = jsonb_set(options, '{namespace}', to_jsonb(split_part(options->>'identity', ':', 3))),
    updated_at = NOW()
WHERE kind = 'kubernetes'
  AND options->>'namespace' = 'default'
  AND options->>'identity' LIKE 'system:serviceaccount:%:%'
  AND split_part(options->>'identity', ':', 3) NOT IN ('', 'default');
