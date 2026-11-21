-- The site's look as an administrator set it in Administration, over what the
-- installation's branding/site.config.js says. A NULL leaves that setting to the file.
-- The accent is the one color that marks what is alive; corners is how round boxes are.
CREATE TABLE appearance (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    site_name TEXT CHECK (char_length(site_name) BETWEEN 1 AND 60),
    accent TEXT CHECK (accent ~ '^#[0-9a-f]{6}$'),
    default_theme TEXT CHECK (default_theme IN ('system', 'light', 'dark')),
    personal_accent BOOLEAN,
    corners TEXT CHECK (corners IN ('sharp', 'soft', 'round')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO appearance (id) VALUES (TRUE);
