// How an assistant reply that used tools shows its work. While it runs: one live
// card (what it is doing now, its latest thought, the files it plans, a Steer box).
// When it is done: what changed and whether it works, checkpoints, and a "Show
// work" trace in which each thought sits before the step it led to.

import type { ActivityEntry, Message } from "./types";

export const TOOL_ICONS: Record<string, string> = {
  search_knowledge: "book", write_file: "edit", edit_file: "edit", read_file: "file", list_files: "grid", run_command: "terminal",
  run_in_terminal: "terminal", start_server: "play", server_logs: "file", plan: "grid", research_setup: "flask", research_status: "grid",
  experiment_create: "branch", experiment_checkout: "branch", experiment_run: "play", experiment_wait: "clock", run_log: "file",
  read_paper: "book", report_check: "shield", delegate: "users", output_limit: "info", connection: "info", thinking_limit: "info",
  check: "check", browser_search: "search", browser_open: "globe", browser_click: "cursor", browser_type: "edit", browser_read: "file",
  browser_scroll: "down", browser_back: "globe",
};
const RESEARCH = new Set(["research_setup", "research_status", "experiment_create", "experiment_checkout", "experiment_run", "experiment_wait", "run_log", "read_paper", "report_check"]);
export const kindOf = (e: ActivityEntry) => e.kind || "tool";
// Work on a cluster or other service goes through its own stages: look, ask, act,
// then check what the change did.
export interface Stages {
  name: string;
  icon: string;
  look: string;
  ask: string;
  act: string;
  check: string;
  rail: string[];
}
const STAGES: Record<string, Stages> = {
  kubernetes: { name: "Kubernetes", icon: "cube", look: "Inspect", ask: "Approve", act: "Apply", check: "Verify", rail: ["Inspect", "Approve", "Apply", "Verify"] },
  slurm: { name: "Slurm", icon: "server", look: "Inspect", ask: "Submit", act: "Submit", check: "Watch", rail: ["Inspect", "Submit", "Watch", "Results"] },
  mcp: { name: "", icon: "link", look: "Inspect", ask: "Approve", act: "Act", check: "Check", rail: ["Inspect", "Approve", "Act", "Check"] },
};
export const stagesOf = (e: ActivityEntry) => STAGES[e.service as string] || STAGES.mcp!;
export const browsing = (e: ActivityEntry) => String(e.tool || "").startsWith("browser_");
const CARDS = new Set(["sources", "saved", "question", "workspace", "research", "browser-handoff", "browser-request"]);
export const FILE_TOOLS = new Set(["write_file", "edit_file"]);
export const COMMANDS = new Set(["run_command", "start_server", "run_in_terminal"]);
// Searching knowledge, saving and asking are not work to show: their cards sit below the answer.
const KNOWLEDGE = (e: ActivityEntry) => CARDS.has(e.kind || "") || e.tool === "search_knowledge" || e.tool === "save_item";

export function duration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "";
  if (ms < 10000) return `${(ms / 1000).toFixed(1)}s`;
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

/** The first sentence of a thought, without markdown, for its one-line row. */
export function gist(text: string | undefined, max = 160): string {
  const plain = String(text || "").replace(/```[\s\S]*?```/g, " ").replace(/[#*_`>]+/g, "").replace(/\s+/g, " ").trim();
  const sentence = plain.match(/^(.{12,}?[.!?])(?=\s|$)/)?.[1] || plain;
  return sentence.length > max ? `${sentence.slice(0, max - 1).trimEnd()}…` : sentence;
}
/** The last sentences of a thought still being written. */
export function latest(text: string | undefined, max = 260): string {
  const plain = String(text || "").replace(/```[\s\S]*?```/g, " ").replace(/[#*_`>]+/g, "").replace(/\s+/g, " ").trim();
  if (plain.length <= max) return plain;
  const tail = plain.slice(-max),
    cut = tail.search(/[.!?]\s+\S/);
  return cut >= 0 && cut < max * 0.75 ? tail.slice(cut + 1).trim() : `…${tail.trim()}`;
}

export type Row =
  | { type: "thought"; e: ActivityEntry; text: string; phase: string }
  | { type: "note" | "checkpoint"; e: ActivityEntry; phase: string }
  | { type: "step"; e: ActivityEntry; phase: string }
  | { type: "deadend"; phase: string; tries: Row[] }
  | { type: "problem"; phase: string; start: Extract<Row, { type: "step" }>; middle: Row[]; fix: Extract<Row, { type: "step" }> };
export interface Work {
  rows: Row[];
  steps: ActivityEntry[];
  entries: ActivityEntry[];
  reasoning: string;
  thoughtsRecorded: boolean;
}

/**
 * The reply's work in order: thoughts (sliced from the reasoning by their offsets),
 * tool steps with a phase, notes and checkpoints. An older reply with one block of
 * reasoning gets it as a single first thought.
 */
export function workOf(m: Message): Work {
  const entries = Array.isArray(m.activity) ? m.activity : [];
  const reasoning = m.reasoningContent || "";
  const thoughtsRecorded = entries.some((e) => kindOf(e) === "thought");
  const list: ActivityEntry[] = !thoughtsRecorded && reasoning.trim() ? [{ id: "thought-all", kind: "thought", from: 0, to: reasoning.length }, ...entries] : entries;
  const rows: Row[] = [],
    pending: any[] = [];
  let wrote = false,
    phase: string | null = null;
  const acted = new Set<string>();
  const phaseOf = (e: ActivityEntry) => {
    if (e.connection) {
      const st = stagesOf(e);
      if (e.readOnly === false) return e.status === "awaiting" ? st.ask : st.act;
      return acted.has(e.connection) ? st.check : st.look;
    }
    if (e.tool === "plan") return "Plan";
    if (RESEARCH.has(e.tool || "")) return "Research";
    if (browsing(e)) return "Browse";
    if (FILE_TOOLS.has(e.tool || "")) return "Build";
    if (e.tool === "start_server" || e.tool === "server_logs" || e.tool === "run_in_terminal") return "Run";
    if (e.tool === "run_command") return wrote ? "Run" : "Explore";
    return wrote ? "Build" : "Explore";
  };
  for (const e of list) {
    const kind = kindOf(e);
    if (CARDS.has(kind)) continue;
    if (kind === "thought") {
      const text = reasoning.slice(e.from || 0, e.to ?? reasoning.length).trim();
      if (text) pending.push({ type: "thought", e, text });
      continue;
    }
    if (kind === "note" || kind === "checkpoint") {
      if (phase) {
        pending.splice(0).forEach((t) => rows.push({ ...t, phase }));
        rows.push({ type: kind, e, phase });
      } else pending.push({ type: kind, e });
      continue;
    }
    phase = phaseOf(e);
    if (FILE_TOOLS.has(e.tool || "") && e.status === "completed") wrote = true;
    if (e.connection && e.readOnly === false && e.status === "completed") acted.add(e.connection);
    pending.splice(0).forEach((t) => rows.push({ ...t, phase }));
    rows.push({ type: "step", e, phase });
  }
  // Thinking after the last step led to the answer.
  pending.forEach((t) => rows.push({ ...t, phase: phase ? "Answer" : "Thinking" }));
  return { rows: fold(rows), steps: list.filter((e) => kindOf(e) === "tool"), entries: list, reasoning, thoughtsRecorded };
}

const minutes = (m: number) => (!m ? "" : m >= 60 && m % 60 === 0 ? `${m / 60} h` : m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m} min`);
export interface ChangeItem {
  verb: string;
  what: string;
  where?: string;
  facts: [string, string][];
}
/**
 * What a change waiting for approval does, read loosely from its arguments: the
 * resources a Kubernetes manifest applies or deletes, or the Slurm job a script
 * submits. Null when there is nothing to read.
 */
export function changeOf(s: ActivityEntry): { items: ChangeItem[]; raw?: string; text?: string } | null {
  const text = String(s.detail || "");
  const field = (src: string, key: string) => src.match(new RegExp(`^${key}:[ \\t]*["']?([^"'\\n]+?)["']?[ \\t]*$`, "m"))?.[1];
  if (s.service === "kubernetes" && s.action === "apply") {
    const body = text.replace(/^manifest:[ \t]*\n?/, "");
    let docs: { kind?: string; name?: string; namespace?: string; text: string }[];
    try {
      const v = JSON.parse(body);
      docs = (Array.isArray(v) ? v : v?.kind === "List" ? v.items || [] : [v]).map((o: any) => ({ kind: o?.kind, name: o?.metadata?.name, namespace: o?.metadata?.namespace, text: JSON.stringify(o) }));
    } catch {
      docs = body
        .split(/^---[ \t]*$/m)
        .filter((d) => d.trim())
        .map((d) => {
          const meta = d.match(/^metadata:[ \t]*\n((?:[ \t]+.*(?:\n|$))*)/m)?.[1] || "";
          return { kind: field(d, "kind"), name: meta.match(/^[ \t]+name:[ \t]*["']?([^"'\s]+)/m)?.[1], namespace: meta.match(/^[ \t]+namespace:[ \t]*["']?([^"'\s]+)/m)?.[1], text: d };
        });
    }
    const items = docs
      .filter((d) => d.kind)
      .map((d) => {
        const images = [...new Set([...d.text.matchAll(/"?image"?:[ \t]*"?([^"\s,}]+)/g)].map((x) => x[1]!))];
        const replicas = d.text.match(/"?replicas"?:[ \t]*(\d+)/)?.[1];
        const ports = [...new Set([...d.text.matchAll(/"?(?:containerPort|port)"?:[ \t]*(\d+)/g)].map((x) => x[1]!))];
        return {
          verb: "Apply",
          what: `${d.kind}/${d.name || "?"}`,
          where: d.namespace,
          facts: ([["Image", images.join(", ")], ["Replicas", replicas], ["Port", ports.join(", ")]] as [string, string | undefined][]).filter((f) => f[1]) as [string, string][],
        };
      });
    return items.length ? { items, raw: "Manifest", text: body } : null;
  }
  if (s.service === "kubernetes" && s.action === "delete")
    return { items: [{ verb: "Delete", what: `${field(text, "kind") || "?"}/${field(text, "name") || "?"}`, where: field(text, "namespace"), facts: [] }] };
  if (s.service === "slurm" && s.action === "submit") {
    const script = text.match(/^script:[ \t]*\n([\s\S]*?)(?=^(?:cpus|memory_mb|name|nodes|output|partition|tasks|time_limit_minutes|working_directory):|$(?![\s\S]))/m)?.[1];
    const mb = Number(field(text, "memory_mb"));
    const facts = (
      [
        ["Partition", field(text, "partition") || "default"],
        ["Time limit", minutes(Number(field(text, "time_limit_minutes")))],
        ["CPUs", field(text, "cpus")],
        ["Nodes", field(text, "nodes")],
        ["Memory", mb ? (mb >= 1024 ? `${+(mb / 1024).toFixed(1)} GB` : `${mb} MB`) : ""],
      ] as [string, string | undefined][]
    ).filter((f) => f[1]) as [string, string][];
    return { items: [{ verb: "Submit", what: field(text, "name") || "batch job", facts }], raw: "Script", text: script?.trimEnd() || text };
  }
  if (s.service === "slurm" && s.action === "cancel") return { items: [{ verb: "Cancel", what: `job ${field(text, "job_id") || "?"}`, facts: [] }] };
  return null;
}

type StepRow = Extract<Row, { type: "step" }>;
const failed = (r: Row): boolean => r.type === "step" && r.e.tool !== "delegate" && (r.e.status === "error" || r.e.check === "error");
/**
 * Folds failed exploration into one dead-end row, and a problem (a failed step or a
 * broken preview) with what fixed it into one problem row.
 */
function fold(rows: Row[]): Row[] {
  const out: Row[] = [];
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i] as StepRow;
    if (failed(r) && r.phase === "Explore" && r.e.status === "error") {
      const tries: Row[] = [r];
      while (i + 1 < rows.length && failed(rows[i + 1]!) && rows[i + 1]!.phase === "Explore") tries.push(rows[++i]!);
      out.push({ type: "deadend", phase: r.phase, tries });
      continue;
    }
    if (failed(r) && r.e.status !== "declined") {
      // What follows until a step shows it fixed: the preview compiles again, or the
      // same tool succeeds. Unresolved within a few rows: leave the rows as they are.
      let end = -1;
      for (let j = i + 1; j < Math.min(rows.length, i + 10); j++) {
        const n = rows[j] as StepRow;
        if (n.type !== "step" || failed(n)) continue;
        if (n.e.check === "ok" || (n.e.tool === r.e.tool && !n.e.connection) || (n.e.connection && n.e.tool === r.e.tool)) {
          end = j;
          break;
        }
      }
      if (end > 0) {
        out.push({ type: "problem", phase: r.phase, start: r, middle: rows.slice(i + 1, end), fix: rows[end] as StepRow });
        i = end;
        continue;
      }
    }
    out.push(r);
  }
  return out;
}

export interface FileChange {
  path: string;
  added: number;
  removed: number;
}
/** Totals for the "Show work" line and the result card. */
export function stats(work: Work) {
  const steps = work.steps,
    files = new Map<string, FileChange>();
  for (const s of steps) {
    if (!FILE_TOOLS.has(s.tool || "") || s.status !== "completed" || !s.path) continue;
    const f = files.get(s.path) || { path: s.path, added: 0, removed: 0 };
    f.added += s.added || 0;
    f.removed += s.removed || 0;
    files.set(s.path, f);
  }
  const thoughtMs = work.entries.filter((e) => kindOf(e) === "thought").reduce((t, e) => t + (e.durationMs || 0), 0);
  const starts = work.entries.map((e) => Date.parse(e.startedAt || "")).filter(Number.isFinite);
  const ends = work.entries.map((e) => Date.parse(e.startedAt || "") + (e.durationMs || 0)).filter(Number.isFinite);
  const lastCheck = [...steps].reverse().find((s) => s.check);
  return {
    steps: steps.length,
    thoughtMs,
    files: [...files.values()],
    added: [...files.values()].reduce((t, f) => t + f.added, 0),
    removed: [...files.values()].reduce((t, f) => t + f.removed, 0),
    deadEnds: work.rows.filter((r) => r.type === "deadend").length,
    fixes: work.rows.filter((r) => r.type === "problem").length,
    span: starts.length ? Math.max(...ends) - Math.min(...starts) : 0,
    began: starts.length ? Math.min(...starts) : null,
    check: (lastCheck?.check as string) || null,
    served: steps.some((s) => (s.tool === "start_server" || s.check) && s.status === "completed"),
    checkpoints: work.entries.filter((e) => kindOf(e) === "checkpoint"),
  };
}
export type Stats = ReturnType<typeof stats>;

/** Whether this reply is shown with the work view (it used a tool). */
export const appliesWork = (m: Message) => m.role === "assistant" && Array.isArray(m.activity) && m.activity.some((e) => kindOf(e) !== "thought" && !KNOWLEDGE(e));
export const isLive = (m: Message) => m.status === "pending" || m.status === "streaming";

/** Which "Show" chip a trace row belongs to. */
export const filterOf = (r: Row) =>
  r.type === "thought"
    ? "thinking"
    : r.type === "deadend" || r.type === "problem"
      ? "problems"
      : r.type === "checkpoint" || (r.type === "step" && (FILE_TOOLS.has(r.e.tool || "") || r.e.tool === "plan"))
        ? "files"
        : r.type === "step"
          ? "commands"
          : "all";

/** An example for the Steer box that fits the reply's work, most specific first. */
export function steerExample(m: Message | null | undefined): string {
  const steps = m ? workOf(m).steps : [];
  const has = (test: (e: ActivityEntry) => boolean) => steps.some(test);
  if (has((e) => RESEARCH.has(e.tool || ""))) return "try a larger batch size next";
  if (has((e) => e.service === "slurm")) return "ask for 4 hours, not 1";
  if (has((e) => e.service === "kubernetes")) return "use 2 replicas instead";
  if (has(browsing)) return "check the pricing page too";
  if (has((e) => FILE_TOOLS.has(e.tool || "") || /^(run_command|run_in_terminal|start_server|list_files|read_file)$/.test(e.tool || ""))) return "add tests before you finish";
  return "keep it short";
}
export const steerPlaceholder = (m: Message | null | undefined) => `Add a note without stopping, like “${steerExample(m)}”`;

/** The last lines of a step's output. */
export const lastLines = (text: string | undefined, n: number) => String(text || "").replace(/\s+$/, "").split("\n").slice(-n).join("\n");
export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
export const stepTime = (s: ActivityEntry) => (({ running: "Running…", awaiting: "Needs your answer", error: "Failed", declined: "Declined" }) as Record<string, string>)[s.status || ""] || duration(s.durationMs);
/** How a step went, for its mark: bad, now, off, warn or ok. */
export const stepMark = (s: ActivityEntry) =>
  s.status === "error" || s.check === "error" ? "bad" : s.status === "running" || s.status === "awaiting" ? "now" : s.status === "declined" ? "off" : s.flagged ? "warn" : "ok";

const PHASES = ["Plan", "Explore", "Build", "Run"];
/** Everything the live card shows, worked out from the reply so far. */
export function runState(m: Message, work: Work, now = Date.now()) {
  const s = stats(work);
  const waiting = work.steps.filter((x) => x.status === "awaiting");
  const running = [...work.steps].reverse().find((x) => x.status === "running");
  const lastStep = work.steps.at(-1);
  const lastThought = [...work.entries].reverse().find((e) => kindOf(e) === "thought");
  const fresh = work.reasoning.slice(lastThought ? lastThought.to : 0).trim();
  const draft = m._draft && !work.steps.some((x) => x.id === m._draft!.id) ? m._draft : null;
  const answering = !running && !waiting.length && !draft && !!String(m.content || "").trim();
  const asked = waiting[0] && changeOf(waiting[0])?.items[0];
  const headline = asked
    ? `Waiting for you: ${asked.verb.toLowerCase()} ${asked.what}`
    : waiting[0]
      ? `Waiting for you: ${waiting[0].summary}`
      : draft
        ? `Writing ${draft.path || "a file"}`
        : running
          ? running.summary || running.tool || "Working"
          : answering
            ? "Writing the answer"
            : fresh
              ? "Thinking"
              : "Working";
  const began = s.began || Date.parse(m.createdAt || m._began || "") || now;
  const stepRows = work.rows.filter((r): r is Extract<Row, { type: "step" }> => r.type === "step");
  const seen = stepRows.map((r) => r.phase);
  const service = [...work.steps].reverse().find((x) => x.connection);
  const rail = service ? stagesOf(service).rail : [];
  let current = draft ? "Build" : waiting[0] ? stepRows.find((r) => r.e === waiting[0])?.phase : seen.at(-1);
  if (answering && rail.includes("Results") && service && current === stagesOf(service).check) current = "Results";
  // Workspace work goes through the usual phases; a cluster or service through its stages.
  const usual = draft || work.steps.some((x) => !x.connection && !browsing(x) && x.tool !== "delegate") ? PHASES.filter((p) => p !== "Plan" || seen.includes("Plan")) : [];
  const phases = [...new Set([...usual, ...rail, ...seen])];
  const count = (p: string) => seen.filter((x) => x === p).length;
  const cur = phases.indexOf(current || "");
  const railItems = phases.map((p, at) => {
    const status = p === current ? (waiting[0] ? "ask" : "now") : seen.includes(p) || cur > at ? "done" : "next";
    const note =
      status === "ask"
        ? "your call"
        : status === "now"
          ? draft
            ? "writing"
            : p === "Results"
              ? "answering"
              : running
                ? "working"
                : "thinking"
          : count(p)
            ? plural(count(p), "step")
            : "";
    return { phase: p, status, note };
  });
  const output = [...work.steps]
    .reverse()
    .find((x) => x.status !== "running" && x.status !== "awaiting" && x.detail && !FILE_TOOLS.has(x.tool || "") && x.tool !== "delegate");
  let pane: { label: string; note: string; kind: "draft" | "running" | "thinking" | "output" | "idle"; text: string };
  if (draft) pane = { label: draft.path || "file", note: `${plural(draft.lines, "line")} so far`, kind: "draft", text: draft.tail };
  else if (running && (COMMANDS.has(running.tool || "") || running.connection || browsing(running)))
    pane = {
      label: running.summary || running.tool || "",
      note: "running",
      kind: "running",
      text: `${output && output !== running ? `${lastLines(output.detail, 6)}\n` : ""}`,
    };
  else if (fresh && !answering) pane = { label: "Thinking", note: "", kind: "thinking", text: latest(fresh, 420) };
  else if (output) pane = { label: output.summary || output.tool || "", note: output.status === "error" ? "failed" : "output", kind: "output", text: lastLines(output.detail, 9) };
  else pane = { label: answering ? "Answer" : "Starting", note: "", kind: "idle", text: answering ? "Writing the answer…" : "Waiting for the model…" };
  const thoughtText = lastThought ? work.reasoning.slice(lastThought.from, lastThought.to) : "";
  const ticker = fresh && pane.label !== "Thinking" ? latest(fresh, 200) : thoughtText ? latest(thoughtText, 200) : "";
  const recent: (ActivityEntry | null)[] = work.steps.filter((x) => x.status !== "awaiting").slice(-3);
  while (recent.length < 3) recent.push(null);
  const graph = running?.tool === "delegate" && running.agents?.length ? running : null;
  return { stats: s, waiting, running, lastStep, draft, answering, headline, began, rail: railItems, pane, ticker, recent, graph };
}
