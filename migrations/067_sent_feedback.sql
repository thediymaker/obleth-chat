-- What people send with the sidebar's Feedback button: a problem, an idea or anything
-- else, with the page they were on and Chat's version. Administrators read them on the
-- Feedback tab, and see who sent one only when that person said they may be contacted.
CREATE TABLE sent_feedback (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    kind TEXT NOT NULL CHECK (kind IN ('problem', 'idea', 'other')),
    message TEXT NOT NULL CHECK (char_length(message) BETWEEN 1 AND 4000),
    contact BOOLEAN NOT NULL DEFAULT FALSE,
    page TEXT CHECK (char_length(page) <= 512),
    version TEXT NOT NULL CHECK (char_length(version) <= 40),
    done BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX sent_feedback_created_idx ON sent_feedback (created_at DESC);
CREATE INDEX sent_feedback_user_idx ON sent_feedback (user_id, created_at);

-- How the Feedback button shows: in the sidebar, highlighted there (as while a site is
-- in development), or not at all.
CREATE TABLE feedback_button (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    mode TEXT NOT NULL DEFAULT 'sidebar' CHECK (mode IN ('off', 'sidebar', 'highlighted')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO feedback_button (id) VALUES (TRUE);
