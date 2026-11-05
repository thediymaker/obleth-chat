#!/usr/bin/env python3
"""Merges certification results and grades them again with the current checks.

Each results folder holds <model>.jsonl files from run.py. A later folder's record for
the same model and case replaces an earlier one, so a rerun of a few cases can be
layered over a full run. Every record's machine checks are applied again from the
case file. A record that now passes them but was never judged (the old checks failed
it first) is judged with --rejudge, from its saved steps and reply. --rejudge-cases
judges every record of the cases named again, for a rubric that changed.

Writes the merged records and summary.md to --out, and rerun.json: per model, the cases
that could not run, timed out or ran into a busy service, to run again.

    CERTIFY_TOKEN=obc_... ./rescore.py --base https://chat.example.edu --judge glm-5-3 \\
      --judge-for glm-5-2=deepseek-v4-1 --rejudge --out merged/ results/ rerun1/
"""
import argparse, json, os, sys
from pathlib import Path

import yaml

import run


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("folders", nargs="+", help="results folders, earliest first")
    parser.add_argument("--suite", default="cases.yaml")
    parser.add_argument("--out", required=True)
    parser.add_argument("--base", help="Chat's address, for --rejudge")
    parser.add_argument("--judge", help="the model that grades rubrics, for --rejudge")
    parser.add_argument("--judge-for", default="", help="model=judge pairs")
    parser.add_argument("--rejudge", action="store_true", help="judge records the old checks stopped before judging")
    parser.add_argument("--rejudge-cases", default="", help="cases whose rubric changed: judge all their records again")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    judge_for = dict(pair.split("=", 1) for pair in args.judge_for.split(",") if pair)
    again = set(filter(None, args.rejudge_cases.split(",")))
    cases = {c["id"]: c for c in yaml.safe_load((Path(__file__).parent / args.suite).read_text())["cases"]}
    chat = None
    if args.rejudge or again:
        token = os.environ.get("CERTIFY_TOKEN") or sys.exit("Set CERTIFY_TOKEN to a Chat API token")
        if not (args.base and args.judge):
            sys.exit("judging needs --base and --judge")
        chat = run.Chat(args.base, token)

    merged = {}
    for folder in args.folders:
        for path in sorted(Path(folder).glob("**/*.jsonl")):
            for line in path.read_text().splitlines():
                record = json.loads(line)
                if record["id"] in cases:
                    merged[(record["model"], record["id"])] = record

    rerun = {}
    for (model, case_id), record in merged.items():
        case = cases[case_id]
        record["area"] = case["area"]
        if "steps" not in record:  # it never ran
            rerun.setdefault(model, []).append(case_id)
            continue
        failed = run.machine_checks(case["expect"], record["steps"], record.get("reply") or "", record.get("approval", False))
        failed += [f for f in record["failed"] if f.startswith(("still running", "the reply failed"))]
        if any(f.startswith("still running") for f in failed):
            rerun.setdefault(model, []).append(case_id)
        rubric = case["expect"].get("judge")
        verdict = record.get("judge")
        if verdict and (verdict["verdict"] == "the judge was busy" or case_id in again):
            verdict = None
        if not failed and rubric and not verdict and chat:  # with --rejudge or --rejudge-cases
            judge_model = judge_for.get(model, args.judge)
            passed, text = run.judge(chat, judge_model, case, record["steps"], record.get("reply") or "", args.timeout,
                                     record.get("cards", ()), record.get("sent", ()))
            verdict = record["judge"] = {"model": judge_model, "verdict": text}
            print(f"{model} {case_id}: judged {'pass' if passed else 'FAIL'}", flush=True)
        if not failed and rubric:
            if not verdict:
                failed.append("not judged")
            else:
                first = verdict["verdict"].splitlines()[0].upper() if verdict["verdict"] else ""
                if not first.lstrip("*# ").startswith("PASS"):
                    reason = [l for l in verdict["verdict"].splitlines()[1:] if l.strip()]
                    failed.append("judge: " + (reason[0] if reason else verdict["verdict"]))
        record["failed"], record["passed"] = failed, not failed

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    models = sorted({m for m, _ in merged})
    for model in models:
        with (out / f"{model}.jsonl").open("w") as sink:
            for (m, _), record in sorted(merged.items()):
                if m == model:
                    sink.write(json.dumps(record) + "\n")
    areas = dict.fromkeys(c["area"] for c in cases.values())
    table = run.summary(list(merged.values()), models, areas)
    (out / "summary.md").write_text(table + "\n")
    (out / "rerun.json").write_text(json.dumps(rerun, indent=1) + "\n")
    print(table)
    if rerun:
        print(f"to run again: {sum(len(v) for v in rerun.values())} cases over {len(rerun)} models (rerun.json)")


if __name__ == "__main__":
    main()
