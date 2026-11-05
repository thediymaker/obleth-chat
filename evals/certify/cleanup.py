#!/usr/bin/env python3
"""Deletes the chats certification runs left in the account: those whose title starts
with "certify " (run.py names test chats "certify <case> <model>" and judge chats
"certify judge <case>"). Other chats are never touched. Needs a Chat with delete_chat.

    CERTIFY_TOKEN=obc_... ./cleanup.py --base https://chat.example.edu [--dry-run]
"""
import argparse, os, sys

import run

PREFIX = "certify "


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", required=True, help="Chat's address")
    parser.add_argument("--dry-run", action="store_true", help="count them, delete nothing")
    args = parser.parse_args()
    token = os.environ.get("CERTIFY_TOKEN") or sys.exit("Set CERTIFY_TOKEN to a Chat API token")
    chat = run.Chat(args.base, token)
    if args.dry_run:
        total = chat.call("list_chats", {"query": PREFIX.strip(), "limit": 1})["total"]
        print(f"{total} chats match {PREFIX.strip()!r}; those titled {PREFIX!r}... would be deleted")
        return
    seen = set()
    while True:
        # Deleted chats drop out of the list, so the first page always holds the next ones.
        page = chat.call("list_chats", {"query": PREFIX.strip(), "limit": 50})["chats"]
        found = [c for c in page if (c.get("title") or "").startswith(PREFIX) and c["id"] not in seen]
        if not found:
            break
        for c in found:
            seen.add(c["id"])
            chat.discard(c["id"])
            if run.Chat.cannot_delete:
                sys.exit("This Chat has no delete_chat yet")
    print(f"{len(seen)} certification chats deleted")


if __name__ == "__main__":
    main()
