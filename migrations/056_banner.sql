-- The banner an administrator shows at the top of every page, such as an announcement
-- or a note that the service is in development. Empty text shows none. signed_out also
-- shows it above the sign-in page; dismissible lets people close it.
CREATE TABLE banner (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    text TEXT NOT NULL DEFAULT '' CHECK (char_length(text) <= 200),
    link_label TEXT CHECK (char_length(link_label) BETWEEN 1 AND 40),
    link_url TEXT CHECK (char_length(link_url) BETWEEN 1 AND 2048),
    tone TEXT NOT NULL DEFAULT 'info' CHECK (tone IN ('info', 'warning')),
    signed_out BOOLEAN NOT NULL DEFAULT FALSE,
    dismissible BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((link_label IS NULL) = (link_url IS NULL))
);
INSERT INTO banner (id) VALUES (TRUE);
