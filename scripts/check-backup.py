#!/usr/bin/env python3
"""Restore a Chat dump into a new disposable database and verify its integrity.

Uses ordinary PGHOST/PGPORT/PGUSER/PGPASSWORD environment variables. Only a local
Postgres host is accepted; PGHOSTADDR, PGSERVICE, and PGSERVICEFILE are refused
because libpq would let them select another server. Never touches an existing
application database.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid


def run(*args, **kwargs):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kwargs).stdout


def remote_connection_setting(environ):
    """Name the setting that could route libpq away from a local server, if any."""
    for name in ("PGHOSTADDR", "PGSERVICE", "PGSERVICEFILE"):
        if environ.get(name):
            return name
    host = environ.get("PGHOST", "")
    # libpq tries every host in a comma-separated list, so accept only one.
    if "," in host or host and host not in ("localhost", "127.0.0.1", "::1") and not host.startswith("/"):
        return "PGHOST"
    return None


def check_message_versions(query, result):
    # Migration010 is optional: older backups and unopened linear imports have
    # no selected path yet. Discover columns before referencing them in SQL.
    expected = {"messages.parent_message_id", "messages.selected_child_id",
                "conversations.selected_leaf_id"}
    columns = set(json.loads(query(
        "SELECT COALESCE(json_agg(table_name||'.'||column_name),'[]'::json) "
        "FROM information_schema.columns WHERE table_schema='public' AND "
        "((table_name='messages' AND column_name IN ('parent_message_id','selected_child_id')) "
        "OR (table_name='conversations' AND column_name='selected_leaf_id'))")))
    if not columns:
        return
    if columns != expected:
        raise RuntimeError("Restored database contains an incomplete message-version schema")
    result["invalidMessageParents"] = int(query(
        "SELECT count(*) FROM public.messages m LEFT JOIN public.messages p ON p.id=m.parent_message_id "
        "WHERE m.parent_message_id IS NOT NULL AND (p.id IS NULL "
        "OR p.conversation_id IS DISTINCT FROM m.conversation_id OR p.ordinal>=m.ordinal)"))
    result["invalidSelectedChildren"] = int(query(
        "SELECT count(*) FROM public.messages m LEFT JOIN public.messages c ON c.id=m.selected_child_id "
        "WHERE m.selected_child_id IS NOT NULL AND (c.id IS NULL "
        "OR c.conversation_id IS DISTINCT FROM m.conversation_id "
        "OR c.parent_message_id IS DISTINCT FROM m.id OR c.ordinal<=m.ordinal)"))
    result["invalidSelectedLeaves"] = int(query(
        "SELECT count(*) FROM public.conversations c LEFT JOIN public.messages m ON m.id=c.selected_leaf_id "
        "WHERE c.selected_leaf_id IS NOT NULL AND (m.id IS NULL "
        "OR m.conversation_id IS DISTINCT FROM c.id OR m.selected_child_id IS NOT NULL)"))
    if any(result[key] for key in ("invalidMessageParents", "invalidSelectedChildren", "invalidSelectedLeaves")):
        raise RuntimeError("Restored message versions contain invalid parent, selected-child, or selected-leaf references")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("backup", type=Path)
    args = parser.parse_args()
    setting = remote_connection_setting(os.environ)
    if setting:
        parser.error(f"Use a local disposable PostgreSQL instance for restore verification ({setting} not accepted)")
    target = args.backup
    manifest = json.loads((target / "manifest.json").read_text())
    if not (target / "COMPLETE").exists() or manifest.get("format") != "obleth-chat-backup" or manifest.get("version") != 1:
        parser.error("Not a complete supported backup")
    for name in ("database.dump", "signing-key.json"):
        if hashlib.sha256((target / name).read_bytes()).hexdigest() != manifest["sha256"].get(name):
            parser.error(f"Checksum failed for {name}")
    # Random, fixed-prefix name is generated here; no operator database name is accepted.
    database = "obleth_restore_check_" + uuid.uuid4().hex
    run("createdb", database)
    try:
        run("pg_restore", "--exit-on-error", "--no-owner", "--no-acl", "--dbname", database, str(target / "database.dump"))

        def query(statement):
            return run("psql", "--no-psqlrc", "--tuples-only", "--no-align", "--set", "ON_ERROR_STOP=1",
                       "--dbname", database, "--command", statement).decode().strip()

        result = json.loads(query("SELECT json_build_object('users',(SELECT count(*) FROM public.users),"
                                  "'conversations',(SELECT count(*) FROM public.conversations),"
                                  "'messages',(SELECT count(*) FROM public.messages),"
                                  "'migrations',(SELECT count(*) FROM public._sqlx_migrations WHERE success))"))
        orphans = int(query("SELECT count(*) FROM public.messages m LEFT JOIN public.conversations c "
                            "ON c.id=m.conversation_id WHERE c.id IS NULL"))
        if orphans:
            raise RuntimeError("Restored database contains orphaned messages")
        result["orphanedMessages"] = orphans

        # Older backups stop at migration003. Discover optional tables before
        # querying them; CASE cannot protect references to nonexistent tables.
        optional = {"attachments": "attachments", "message_attachments": "messageAttachments",
                    "model_profiles": "modelProfiles", "model_profile_groups": "modelProfileGroups",
                    "retention_policy": "retentionPolicy"}
        tables = set(json.loads(query("SELECT COALESCE(json_agg(tablename),'[]'::json) FROM pg_catalog.pg_tables "
                                      "WHERE schemaname='public' AND tablename IN "
                                      "('attachments','message_attachments','model_profiles','model_profile_groups','retention_policy')")))
        if ("attachments" in tables) != ("message_attachments" in tables):
            raise RuntimeError("Restored database contains an incomplete attachment schema")
        for table, key in optional.items():
            if table in tables:
                # Both identifiers come only from the fixed allowlist above.
                result[key] = int(query(f"SELECT count(*) FROM public.{table}"))
        if "attachments" in tables:
            result["attachmentBytes"] = int(query("SELECT COALESCE(SUM(OCTET_LENGTH(data)),0) FROM public.attachments"))
            result["invalidAttachmentFiles"] = int(query(
                "SELECT count(*) FROM public.attachments a LEFT JOIN public.users u ON u.id=a.owner_id "
                "WHERE u.id IS NULL OR OCTET_LENGTH(a.data) IS DISTINCT FROM a.size"))
            result["invalidAttachmentReferences"] = int(query(
                "SELECT count(*) FROM public.message_attachments ma "
                "LEFT JOIN public.messages m ON m.id=ma.message_id "
                "LEFT JOIN public.conversations c ON c.id=m.conversation_id "
                "LEFT JOIN public.attachments a ON a.id=ma.attachment_id "
                "WHERE m.id IS NULL OR c.id IS NULL OR a.id IS NULL "
                "OR a.owner_id IS DISTINCT FROM c.owner_id OR m.role<>'user'"))
            if result["invalidAttachmentFiles"] or result["invalidAttachmentReferences"]:
                raise RuntimeError("Restored attachments have invalid sizes, missing owners, or inconsistent message ownership")
        if "retention_policy" in tables and result["retentionPolicy"] != 1:
            raise RuntimeError("Restored database must contain exactly one retention policy row")
        check_message_versions(query, result)
        print("Restore verified: " + json.dumps(result, sort_keys=True))
    finally:
        run("dropdb", "--if-exists", database)


if __name__ == "__main__":
    main()
