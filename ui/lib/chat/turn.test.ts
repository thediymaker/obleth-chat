import { expect, test } from "vitest";
import { changeOf, duration, gist, latest, runState, stats, steerExample, steerPlaceholder, workOf } from "./turn";
import type { Message } from "./types";

const t = (s: number) => new Date(Date.UTC(2026, 9, 19, 12, 0, s)).toISOString();
const reasoning = "Plan seven files. Then write them.Nothing vendored, so load three.js from a CDN.The script was deferred, so ours ran first.";
const a = "Plan seven files. Then write them.".length,
  b = a + "Nothing vendored, so load three.js from a CDN.".length;
const message: Message = {
  role: "assistant", status: "completed", content: "", reasoningContent: reasoning,
  activity: [
    { id: "thought-1", kind: "thought", from: 0, to: a, durationMs: 12000 },
    { id: "c1", tool: "list_files", summary: "Listed .", status: "completed", startedAt: t(0), durationMs: 100 },
    { id: "c2", tool: "list_files", summary: "Listed vendor", status: "error", detail: "can't cd to vendor", startedAt: t(1), durationMs: 100 },
    { id: "c3", tool: "run_command", summary: "Ran find", status: "error", startedAt: t(2), durationMs: 100 },
    { id: "thought-2", kind: "thought", from: a, to: b, durationMs: 4000 },
    { id: "c4", tool: "write_file", summary: "Wrote index.html", path: "index.html", added: 38, status: "completed", check: "error", startedAt: t(10), durationMs: 200 },
    { id: "thought-3", kind: "thought", from: b, to: reasoning.length, durationMs: 9000 },
    { id: "c5", tool: "edit_file", summary: "Edited index.html", path: "index.html", added: 1, removed: 1, status: "completed", check: "ok", startedAt: t(20), durationMs: 200 },
    { id: "checkpoint-ab12", kind: "checkpoint", sha: "ab12cd34ef56", summary: "Checkpoint 1", stat: "1 file changed", status: "completed" },
    { id: "note-1", kind: "note", summary: "make the mane darker", status: "completed" },
  ],
};

test("thoughts sit before the step they led to, in phases", () => {
  const work = workOf(message);
  expect(work.rows.map((r) => [r.type, r.phase])).toEqual([
    ["thought", "Explore"], ["step", "Explore"], ["deadend", "Explore"], ["thought", "Build"], ["problem", "Build"], ["checkpoint", "Build"], ["note", "Build"],
  ]);
  expect((work.rows[0] as any).text).toBe("Plan seven files. Then write them.");
});

test("failed exploration folds into a dead end, and a broken preview with its fix into a problem", () => {
  const work = workOf(message);
  expect((work.rows[2] as any).tries.length).toBe(2);
  const problem = work.rows[4] as any;
  expect(problem.start.e.id).toBe("c4");
  expect(problem.fix.e.id).toBe("c5");
  expect(problem.middle[0].text).toBe("The script was deferred, so ours ran first.");
});

test("stats add up files, lines, thinking time and checks", () => {
  const s = stats(workOf(message));
  expect(s.files).toEqual([{ path: "index.html", added: 39, removed: 1 }]);
  expect(s.thoughtMs).toBe(25000);
  expect(s.check).toBe("ok");
  expect([s.steps, s.deadEnds, s.fixes, s.checkpoints.length]).toEqual([5, 1, 1, 1]);
  expect(s.span).toBe(20200);
});

test("an older reply shows its one block of reasoning as the first thought", () => {
  const work = workOf({ role: "assistant", content: "", reasoningContent: "Look first.", activity: [{ id: "x", tool: "read_file", summary: "Read a", status: "completed" }] });
  expect(work.rows.map((r) => r.type)).toEqual(["thought", "step"]);
});

test("gists and latest thoughts are short and plain", () => {
  expect(gist("**Stance** must not slip. The hip follows an arcsine.")).toBe("Stance must not slip.");
  expect(gist("x".repeat(400)).length).toBeLessThanOrEqual(160);
  expect(latest("One. Two. Three.", 9)).toBe("Three.");
  expect([duration(400), duration(12000), duration(100000)]).toEqual(["0.4s", "12s", "1m 40s"]);
});

test("cluster work goes through its own stages, not the cluster's name", () => {
  const k8s = { service: "kubernetes", connection: "Lab cluster" };
  const steps: any[] = [
    { id: "k1", tool: "lab__list", action: "list", readOnly: true, status: "completed", ...k8s },
    { id: "k2", tool: "lab__apply", action: "apply", readOnly: false, status: "awaiting", ...k8s },
  ];
  expect(workOf({ role: "assistant", content: "", activity: steps }).rows.map((r) => r.phase)).toEqual(["Inspect", "Approve"]);
  steps[1].status = "completed";
  steps.push({ id: "k3", tool: "lab__get", action: "get", readOnly: true, status: "completed", ...k8s });
  expect(workOf({ role: "assistant", content: "", activity: steps }).rows.map((r) => r.phase)).toEqual(["Inspect", "Apply", "Verify"]);
  const slurm = { service: "slurm", connection: "Campus HPC" };
  const job = [
    { id: "s1", action: "cluster", readOnly: true, status: "completed", ...slurm },
    { id: "s2", action: "submit", readOnly: false, status: "completed", ...slurm },
    { id: "s3", action: "job", readOnly: true, status: "completed", ...slurm },
  ];
  expect(workOf({ role: "assistant", content: "", activity: job }).rows.map((r) => r.phase)).toEqual(["Inspect", "Submit", "Watch"]);
});

test("a change waiting for approval says what it does", () => {
  const yaml = "manifest:\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: hello\n  namespace: default\nspec:\n  replicas: 1\n  template:\n    spec:\n      containers:\n      - name: web\n        image: nginx:1.27\n        ports:\n        - containerPort: 80\n---\nkind: Service\nmetadata:\n  name: hello\nspec:\n  ports:\n  - port: 80";
  const applied = changeOf({ service: "kubernetes", action: "apply", detail: yaml })!;
  expect(applied.items.map((i) => [i.what, i.where])).toEqual([["Deployment/hello", "default"], ["Service/hello", undefined]]);
  expect(applied.items[0]!.facts).toEqual([["Image", "nginx:1.27"], ["Replicas", "1"], ["Port", "80"]]);
  expect(applied.text!.startsWith("apiVersion")).toBe(true);
  const json = changeOf({ service: "kubernetes", action: "apply", detail: 'manifest: {"kind":"ConfigMap","metadata":{"name":"cfg","namespace":"lab"}}' })!;
  expect(json.items.map((i) => [i.what, i.where])).toEqual([["ConfigMap/cfg", "lab"]]);
  const job = changeOf({ service: "slurm", action: "submit", detail: "cpus: 2\nmemory_mb: 4096\nname: heat-bench\npartition: long\nscript:\n#!/bin/bash\nsrun ./heat\ntime_limit_minutes: 90" })!;
  expect(job.items[0]!.what).toBe("heat-bench");
  expect(job.items[0]!.facts).toEqual([["Partition", "long"], ["Time limit", "1 h 30 min"], ["CPUs", "2"], ["Memory", "4 GB"]]);
  expect(job.text).toBe("#!/bin/bash\nsrun ./heat");
  expect(changeOf({ service: "mcp", action: "send", detail: "to: x" })).toBe(null);
});

test("the Steer example fits the work, most specific first", () => {
  const reply = (...activity: any[]): Message => ({ role: "assistant", content: "", status: "streaming", activity });
  const step = (tool: string, more = {}) => ({ id: tool, tool, status: "completed", ...more });
  expect(steerExample(reply())).toBe("keep it short");
  expect(steerExample(null)).toBe("keep it short");
  expect(steerExample(reply(step("write_file")))).toBe("add tests before you finish");
  expect(steerExample(reply(step("browser_open")))).toBe("check the pricing page too");
  expect(steerExample(reply(step("run_command"), step("kubernetes__list", { connection: "k", service: "kubernetes" })))).toBe("use 2 replicas instead");
  expect(steerExample(reply(step("run_command"), step("sol__jobs", { connection: "s", service: "slurm" })))).toBe("ask for 4 hours, not 1");
  expect(steerExample(reply(step("sol__jobs", { connection: "s", service: "slurm" }), step("experiment_run")))).toBe("try a larger batch size next");
  expect(steerPlaceholder(reply())).toBe("Add a note without stopping, like “keep it short”");
});

test("the live card says what it is doing now and where it is in its stages", () => {
  const live: Message = {
    role: "assistant", status: "streaming", content: "", reasoningContent: "Look at the tree first.",
    activity: [
      { id: "c1", tool: "list_files", summary: "Listed .", status: "completed", startedAt: t(0), durationMs: 100 },
      { id: "c2", tool: "write_file", summary: "Writing app.js", status: "running", startedAt: t(1) },
    ],
    _draft: { id: "c3", path: "styles.css", lines: 12, tail: "body {" },
  };
  const s = runState(live, workOf(live));
  expect(s.headline).toBe("Writing styles.css");
  expect(s.pane).toMatchObject({ kind: "draft", label: "styles.css", note: "12 lines so far" });
  expect(s.rail.map((r) => [r.phase, r.status])).toEqual([["Explore", "done"], ["Build", "now"], ["Run", "next"]]);
  expect(s.recent.filter(Boolean).length).toBe(2);
  const asking: Message = { ...live, _draft: null, activity: [{ id: "k", tool: "lab__apply", service: "kubernetes", action: "apply", readOnly: false, connection: "Lab", status: "awaiting", detail: 'manifest: {"kind":"Pod","metadata":{"name":"p"}}' }] };
  const ask = runState(asking, workOf(asking));
  expect(ask.headline).toBe("Waiting for you: apply Pod/p");
  expect(ask.rail.find((r) => r.status === "ask")?.phase).toBe("Approve");
});
