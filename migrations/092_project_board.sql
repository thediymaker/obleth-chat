-- A project's board: cards that the people in the project and the assistant make
-- and work through, from an idea to done. Cards belong to the project: whoever may
-- use the project may see and change them.
CREATE TABLE project_cards (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (CHAR_LENGTH(title) BETWEEN 1 AND 200),
    notes TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(notes) <= 4000),
    -- A short label of the people's own words, such as Experiment, Writing or Level.
    kind TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(kind) <= 40),
    -- suggested: the assistant proposed it unasked, to keep or dismiss; idea: Ideas;
    -- next: Up next; doing: Now; waiting: Needs you; done; dismissed: out of sight.
    status TEXT NOT NULL DEFAULT 'idea' CHECK (status IN ('suggested', 'idea', 'next', 'doing', 'waiting', 'done', 'dismissed')),
    -- The order within its column, lowest first.
    position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    made_by TEXT NOT NULL CHECK (made_by IN ('person', 'assistant')),
    -- The person who made it, or in whose chat the assistant made it.
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    -- Who is on it now: the assistant (in chat_id, a chat of worker_id's) or the
    -- person worker_id.
    worker TEXT CHECK (worker IN ('person', 'assistant')),
    worker_id UUID REFERENCES users(id) ON DELETE SET NULL,
    chat_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    source_chat_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    -- [{text, state: todo|now|done}], at most 12.
    steps JSONB NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(steps) = 'array' AND jsonb_array_length(steps) <= 12 AND OCTET_LENGTH(steps::text) <= 16384),
    progress TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(progress) <= 200),
    -- For a card in Needs you: what it asks, up to four answers to pick from, and
    -- the answer given.
    question TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(question) <= 500),
    options JSONB NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(options) = 'array' AND jsonb_array_length(options) <= 4 AND OCTET_LENGTH(options::text) <= 2048),
    answer TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(answer) <= 500),
    answered_at TIMESTAMPTZ,
    -- [{kind, label, ref}]: jobs, runs, transfers or paper sections the card touched.
    links JSONB NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(links) = 'array' AND jsonb_array_length(links) <= 20 AND OCTET_LENGTH(links::text) <= 65536),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    done_at TIMESTAMPTZ
);
CREATE INDEX project_cards_project_id_status_position_idx ON project_cards (project_id, status, position);
-- For the chat a card is worked in, and the ON DELETE SET NULL checks when a chat
-- or a person is deleted.
CREATE INDEX project_cards_chat_id_idx ON project_cards (chat_id) WHERE chat_id IS NOT NULL;
CREATE INDEX project_cards_source_chat_id_idx ON project_cards (source_chat_id) WHERE source_chat_id IS NOT NULL;
CREATE INDEX project_cards_created_by_idx ON project_cards (created_by) WHERE created_by IS NOT NULL;
CREATE INDEX project_cards_worker_id_idx ON project_cards (worker_id) WHERE worker_id IS NOT NULL;

-- What happened to a card, newest last, as its open card shows it. The API keeps
-- the latest 50 per card.
CREATE TABLE project_card_events (
    id UUID PRIMARY KEY,
    card_id UUID NOT NULL REFERENCES project_cards(id) ON DELETE CASCADE,
    action TEXT NOT NULL CHECK (action IN ('made', 'moved', 'picked', 'asked', 'answered', 'done')),
    actor TEXT NOT NULL CHECK (actor IN ('person', 'assistant')),
    actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
    from_status TEXT CHECK (from_status IN ('suggested', 'idea', 'next', 'doing', 'waiting', 'done', 'dismissed')),
    to_status TEXT CHECK (to_status IN ('suggested', 'idea', 'next', 'doing', 'waiting', 'done', 'dismissed')),
    -- The answer given, or the question asked.
    detail TEXT NOT NULL DEFAULT '' CHECK (CHAR_LENGTH(detail) <= 500),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX project_card_events_card_id_created_at_idx ON project_card_events (card_id, created_at);
CREATE INDEX project_card_events_actor_id_idx ON project_card_events (actor_id) WHERE actor_id IS NOT NULL;
