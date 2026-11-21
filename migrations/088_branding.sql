-- The rest of the site's branding as an administrator set it in Administration: the
-- words on the home and sign-in pages, the links, the icons, and the logo and tab
-- icon as uploaded images. `branding` holds only what was changed, in the shape of
-- branding/site.config.js; anything not in it stays as that file has it.
ALTER TABLE appearance ADD COLUMN branding JSONB NOT NULL DEFAULT '{}'::jsonb;

-- One image per place it shows. Small, since every page loads them.
CREATE TABLE branding_images (
    slot TEXT PRIMARY KEY CHECK (slot IN ('logo-light', 'logo-dark', 'favicon')),
    content_type TEXT NOT NULL CHECK (content_type IN ('image/png', 'image/jpeg', 'image/webp', 'image/gif', 'image/svg+xml', 'image/x-icon')),
    bytes BYTEA NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 524288),
    etag TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
