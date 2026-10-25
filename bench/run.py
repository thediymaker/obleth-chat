#!/usr/bin/env python3
"""Run Chat benchmark tasks against models over Chat's MCP endpoint.

Each task runs as a new chat. The runner waits for it to finish, sends a task's
follow-up message if it has one, then grades it and appends one JSON line per run
to the results file. Runs already in the results file are skipped, so a stopped
run picks up where it left off.

  OBLETH_TOKENS=obc_a,obc_b bench/run.py --url https://chat.example.com \\
      --models auto,model-a --tags screen --exec ./exec-in-workspace

Each token gets its own share of the concurrency, so tokens for several accounts
spread workspace tasks across several workspace pods. Shell checks need --exec: a
program called as `exec CHAT_ID SCRIPT` that runs SCRIPT in the chat's project
folder with PREVIEW_PORT set, and exits with the script's status.
"""
import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))


class Mcp:
    def __init__(self, url, token):
        self.url = url.rstrip("/") + "/mcp"
        self.token = token
        self.next = 0
        self.lock = threading.Lock()

    def rpc(self, method, params):
        with self.lock:
            self.next += 1
            ident = self.next
        body = json.dumps({"jsonrpc": "2.0", "id": ident, "method": method, "params": params}).encode()
        for attempt in range(6):
            request = urllib.request.Request(self.url, body, {
                "authorization": f"Bearer {self.token}",
                "content-type": "application/json",
                "accept": "application/json, text/event-stream",
            })
            try:
                with urllib.request.urlopen(request, timeout=300) as response:
                    reply = json.load(response)
                break
            except urllib.error.HTTPError as error:
                if error.code in (429, 502, 503, 504) and attempt < 5:
                    time.sleep(5 * (attempt + 1))
                    continue
                raise RuntimeError(f"{method}: HTTP {error.code} {error.read()[:200]!r}")
            except (urllib.error.URLError, TimeoutError) as error:
                if attempt < 5:
                    time.sleep(5 * (attempt + 1))
                    continue
                raise RuntimeError(f"{method}: {error}")
        if "error" in reply:
            raise RuntimeError(f"{method}: {reply['error']['message']}")
        return reply["result"]

    def tool(self, name, **arguments):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise ToolError(result["content"][0]["text"])
        return result["structuredContent"]


class ToolError(Exception):
    pass


def wait(mcp, chat, deadline):
    while True:
        state = mcp.tool("get_chat", id=chat, messages=1)
        if state["state"] != "running":
            return state, False
        if time.time() > deadline:
            mcp.tool("stop_chat", id=chat)
            time.sleep(5)
            return mcp.tool("get_chat", id=chat, messages=1), True
        time.sleep(10)


def last_reply(state):
    messages = [m for m in state["messages"] if m["role"] == "assistant"]
    return messages[-1]["content"] if messages else ""


def grade(check, state, chat, exec_hook):
    kind = check["type"]
    if kind == "state":
        return state["state"] == check["is"], state["state"]
    if kind == "waiting":
        kinds = [w.get("kind") for w in state.get("waitingFor") or []]
        return check["kind"] in kinds, ",".join(map(str, kinds)) or state["state"]
    reply = last_reply(state).strip()
    if kind == "reply_regex":
        return bool(re.search(check["pattern"], reply)), reply[:200]
    if kind == "reply_json":
        text = re.sub(r"^```\w*\n|\n```$", "", reply)
        try:
            value = json.loads(text)
        except ValueError:
            return False, reply[:200]
        return value == check["equals"], text[:200]
    if kind == "shell":
        if not exec_hook:
            return None, "no --exec"
        try:
            done = subprocess.run([exec_hook, chat, check["script"]], capture_output=True, text=True, timeout=300)
        except subprocess.TimeoutExpired:
            return False, "check timed out"
        return done.returncode == 0, (done.stdout + done.stderr)[-300:]
    return None, f"unknown check {kind}"


def run(mcp, task, model, args):
    started = time.time()
    record = {"task": task["id"], "model": model, "account": mcp.token[4:10], "started": started}
    try:
        for attempt in range(30):
            try:
                chat = mcp.tool("start_chat", message=task["message"], model=model,
                                title=f"bench {task['id']} · {model}", workspace=bool(task.get("workspace")))
                break
            except ToolError as error:
                # Full workspace slots or reply limits clear as other runs finish.
                busy = "slots are in use" in str(error) or "responses at once" in str(error)
                if not busy or attempt == 29:
                    raise
                time.sleep(60)
        started = time.time()
        record.update(chat=chat["id"], url=chat["url"])
        deadline = started + args.timeout * 60
        state, timed_out = wait(mcp, chat["id"], deadline)
        if task.get("then") and state["state"] == "completed" and not timed_out:
            mcp.tool("send_message", id=chat["id"], message=task["then"])
            state, timed_out = wait(mcp, chat["id"], deadline)
        record.update(state=state["state"], timedOut=timed_out, seconds=round(time.time() - started),
                      reply=last_reply(state)[:600],
                      error=next((m.get("error") for m in reversed(state["messages"]) if m.get("error")), None))
        checks = []
        for check in task["checks"]:
            passed, detail = grade(check, state, chat["id"], args.exec)
            checks.append({"name": check.get("name", check["type"]), "passed": passed, "detail": detail})
        record["checks"] = checks
        record["passed"] = all(c["passed"] for c in checks if c["passed"] is not None)
    except (RuntimeError, ToolError) as error:
        record.update(state="harness-error", error=str(error), passed=False, seconds=round(time.time() - started))
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", required=True, help="Chat's address")
    parser.add_argument("--models", help="Comma-separated model ids; every available model if omitted")
    parser.add_argument("--skip-models", default="", help="Comma-separated model ids to leave out")
    parser.add_argument("--tasks", help="Comma-separated task ids")
    parser.add_argument("--tags", help="Only tasks with one of these comma-separated tags")
    parser.add_argument("--concurrency", type=int, default=2, help="Chats at once per token")
    parser.add_argument("--timeout", type=float, default=25, help="Minutes per task before it is stopped")
    parser.add_argument("--exec", help="Program that runs a shell check in a chat's project folder")
    parser.add_argument("--out", default="results.jsonl")
    args = parser.parse_args()
    tokens = [t.strip() for t in os.environ.get("OBLETH_TOKENS", os.environ.get("OBLETH_TOKEN", "")).split(",") if t.strip()]
    if not tokens:
        sys.exit("Set OBLETH_TOKENS to one or more Chat API tokens")
    clients = [Mcp(args.url, token) for token in tokens]
    tasks = json.load(open(os.path.join(HERE, "tasks.json")))["tasks"]
    if args.tasks:
        wanted = set(args.tasks.split(","))
        tasks = [t for t in tasks if t["id"] in wanted]
    if args.tags:
        tags = set(args.tags.split(","))
        tasks = [t for t in tasks if tags & set(t["tags"])]
    if args.models:
        models = args.models.split(",")
    else:
        models = [m["id"] for m in clients[0].tool("list_models")["models"]]
    skip = set(filter(None, args.skip_models.split(",")))
    models = [m for m in models if m not in skip]
    done = set()
    if os.path.exists(args.out):
        for line in open(args.out):
            r = json.loads(line)
            if r.get("state") != "harness-error":
                done.add((r["task"], r["model"]))
    # Interleave models so one slow model never holds every slot.
    jobs = [(t, m) for t in tasks for m in models if (t["id"], m) not in done]
    print(f"{len(jobs)} runs: {len(tasks)} tasks × {len(models)} models, {len(done)} already done", flush=True)
    write = threading.Lock()
    queues = [jobs[i::len(clients)] for i in range(len(clients))]

    def drain(client, queue):
        with concurrent.futures.ThreadPoolExecutor(args.concurrency) as pool:
            for record in pool.map(lambda job: run(client, job[0], job[1], args), queue):
                with write:
                    with open(args.out, "a") as out:
                        out.write(json.dumps(record) + "\n")
                    mark = "PASS" if record["passed"] else "FAIL"
                    print(f"{mark} {record['task']:<14} {record['model']:<40} {record.get('state')} {record.get('seconds')}s", flush=True)

    with concurrent.futures.ThreadPoolExecutor(len(clients)) as accounts:
        list(accounts.map(lambda pair: drain(*pair), zip(clients, queues)))


if __name__ == "__main__":
    main()
