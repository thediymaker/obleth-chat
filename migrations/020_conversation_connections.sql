-- Connections switched off in one conversation. Every other active connection is on.
CREATE TABLE conversation_connections_off (
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, connection_id)
);
