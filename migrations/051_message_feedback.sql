-- A person's thumbs up or down on a reply in their own chat, with the reasons and
-- comment they chose to add. The reply's model is kept so administrators can compare
-- models without reading conversations. A rating goes with its reply, chat or person.
CREATE TABLE message_feedback (
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    rating SMALLINT NOT NULL CHECK (rating IN (-1, 1)),
    reasons TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(reasons) <= 6),
    comment TEXT CHECK (char_length(comment) BETWEEN 1 AND 2000),
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (message_id, user_id)
);
CREATE INDEX message_feedback_updated_idx ON message_feedback (updated_at);
