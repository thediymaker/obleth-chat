#!/bin/sh
set -eu

# Official PostgreSQL entrypoint runs this only when the data volume is empty.
# The app owns its database and can migrate it, but has no cluster admin powers,
# so the vector extension Knowledge needs is created here.
: "${OBLETH_CHAT_POSTGRES_PASSWORD:?Set the Chat database password}"
psql --set=ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres <<'SQL'
\getenv app_password OBLETH_CHAT_POSTGRES_PASSWORD
CREATE ROLE obleth_chat LOGIN PASSWORD :'app_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION;
CREATE DATABASE obleth_chat OWNER obleth_chat;
\connect obleth_chat
CREATE EXTENSION IF NOT EXISTS vector;
SQL
