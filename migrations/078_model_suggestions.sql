-- A chat on a model not certified for the work it has started (its first tool call
-- on a cluster or in a workspace) is offered a certified model once; what the person
-- did with the offer is kept, so the offer can be judged.
CREATE TABLE model_suggestions (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    suggested TEXT NOT NULL,
    shown_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- switched, kept or never (no more offers for this person)
    outcome TEXT CHECK (outcome IN ('switched', 'kept', 'never')),
    answered_at TIMESTAMPTZ
);
CREATE INDEX model_suggestions_user ON model_suggestions (user_id, shown_at);
