#!/usr/bin/env python3
"""Sends each model's certification score to Chat, where it decides how far the model
is trusted with changes through connections (Admin › Models › Changes through
connections, "Decide from certification").

    CERTIFY_TOKEN=obc_... ./publish.py --base https://chat.example.edu results/

Reads every <model>.jsonl under the folder (as run.py and rescore.py write them) and
PUTs score, passed, total and suite per model. --dry-run prints what would be sent.
"""
import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("folder")
    parser.add_argument("--base", required=True, help="Chat's address")
    parser.add_argument("--suite", default="cases.yaml")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    token = os.environ.get("CERTIFY_TOKEN")
    if not token and not args.dry_run:
        sys.exit("set CERTIFY_TOKEN to a personal API token with write scope")
    counts = defaultdict(lambda: [0, 0])
    for path in sorted(Path(args.folder).glob("**/*.jsonl")):
        with path.open() as source:
            for line in source:
                record = json.loads(line)
                if "passed" not in record or "model" not in record:
                    continue
                counts[record["model"]][1] += 1
                counts[record["model"]][0] += 1 if record["passed"] else 0
    if not counts:
        sys.exit("no records found")
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    for model, (passed, total) in sorted(counts.items()):
        body = {"score": round(100.0 * passed / total, 1), "passed": passed, "total": total, "suite": args.suite, "at": now}
        print(f"{model}: {body['score']}% ({passed}/{total})")
        if args.dry_run:
            continue
        url = args.base.rstrip("/") + "/api/admin/model-certification/" + urllib.parse.quote(model, safe="")
        request = urllib.request.Request(url, data=json.dumps(body).encode(), method="PUT")
        request.add_header("Authorization", "Bearer " + token)
        request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                response.read()
        except urllib.error.HTTPError as error:
            sys.exit(f"{model}: Chat answered {error.code}: {error.read().decode('utf-8', 'replace')[:300]}")


if __name__ == "__main__":
    main()
