-- Memory: what Chat learns about each person across chats, kept as short statements
-- they can see, change and delete, each linked to the topics it is about.

CREATE TABLE memory_settings (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    -- Whether memory starts on for people who have not chosen.
    default_on BOOLEAN NOT NULL DEFAULT TRUE,
    -- The model that reads new messages and keeps the memories up to date.
    model TEXT CHECK (char_length(model) <= 160),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO memory_settings DEFAULT VALUES;

-- Each person's own choices; no row means the defaults.
CREATE TABLE memory_choices (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    -- Chats get the memories that fit the message.
    use_memory BOOLEAN NOT NULL,
    -- Chats are read for new memories.
    learn BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE memories (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('about', 'work', 'preference', 'goal')),
    content TEXT NOT NULL CHECK (char_length(content) BETWEEN 1 AND 500),
    -- learned from a chat, or added by the person.
    origin TEXT NOT NULL CHECK (origin IN ('learned', 'added')),
    -- Given to every chat, and never changed by the model.
    pinned BOOLEAN NOT NULL DEFAULT FALSE,
    source_conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    source_message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    -- Filled when saved, or by the memory worker if the embedding model was busy.
    embedding halfvec(1024),
    embedding_attempts SMALLINT NOT NULL DEFAULT 0,
    tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED,
    uses INTEGER NOT NULL DEFAULT 0,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX memories_owner_idx ON memories(owner_id, created_at DESC);
CREATE INDEX memories_tsv_idx ON memories USING gin (tsv);

CREATE TABLE memory_topics (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 60),
    kind TEXT NOT NULL CHECK (kind IN ('project', 'tool', 'field', 'course', 'topic')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX memory_topics_name_idx ON memory_topics(owner_id, lower(name));

CREATE TABLE memory_links (
    memory_id UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    topic_id UUID NOT NULL REFERENCES memory_topics(id) ON DELETE CASCADE,
    PRIMARY KEY (memory_id, topic_id)
);
CREATE INDEX memory_links_topic_idx ON memory_links(topic_id);

-- How far each chat has been read for memories, and the claim of a pass reading it.
CREATE TABLE memory_reads (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_through INTEGER NOT NULL DEFAULT -1,
    claimed_until TIMESTAMPTZ,
    attempts SMALLINT NOT NULL DEFAULT 0,
    failed_at TIMESTAMPTZ
);
