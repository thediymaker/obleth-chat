#!/usr/bin/env python3
"""Certify models on Chat's workflows.

Runs each case in cases.yaml as a real chat through Chat's MCP endpoint, grades the
machine checks from the chat's steps and text, and has a judge model grade the
rubric, also as a chat. Writes one JSON line per case and model, and a score table.

    CERTIFY_TOKEN=obc_... ./run.py --base https://chat.example.edu \\
        --models glm-5-3,deepseek-v4-1 --judge deepseek-v4-1 --out results/

The token is a personal API token with write scope for an account set up with the
fixtures (see README.md). Only the standard library and PyYAML are needed.
"""
import argparse
import concurrent.futures
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from collections import defaultdict
from pathlib import Path

import yaml

# Steps Chat itself adds; they are not the model's tool calls.
HOUSEKEEPING = {"thinking_limit", "connection", "output_limit", "check"}
# Connection tools that change things. A case must never see one run: the cluster
# connections must ask before changes, and the runner stops a chat at the approval.
WRITES = ("__apply", "__delete", "__submit", "__cancel")
# Activity kinds that are cards the reply ends with.
CARDS = {"question", "workspace", "research", "app", "browser-request", "approval"}
TERMINAL = {"completed", "error", "stopped", "waiting", "idle", "failed"}
# Errors that say the service was busy, not that the model did anything wrong: the
# case is run again after a pause instead of failing.
BUSY = ("stayed full", "responses at once", "Try again in a few minutes")
ATTEMPTS = 6
PAUSE = 90


class Chat:
    def __init__(self, base, token):
        self.url = base.rstrip("/") + "/mcp"
        self.token = token
        self.next_id = 0

    def call(self, name, arguments):
        self.next_id += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self.next_id, "method": "tools/call",
                           "params": {"name": name, "arguments": arguments}}).encode()
        request = urllib.request.Request(self.url, data=body, method="POST", headers={
            "Authorization": f"Bearer {self.token}", "Content-Type": "application/json"})
        for attempt in range(5):
            try:
                with urllib.request.urlopen(request, timeout=60) as response:
                    message = json.load(response)
                break
            except urllib.error.HTTPError as error:
                # The token has a per-minute limit; wait it out.
                if error.code == 429 and attempt < 4:
                    time.sleep(15)
                    continue
                raise
        if "error" in message:
            raise RuntimeError(message["error"]["message"])
        result = message["result"]
        if result.get("isError"):
            raise RuntimeError(result["content"][0]["text"])
        return result["structuredContent"]

    def wait(self, chat_id, timeout):
        deadline = time.time() + timeout
        while True:
            state = self.call("get_chat", {"id": chat_id, "messages": 1})
            if state["state"] != "running":
                return state
            # A change waiting for Allow is as far as a case goes: stop it so it never runs.
            if any(s.get("status") == "awaiting" for s in state.get("steps", [])):
                self.call("stop_chat", {"id": chat_id})
                awaiting = state["steps"]
                state = self.call("get_chat", {"id": chat_id, "messages": 1})
                state["approval"] = True
                state["steps"] = state.get("steps") or awaiting
                return state
            if time.time() > deadline:
                self.call("stop_chat", {"id": chat_id})
                state = self.call("get_chat", {"id": chat_id, "messages": 1})
                state["timedOut"] = True
                return state
            time.sleep(5)


def reply_text(state):
    messages = [m for m in state.get("messages", []) if m["role"] == "assistant"]
    return messages[-1]["content"] if messages else ""


def tools_called(steps):
    return [s["tool"] for s in steps if s.get("tool") and s["tool"] not in HOUSEKEEPING
            and s.get("kind") not in CARDS]


def card_shown(steps, approval=False):
    if approval or any(s.get("status") == "awaiting" for s in steps):
        return "approval"
    kinds = [s["kind"] for s in steps if s.get("kind") in CARDS]
    return kinds[-1] if kinds else "none"


def matches(pattern, tool):
    """A tool name, "*__name" for a connection's tool, or "*__*" for any connection tool."""
    if pattern.startswith("*") and pattern.endswith("*"):
        return pattern[1:-1] in tool
    return tool.endswith(pattern[1:]) if pattern.startswith("*") else tool == pattern


def machine_checks(expect, steps, text, approval=False):
    """The failed machine checks, as short reasons."""
    failed = []
    called = tools_called(steps)
    asked = [s["tool"] for s in steps if s.get("tool")]
    for pattern in expect.get("calls", []):
        if not any(matches(pattern, t) for t in asked):
            failed.append(f"did not call {pattern}")
    no_calls = expect.get("no_calls", [])
    if no_calls == "any":
        if called:
            failed.append(f"called {', '.join(called)} but should call no tool")
    else:
        for pattern in no_calls:
            hit = [t for t in asked if matches(pattern, t)]
            if hit:
                failed.append(f"called {hit[0]}")
    shown = card_shown(steps, approval)
    if "card" in expect and shown != expect["card"]:
        failed.append(f"ended with {shown}, not {expect['card']}")
    lowered = text.lower()
    for needle in expect.get("contains", []):
        if needle.lower() not in lowered:
            failed.append(f"reply lacks {needle!r}")
    for needle in expect.get("not_contains", []):
        if needle.lower() in lowered:
            failed.append(f"reply has {needle!r}")
    return failed


JUDGE_PROMPT = """You grade one reply from an AI assistant inside a chat app against a rubric.
Answer with PASS or FAIL on the first line, then one sentence saying why.

The person wrote:
<<<
{message}
>>>

The assistant's steps (tools it called and cards it showed): {steps}

The assistant's final reply:
<<<
{reply}
>>>

Rubric: {rubric}"""


def judge(chat, model, case, steps, text, timeout):
    prompt = JUDGE_PROMPT.format(message=case["message"], steps=json.dumps(steps),
                                 reply=text[:12000], rubric=case["expect"]["judge"])
    for attempt in range(ATTEMPTS):
        try:
            started = chat.call("start_chat", {"message": prompt, "model": model,
                                               "title": f"certify judge {case['id']}", "connections": []})
        except RuntimeError as error:
            if busy(str(error)) and attempt < ATTEMPTS - 1:
                time.sleep(PAUSE)
                continue
            raise
        state = chat.wait(started["id"], timeout)
        errors = [m["error"] for m in state.get("messages", []) if m.get("error")]
        if errors and busy(errors[-1]) and attempt < ATTEMPTS - 1:
            time.sleep(PAUSE)
            continue
        verdict = reply_text(state).strip()
        first = verdict.splitlines()[0].upper() if verdict else ""
        return first.lstrip("*# ").startswith("PASS"), verdict[:400]
    return False, "the judge was busy"


def answer_for(turn, state):
    """A later turn's message: "pick:N" answers the open question card with option N."""
    found = re.fullmatch(r"pick:(\d+)", turn)
    if not found:
        return turn
    waiting = [w for w in state.get("waitingFor", []) if w.get("questions")]
    if not waiting:
        raise RuntimeError("no question card to answer")
    # Every question in the card gets option N, or its last option when it has fewer.
    n = int(found.group(1)) - 1
    answers = []
    for question in waiting[-1]["questions"]:
        options = question["options"]
        answers.append(f"{question['question']} {options[min(n, len(options) - 1)]['label']}")
    return "\n".join(answers)


def busy(error):
    return any(b in (error or "") for b in BUSY)


def run_case(chat, model, case, config, args):
    """Runs a case, again after a pause while the model or the account is busy."""
    for attempt in range(1, ATTEMPTS + 1):
        record = run_once(chat, model, case, config, args)
        if not record.get("busy") or attempt == ATTEMPTS:
            record["attempts"] = attempt
            return record
        time.sleep(PAUSE)


def run_once(chat, model, case, config, args):
    setup = case.get("setup", {})
    arguments = {"message": case["message"], "model": model,
                 "title": f"certify {case['id']} {model}",
                 "connections": [config["connections"][setup["connection"]]] if "connection" in setup else []}
    if setup.get("workspace"):
        arguments["workspace"] = True
    if setup.get("browser"):
        arguments["browser"] = True
    if "project" in setup:
        arguments["project"] = config["projects"][setup["project"]]
    record = {"id": case["id"], "area": case["area"], "model": model}
    started_at = time.time()
    try:
        started = chat.call("start_chat", arguments)
        record["chat"] = started.get("url")
        state = chat.wait(started["id"], args.timeout)
        for turn in case.get("turns", []):
            chat.call("send_message", {"id": started["id"], "message": answer_for(turn, state)})
            state = chat.wait(started["id"], args.timeout)
    except Exception as error:  # A case that could not run fails with the reason.
        record.update(passed=False, failed=[f"could not run: {error}"], seconds=round(time.time() - started_at), busy=busy(str(error)))
        return record
    steps, text = state.get("steps", []), reply_text(state)
    ran = [s["tool"] for s in steps if (s.get("tool") or "").endswith(WRITES) and s.get("status") not in ("awaiting", "declined")]
    if ran:
        # A change ran without stopping at Allow: the connection doesn't ask first.
        sys.exit(f"{model} {case['id']}: {ran[0]} ran without an approval. Turn on Ask before changes for that connection before running again.")
    failed = machine_checks(case["expect"], steps, text, state.get("approval", False))
    if state.get("timedOut"):
        failed.append(f"still running after {args.timeout} s")
    errors = [m["error"] for m in state.get("messages", []) if m.get("error")]
    if errors and busy(errors[-1]):
        record.update(passed=False, failed=[f"busy: {errors[-1]}"], busy=True, seconds=round(time.time() - started_at))
        return record
    if state["state"] == "failed" or errors:
        failed.append("the reply failed: " + (errors[-1] if errors else state["state"]))
    if not failed and case["expect"].get("judge"):
        judge_model = args.judge_for.get(model, args.judge)
        passed, verdict = judge(chat, judge_model, case, steps, text, args.timeout)
        record["judge"] = {"model": judge_model, "verdict": verdict}
        if not passed:
            reason = [line for line in verdict.splitlines()[1:] if line.strip()]
            failed.append("judge: " + (reason[0] if reason else verdict))
    record.update(passed=not failed, failed=failed, steps=steps, reply=text[:4000],
                  seconds=round(time.time() - started_at))
    return record


def summary(records, models, areas):
    by = defaultdict(lambda: [0, 0])
    for r in records:
        for key in ((r["model"], r["area"]), (r["model"], "total")):
            by[key][1] += 1
            by[key][0] += r["passed"]
    columns = list(areas) + ["total"]
    lines = ["| Model | " + " | ".join(columns) + " |", "|---|" + "---|" * len(columns)]
    ranked = sorted(models, key=lambda m: -by[(m, "total")][0])
    for m in ranked:
        cells = [f"{by[(m, a)][0]}/{by[(m, a)][1]}" if by[(m, a)][1] else "" for a in columns]
        lines.append(f"| {m} | " + " | ".join(cells) + " |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", required=True, help="Chat's address")
    parser.add_argument("--models", help="comma-separated model ids; every listed model otherwise")
    parser.add_argument("--judge", required=True, help="the model that grades rubrics")
    parser.add_argument("--judge-for", default="", help="model=judge pairs, so no model grades itself")
    parser.add_argument("--cases", help="comma-separated case ids or areas to run")
    parser.add_argument("--config", default=str(Path(__file__).with_name("config.yaml")))
    parser.add_argument("--out", default="results")
    parser.add_argument("--parallel", type=int, default=2, help="cases at once per model; each may also start a judge chat")
    parser.add_argument("--timeout", type=int, default=600, help="seconds a turn may run")
    args = parser.parse_args()
    args.judge_for = dict(pair.split("=", 1) for pair in args.judge_for.split(",") if pair)
    token = os.environ.get("CERTIFY_TOKEN") or sys.exit("Set CERTIFY_TOKEN to a Chat API token")
    chat = Chat(args.base, token)
    here = Path(__file__).parent
    cases = yaml.safe_load((here / "cases.yaml").read_text())["cases"]
    if args.cases:
        wanted = set(args.cases.split(","))
        cases = [c for c in cases if c["id"] in wanted or c["area"] in wanted]
    config = yaml.safe_load(Path(args.config).read_text())
    models = args.models.split(",") if args.models else [m["id"] for m in chat.call("list_models", {})["models"] if m["id"] != "auto"]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    records = []
    for model in models:
        path = out / f"{model}.jsonl"
        with concurrent.futures.ThreadPoolExecutor(args.parallel) as pool, path.open("w") as sink:
            # Each worker gets its own client so request ids stay apart.
            futures = [pool.submit(run_case, Chat(args.base, token), model, c, config, args) for c in cases]
            for future in concurrent.futures.as_completed(futures):
                record = future.result()
                records.append(record)
                sink.write(json.dumps(record) + "\n")
                sink.flush()
                mark = "pass" if record["passed"] else "FAIL " + "; ".join(record["failed"])
                print(f"{model} {record['id']}: {mark}", flush=True)
    areas = dict.fromkeys(c["area"] for c in cases)
    table = summary(records, models, areas)
    (out / "summary.md").write_text(table + "\n")
    print(table)


if __name__ == "__main__":
    main()
