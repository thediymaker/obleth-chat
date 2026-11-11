"use client";

// How a reply that used tools shows its work. While it runs: one live
// card (what it is doing now, its stages, the pane of what is happening, the latest
// thought and steps, a Steer box). When it is done: what changed and whether it
// works, checkpoints, and a "Show work" trace in which each thought sits before the
// step it led to.

import { Bookmark, Check, ChevronDown, Clock, Download, File, Globe, MessageSquare, Orbit, PanelLeft, Play, Terminal, Users } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { clock } from "@/lib/chat/composer";
import { steerDraft } from "@/lib/chat/steer-draft";
import {
  changeOf,
  duration,
  FILE_TOOLS,
  filterOf,
  gist,
  isLive,
  plural,
  runState,
  stagesOf,
  stats as statsOf,
  steerPlaceholder,
  stepMark,
  stepTime,
  browsing,
  TOOL_ICONS,
  workOf,
  type Row,
  type Stats,
  type Work,
} from "@/lib/chat/turn";
import type { ActivityEntry, Message } from "@/lib/chat/types";
import { cn } from "@/lib/utils";
import { useChatStore as useChat } from "./chat-provider";
import { NamedIcon } from "./icon";
import { Markdown } from "./markdown-view";

const MARK_TONE: Record<string, string> = {
  bad: "text-destructive-foreground",
  now: "text-blue",
  off: "text-faint",
  warn: "text-amber",
  ok: "text-success",
  next: "text-faint",
};

/** A second counter for live times. */
function useNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}

function Lines({ s }: { s: ActivityEntry }) {
  return (
    <>
      {s.added ? <span className="text-success"> +{s.added}</span> : null}
      {s.removed ? <span className="text-destructive-foreground"> −{s.removed}</span> : null}
    </>
  );
}

function FileButton({ path }: { path: string }) {
  const store = useChat();
  return (
    <button type="button" className="font-mono text-[12px] underline-offset-2 hover:underline" onClick={() => store.workspace.openFile(store.route.id, path)}>
      {path}
    </button>
  );
}

/** A step in words: what it did, the file it wrote, the host it browsed. */
function StepText({ s }: { s: ActivityEntry }) {
  const store = useChat();
  if (s.tool === "delegate") {
    const model = s.workerModel || s.agents?.[0]?.model,
      getting = s.status === "running" && !s.agents && s.detail;
    return (
      <>
        {s.summary || "Sent agents"}
        {model && <span className="text-faint"> · {model}</span>}
        {getting && <span className="text-faint"> · {s.detail}</span>}
      </>
    );
  }
  if (s.path && FILE_TOOLS.has(s.tool || ""))
    return (
      <>
        {s.tool === "write_file" ? "Wrote" : "Edited"} <FileButton path={s.path} />
        <Lines s={s} />
      </>
    );
  if (s.tool === "run_in_terminal" && s.command)
    return (
      <>
        Opened <code className="rounded bg-muted px-1 font-mono text-[12px]">{s.command}</code> in the terminal{" "}
        <button type="button" className="text-sm text-muted-foreground underline-offset-2 hover:underline" onClick={() => store.workspace.runInTerminal(store.route.id, s.command)}>
          Run again
        </button>
      </>
    );
  if (s.tool === "plan" && Array.isArray(s.files)) return <>Planned {plural(s.files.length, "file")}</>;
  if (browsing(s) && s.url) {
    let host = "";
    try {
      host = new URL(s.url).host;
    } catch {
      // Not a URL.
    }
    return (
      <>
        {s.summary || s.tool}
        {host && s.tool !== "browser_open" && <span className="text-faint"> · {host}</span>}
      </>
    );
  }
  return <>{s.summary || s.tool}</>;
}

const DECISIONS: ["allow" | "chat" | "deny", (s: ActivityEntry) => string, boolean][] = [
  ["allow", (s) => (s.question ? "Allow once" : "Allow"), true],
  ["chat", (s) => s.remember || "Allow for this chat", false],
  ["deny", () => "Deny", false],
];
function Decisions({ s }: { s: ActivityEntry }) {
  const store = useChat();
  const [busy, setBusy] = useState(false);
  return (
    <>
      {DECISIONS.map(([decision, label, primary]) => (
        <Button
          key={decision}
          size="xs"
          variant={primary ? "default" : "outline"}
          disabled={busy}
          title={decision === "chat" ? `Run ${s.summary} without asking for the rest of this conversation` : undefined}
          onClick={async () => {
            setBusy(true);
            await store.decide(s.id!, decision);
            setBusy(false);
          }}
        >
          {label(s)}
        </Button>
      ))}
    </>
  );
}
function Approval({ s }: { s: ActivityEntry }) {
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-amber/40 bg-amber/5 px-2.5 py-2 text-sm">
      <span className="min-w-0 flex-1">{s.question || `Allow ${s.connection || "this tool"} to run this?`}</span>
      <Decisions s={s} />
    </div>
  );
}

/**
 * A device step this browser carries out: what it is doing, or the button that
 * attaches the device when the call waits for it in this tab.
 */
function DeviceStep({ s }: { s: ActivityEntry }) {
  const store = useChat();
  const [, bump] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => store.devices.subscribe(() => bump((n) => n + 1)), [store]);
  const waiting = s.id ? store.devices.waiting(s.id) : undefined;
  const progress = s.id ? store.devices.progress(s.id) : "";
  if (!store.devices.supported())
    return <p className="mt-1.5 text-sm text-amber">This browser cannot reach devices. Open this chat in Chrome or Edge, with the device plugged in.</p>;
  if (waiting)
    return (
      <div className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-amber/40 bg-amber/5 px-2.5 py-2 text-sm">
        <span className="min-w-0 flex-1">{waiting.connection} is not attached in this browser tab. Plug it in and pick its port.</span>
        <Button
          size="xs"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError("");
            try {
              await store.devices.attach({ id: waiting.connectionId, name: waiting.connection });
            } catch (e) {
              setError((e as Error).message);
            }
            setBusy(false);
          }}
        >
          Attach {waiting.connection}
        </Button>
        {error && <span className="basis-full text-destructive-foreground">{error}</span>}
      </div>
    );
  return progress ? <p className="mt-1 text-sm text-muted-foreground">{progress}</p> : null;
}

/** The workers a delegate step sent: each one's progress, then its report. */
function Agents({ s }: { s: ActivityEntry }) {
  const list: any[] = Array.isArray(s.agents) ? s.agents : [];
  if (!list.length) return null;
  const tokens = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k tokens` : `${n} tokens`);
  return (
    <ol className="mt-1.5 grid gap-1">
      {list.map((a, i) => {
        const mark = a.status === "error" ? "bad" : a.status === "completed" ? "ok" : a.status === "running" ? "now" : "next";
        const facts = [a.status === "completed" ? plural(a.steps, "step") : a.doing, a.status !== "completed" && a.steps ? `step ${a.steps}` : "", a.tokens ? tokens(a.tokens) : ""].filter(Boolean).join(" · ");
        const head = (
          <span className="flex min-w-0 items-center gap-2">
            <span className={cn("size-2 shrink-0 rounded-full bg-current", MARK_TONE[mark])} aria-label={a.status || ""} />
            <span className="min-w-0">
              <strong className="block truncate text-sm font-medium">{a.title}</strong>
              <small className="block truncate text-xs text-muted-foreground">{facts}</small>
            </span>
          </span>
        );
        return (
          <li key={i}>
            {a.report ? (
              <details>
                <summary className="cursor-pointer list-none">{head}</summary>
                <div className="mt-1 ml-4 max-h-80 overflow-y-auto rounded-md bg-muted px-3 py-2">
                  <Markdown text={a.report} />
                </div>
              </details>
            ) : (
              head
            )}
          </li>
        );
      })}
    </ol>
  );
}

function Step({ s }: { s: ActivityEntry }) {
  const mark = stepMark(s);
  const glyph = s.connection ? "link" : TOOL_ICONS[s.tool || ""] || "terminal";
  const thumb = s.thumb && String(s.thumb).startsWith("data:image/jpeg;base64,") ? s.thumb : "";
  const head = (
    <span className="flex min-w-0 flex-1 items-center gap-2">
      {/* eslint-disable-next-line @next/next/no-img-element -- a screenshot the browser step sent */}
      {thumb && <img className="h-8 w-12 shrink-0 rounded border object-cover" src={thumb} alt="" loading="lazy" />}
      <span className="min-w-0 flex-1 text-md">
        <StepText s={s} />
        {s.check === "error" && <span className="text-destructive-foreground"> · preview broke</span>}
      </span>
      <small className="shrink-0 text-xs text-faint">{stepTime(s)}</small>
    </span>
  );
  const detail = s.detail && s.status !== "running" && s.status !== "awaiting" ? String(s.detail) : "";
  let body: React.ReactNode;
  if (s.tool === "delegate")
    body = (
      <>
        <div className="flex items-center">{head}</div>
        {Array.isArray(s.agents) && s.agents.length ? <Agents s={s} /> : s.status === "error" && s.detail ? <Pre text={s.detail} /> : null}
      </>
    );
  else if (s.service === "device" && s.status === "running")
    body = (
      <>
        <div className="flex items-center">{head}</div>
        <DeviceStep s={s} />
      </>
    );
  else if (s.status === "awaiting")
    body = (
      <>
        <div className="flex items-center">{head}</div>
        {s.detail && <Pre text={s.detail} />}
        <Approval s={s} />
      </>
    );
  else if (detail && !(FILE_TOOLS.has(s.tool || "") && s.status === "completed"))
    body = (
      <details open={s.status === "error" || !!s.flagged} className="group/step">
        <summary className="flex cursor-pointer list-none items-center [&::-webkit-details-marker]:hidden">{head}</summary>
        <Pre text={detail} />
      </details>
    );
  else body = <div className="flex items-center">{head}</div>;
  return (
    <li className="flex gap-2.5 py-1">
      <span className={cn("mt-0.5 flex size-5 shrink-0 items-center justify-center", MARK_TONE[mark])} aria-hidden="true">
        <NamedIcon name={glyph} className="size-3.5" />
      </span>
      <div className="min-w-0 flex-1">{body}</div>
    </li>
  );
}

const Pre = ({ text, className }: { text: string; className?: string }) => (
  <pre className={cn("mt-1.5 max-h-64 overflow-auto rounded-md bg-code px-3 py-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-text-2", className)}>{text}</pre>
);

function TraceRow({ r }: { r: Row }) {
  const store = useChat();
  if (r.type === "thought")
    return (
      <li className="flex gap-2.5 py-1" data-f="thinking">
        <Orbit className="mt-0.5 size-4 shrink-0 text-faint" aria-hidden="true" />
        <details className="min-w-0 flex-1">
          <summary className="flex cursor-pointer list-none items-center gap-2 [&::-webkit-details-marker]:hidden">
            <span className="min-w-0 flex-1 text-md text-text-2">{gist(r.text)}</span>
            <small className="text-xs text-faint">{duration(r.e.durationMs)}</small>
          </summary>
          <div className="mt-1 max-h-96 overflow-y-auto rounded-md bg-muted px-3 py-2 text-text-2">
            <Markdown text={r.text} />
          </div>
        </details>
      </li>
    );
  if (r.type === "note")
    return (
      <li className="flex gap-2.5 py-1">
        <MessageSquare className="mt-0.5 size-4 shrink-0 text-faint" aria-hidden="true" />
        <span className="text-md">
          <span className="text-muted-foreground">You, while it worked:</span> {r.e.summary}
        </span>
      </li>
    );
  if (r.type === "checkpoint")
    return (
      <li className="flex items-center gap-2.5 py-1">
        <Bookmark className="size-4 shrink-0 text-faint" aria-hidden="true" />
        <span className="min-w-0 flex-1 text-md">
          {r.e.summary} saved <span className="text-faint">· {r.e.stat || "project saved"}</span>
        </span>
        <button type="button" className="text-sm text-muted-foreground hover:text-foreground hover:underline" onClick={() => store.workspace.restoreCheckpoint(store.route.id, r.e.sha, r.e.summary || "")}>
          Restore
        </button>
      </li>
    );
  if (r.type === "deadend") {
    const first = (r.tries[0] as Extract<Row, { type: "step" }>).e;
    return (
      <li className="flex gap-2.5 py-1">
        <span className="mt-1.5 size-2 shrink-0 rounded-full bg-faint" aria-hidden="true" />
        <details className="min-w-0 flex-1 rounded-md border px-2.5 py-1.5">
          <summary className="flex cursor-pointer list-none items-center gap-2 [&::-webkit-details-marker]:hidden">
            <span className="min-w-0 flex-1 text-md">Dead end: {first.summary}</span>
            <small className="text-xs text-faint">
              {r.tries.length} tr{r.tries.length === 1 ? "y" : "ies"}
            </small>
          </summary>
          <ol className="mt-1">
            {r.tries.map((t, i) => (
              <Step key={i} s={(t as Extract<Row, { type: "step" }>).e} />
            ))}
          </ol>
        </details>
      </li>
    );
  }
  if (r.type === "problem") {
    const why = r.middle.find((x) => x.type === "thought") as Extract<Row, { type: "thought" }> | undefined;
    const fixes = r.middle.filter((x): x is Extract<Row, { type: "step" }> => x.type === "step");
    const fileFix = FILE_TOOLS.has(r.fix.e.tool || "");
    return (
      <li className="flex gap-2.5 py-1">
        <span className="mt-1.5 size-2 shrink-0 rounded-full bg-amber" aria-hidden="true" />
        <div className="min-w-0 flex-1 rounded-md border border-amber/40 px-3 py-2">
          <div className="mb-1 text-xs font-semibold tracking-wide text-amber uppercase">Problem fixed</div>
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-md">
            <dt className="text-muted-foreground">Saw</dt>
            <dd className="min-w-0">
              {r.start.e.check === "error" ? (
                <>
                  <StepText s={r.start.e} />: the preview stopped compiling
                </>
              ) : (
                `${r.start.e.summary}: ${String(r.start.e.detail || "failed").split("\n").find((l) => l.trim()) || "failed"}`
              )}
            </dd>
            {why && (
              <>
                <dt className="text-muted-foreground">Why</dt>
                <dd>{gist(why.text, 220)}</dd>
              </>
            )}
            {(fixes.length > 0 || fileFix) && (
              <>
                <dt className="text-muted-foreground">Fix</dt>
                <dd className="grid">
                  {fixes.length ? fixes.map((x, i) => <span key={i}><StepText s={x.e} /></span>) : <StepText s={r.fix.e} />}
                </dd>
              </>
            )}
            <dt className="text-muted-foreground">Check</dt>
            <dd>
              <span className="text-success">✓</span>{" "}
              {fileFix && !fixes.length ? (
                r.fix.e.check === "ok" ? "The preview compiles again" : "The change went through"
              ) : (
                <>
                  <StepText s={r.fix.e} />
                  {r.fix.e.check === "ok" ? " · the preview compiles" : ""}
                </>
              )}
            </dd>
          </dl>
        </div>
      </li>
    );
  }
  return <Step s={r.e} />;
}

const FILTERS: [string, string][] = [
  ["all", "All"],
  ["thinking", "Thinking"],
  ["files", "Files"],
  ["commands", "Commands"],
  ["problems", "Problems"],
];
// Which filter each trace shows, by reply.
const filters = new Map<string, string>();

function Trace({ m, work }: { m: Message; work: Work }) {
  const id = m.id || "live";
  const [filter, setFilterState] = useState(filters.get(id) || "all");
  const setFilter = (f: string) => {
    filters.set(id, f);
    setFilterState(f);
  };
  const t0 = statsOf(work).began;
  const groups: { phase: string; rows: Row[] }[] = [];
  for (const r of work.rows) {
    if (!groups.length || groups.at(-1)!.phase !== r.phase) groups.push({ phase: r.phase, rows: [] });
    groups.at(-1)!.rows.push(r);
  }
  const range = (g: { rows: Row[] }) => {
    const es = g.rows
      .flatMap((r) => (r.type === "step" ? [r.e] : r.type === "problem" ? [r.start.e, r.fix.e] : r.type === "deadend" ? r.tries.map((t) => (t as Extract<Row, { type: "step" }>).e) : []))
      .filter((e) => Number.isFinite(Date.parse(e.startedAt || "")));
    if (!es.length || t0 == null) return "";
    const a = Math.min(...es.map((e) => Date.parse(e.startedAt!))),
      b = Math.max(...es.map((e) => Date.parse(e.startedAt!) + (e.durationMs || 0)));
    return `${clock(a - t0)} – ${clock(b - t0)}`;
  };
  const problems = work.rows.filter((r) => r.type === "problem" || r.type === "deadend").length;
  const shown = (r: Row) => filter === "all" || filterOf(r) === filter;
  return (
    <section className="mt-2 rounded-lg border bg-card px-3 py-2" aria-label="The work in this reply">
      <div className="mb-1 flex flex-wrap gap-1" role="group" aria-label="Show">
        {FILTERS.map(([k, label]) => (
          <button
            key={k}
            type="button"
            aria-pressed={filter === k}
            onClick={() => setFilter(k)}
            className={cn(
              "rounded-full border px-2.5 py-0.5 text-xs",
              filter === k ? "border-foreground bg-secondary text-foreground" : "text-muted-foreground hover:text-foreground",
              k === "problems" && problems > 0 && "text-amber",
            )}
          >
            {label}
            {k === "problems" && problems ? ` · ${problems}` : ""}
          </button>
        ))}
      </div>
      <ol>
        {groups.map((g, i) => {
          const rows = g.rows.filter(shown);
          if (!rows.length) return null;
          return (
            <li key={i}>
              <div className="mt-2 flex items-baseline justify-between border-b border-line-soft pb-1 text-2xs font-medium tracking-wider text-muted-foreground uppercase">
                <span>{g.phase}</span>
                <small className="font-mono normal-case">{range(g)}</small>
              </div>
              <ol>
                {rows.map((r, j) => (
                  <TraceRow key={j} r={r} />
                ))}
              </ol>
            </li>
          );
        })}
      </ol>
    </section>
  );
}

// Which replies have their trace open.
const opened = new Set<string>();
function useTraceOpen(id: string): [boolean, () => void] {
  const [open, setOpen] = useState(opened.has(id));
  return [
    open,
    () => {
      if (opened.has(id)) opened.delete(id);
      else opened.add(id);
      setOpen(opened.has(id));
    },
  ];
}

/** One argument of a call waiting for approval, whole: a line beside its name, or a block that scrolls. */
function Asked({ name, text }: { name: string; text: string }) {
  const lines = text.split("\n").length;
  if (lines === 1 && text.length <= 120)
    return (
      <div className="flex flex-wrap items-baseline gap-x-2 text-sm">
        <span className="text-muted-foreground">{name}</span>
        <span className="break-all font-mono text-[12px]">{text}</span>
      </div>
    );
  return (
    <div className="grid gap-1">
      <span className="text-sm text-muted-foreground">
        {name} · {plural(lines, "line")}
      </span>
      <Pre text={text} className="max-h-80" />
    </div>
  );
}

/**
 * A call waiting for approval: what it does, as the server read it, and every argument
 * in full and in view. Nothing the call carries is folded away or cut short, so what
 * the person allows is what they could read.
 */
function AskCard({ s, now }: { s: ActivityEntry; now: number }) {
  const change = changeOf(s),
    waits = Date.parse(s.startedAt || "") + 15 * 60000;
  const head = s.question ? s.question : s.readOnly === false && s.service !== "mcp" ? `Allow this change on ${s.connection}?` : `Allow ${s.connection || "this tool"} to run this?`;
  return (
    <div className="grid gap-2.5 rounded-md border border-amber/40 bg-amber/5 p-3">
      <div>
        <strong className="block text-md font-semibold">{head}</strong>
        {s.readOnly === false && (
          <span className="text-sm text-muted-foreground">It changes {s.service === "mcp" || !s.service ? "something outside Chat" : "the cluster"}, so it waits for you. It runs as your own account.</span>
        )}
      </div>
      {change && change.items.length > 0 && (
        <div className="grid gap-2">
          {change.items.map((c, i) => (
            <div key={i} className="rounded-md border bg-card px-2.5 py-2">
              <div className="flex flex-wrap items-center gap-2 text-md">
                <Badge variant={c.verb === "Delete" || c.verb === "Cancel" ? "destructive" : "secondary"}>{c.verb}</Badge>
                <span className="break-all font-mono text-[12px]">{c.what}</span>
                {c.where && <span className="text-sm text-muted-foreground">in {c.where}</span>}
              </div>
              {c.facts.length > 0 && (
                <dl className="mt-1.5 grid grid-cols-2 gap-x-3 gap-y-0.5 text-sm sm:grid-cols-4">
                  {c.facts.map(([k, v]) => (
                    <div key={k}>
                      <dt className="text-xs text-muted-foreground">{k}</dt>
                      <dd className="break-all font-mono text-[12px]">{v}</dd>
                    </div>
                  ))}
                </dl>
              )}
            </div>
          ))}
        </div>
      )}
      {change ? (
        change.arguments.length > 0 && (
          <div className="grid gap-2">
            {change.arguments.map((a) => (
              <Asked key={a.name} name={a.name} text={a.text} />
            ))}
          </div>
        )
      ) : (
        s.detail && (
          <div className="grid gap-1">
            <span className="text-sm text-muted-foreground">Arguments · {plural(String(s.detail).trim().split("\n").length, "line")}</span>
            <Pre text={String(s.detail)} className="max-h-80" />
          </div>
        )
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Decisions s={s} />
        {Number.isFinite(waits) && <small className="ml-auto text-xs text-muted-foreground">Waits {clock(waits - now)} more</small>}
      </div>
    </div>
  );
}

// Which worker each delegate step shows in its detail panel.
const chosen = new Map<string, number>();
/** The workers of a delegate step as a graph: the lead, each worker, and their reports coming back. */
function WorkerGraph({ s, m }: { s: ActivityEntry; m: Message }) {
  const list: any[] = s.agents,
    n = list.length;
  const [pickedState, setPicked] = useState<number | null>(chosen.has(s.id!) ? chosen.get(s.id!)! : null);
  const picked = Math.min(n - 1, pickedState ?? Math.max(0, list.findIndex((a) => a.status === "running")));
  const mark = (a: any) => (a.status === "error" ? "bad" : a.status === "completed" ? "ok" : a.status === "running" ? "now" : "next");
  const tokens = (k: number) => (!k ? "" : k >= 1000 ? `${(k / 1000).toFixed(k >= 10000 ? 0 : 1)}k` : String(k));
  const done = list.filter((a) => a.status === "completed").length,
    bad = list.filter((a) => a.status === "error").length,
    left = n - done - bad;
  const lead = m.settings?.resolvedModel || m.settings?.model || "Lead model";
  const a = list[picked];
  const facts = [a.status === "completed" || a.status === "error" ? plural(a.steps || 0, "step") : `step ${a.steps || 0} of 30`, a.tokens ? `${tokens(a.tokens)} tokens` : ""].filter(Boolean).join(" · ");
  const status = ({ completed: "Reported", error: "Failed", running: "Working", queued: "Waiting to start" } as Record<string, string>)[a.status] || "";
  return (
    <div className="grid gap-3 rounded-md border p-3">
      <div className="grid items-center gap-2 sm:grid-cols-[auto_1fr_auto]">
        <div className="rounded-md border bg-muted px-2.5 py-1.5 text-center">
          <span className="block text-2xs tracking-wider text-muted-foreground uppercase">Lead</span>
          <strong className="block truncate font-mono text-[12px]">{lead}</strong>
          <small className="text-xs text-muted-foreground">{left ? `waiting on ${left}` : "reading reports"}</small>
        </div>
        <div className="grid gap-1.5">
          {list.map((w, i) => {
            const progress = w.status === "completed" || w.status === "error" ? 100 : Math.min(100, Math.round(((w.steps || 0) / 30) * 100));
            return (
              <button
                key={i}
                type="button"
                aria-pressed={i === picked}
                onClick={() => {
                  chosen.set(s.id!, i);
                  setPicked(i);
                }}
                className={cn("relative overflow-hidden rounded-md border px-2.5 py-1.5 text-left", i === picked && "border-foreground")}
              >
                <span className="flex items-center gap-2">
                  <span className={cn("size-2 shrink-0 rounded-full bg-current", MARK_TONE[mark(w)])} aria-hidden="true" />
                  <strong className="min-w-0 flex-1 truncate text-sm font-medium">{w.title}</strong>
                  <small className="font-mono text-xs text-faint">{tokens(w.tokens)}</small>
                </span>
                <span className="block truncate text-xs text-muted-foreground">{w.status === "completed" ? `Reported · ${plural(w.steps || 0, "step")}` : w.doing || ""}</span>
                <span className="absolute bottom-0 left-0 h-0.5 bg-blue" style={{ width: `${progress}%` }} aria-hidden="true" />
              </button>
            );
          })}
        </div>
        <div className="rounded-md border bg-muted px-2.5 py-1.5 text-center">
          <span className="block text-2xs tracking-wider text-muted-foreground uppercase">Reports</span>
          <strong className="block">
            {done} of {n}
          </strong>
          <small className="text-xs text-muted-foreground">{bad ? `${bad} failed` : left ? "coming in" : "all in"}</small>
        </div>
      </div>
      <div className="rounded-md bg-muted px-3 py-2" aria-live="polite">
        <div className="flex flex-wrap items-baseline gap-2">
          <strong className="text-md">{a.title}</strong>
          <span className={cn("text-sm", MARK_TONE[mark(a)])}>{status}</span>
          <small className="text-xs text-muted-foreground">{facts}</small>
        </div>
        {a.task && <p className="mt-1 text-sm text-text-2">Task: {a.task}</p>}
        {a.report || a.status === "error" ? (
          <>
            <p className="mt-1 text-sm">{a.report ? gist(a.report, 400) : a.doing}</p>
            {a.report && (
              <details className="mt-1">
                <summary className="cursor-pointer text-sm text-muted-foreground">Read the whole report</summary>
                <Markdown text={a.report} />
              </details>
            )}
          </>
        ) : (
          <ol className="mt-1 grid gap-0.5">
            {(a.recent?.length ? a.recent : [a.doing]).filter(Boolean).map((t: string, j: number, all: string[]) => (
              <li key={j} className="flex items-center gap-2 text-sm">
                <span className={cn("size-1.5 rounded-full bg-current", j === all.length - 1 && a.status === "running" ? "text-blue" : "text-success")} aria-hidden="true" />
                <span>{t}</span>
              </li>
            ))}
          </ol>
        )}
      </div>
    </div>
  );
}

/** What the card is working on: the workspace, a cluster, the browser or workers. */
function Target({ s, draft }: { s?: ActivityEntry; draft: unknown }) {
  if (!s && !draft) return null;
  const cls = "inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs text-muted-foreground";
  if (s?.connection) {
    const st = stagesOf(s);
    return (
      <span className={cls}>
        <NamedIcon name={st.icon} className="size-3.5" />
        <span className="max-w-32 truncate text-foreground">{s.connection}</span>
        {st.name && <span>· {st.name}</span>}
      </span>
    );
  }
  if (s && browsing(s))
    return (
      <span className={cls}>
        <Globe className="size-3.5" />
        Browser
      </span>
    );
  if (s?.tool === "delegate")
    return (
      <span className={cls}>
        <Users className="size-3.5" />
        <span className="font-mono">{s.workerModel || s.agents?.[0]?.model || "Workers"}</span>
      </span>
    );
  return (
    <span className={cls}>
      <Terminal className="size-3.5" />
      Workspace
    </span>
  );
}

const RAIL_TONE: Record<string, string> = { done: "bg-success", now: "bg-blue animate-pulse", ask: "bg-amber animate-pulse", next: "bg-line-strong" };

function RunCard({ m, work }: { m: Message; work: Work }) {
  const store = useChat();
  const now = useNow(true);
  const run = runState(m, work, now);
  const id = m.id || "live";
  const [open, toggle] = useTraceOpen(id);
  const notes = store.queued(store.route.id, m);
  const pane = run.waiting[0] ? (
    <AskCard s={run.waiting[0]} now={now} />
  ) : run.graph ? (
    <WorkerGraph s={run.graph} m={m} />
  ) : (
    <div className="overflow-hidden rounded-md border bg-code">
      <div className="flex items-center gap-2 border-b px-3 py-1.5 text-xs text-muted-foreground">
        <span className="min-w-0 flex-1 truncate font-mono text-foreground">{run.pane.label}</span>
        {run.pane.note && <span>{run.pane.note}</span>}
        <span className="inline-flex items-center gap-1">
          <span className="size-1.5 animate-pulse rounded-full bg-success" aria-hidden="true" />
          live
        </span>
      </div>
      <div className="h-36 overflow-hidden px-3 py-2">
        {run.pane.kind === "draft" || run.pane.kind === "output" || run.pane.kind === "running" ? (
          <pre className="flex h-full flex-col justify-end overflow-hidden font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-text-2">
            {run.pane.kind === "running" ? (
              <>
                {run.pane.text}
                <span className="text-faint">›</span> {run.pane.label} <span className="animate-pulse">▍</span>
              </>
            ) : (
              <>
                {run.pane.text}
                {run.pane.kind === "draft" && <span className="animate-pulse">▍</span>}
              </>
            )}
          </pre>
        ) : (
          <div className="text-md text-text-2">
            {run.pane.text}
            {run.pane.kind === "thinking" && <span className="animate-pulse">▍</span>}
          </div>
        )}
      </div>
    </div>
  );
  return (
    <>
      <section className="grid gap-3 rounded-lg border bg-card p-3" aria-label="Working">
        <div className="flex items-center gap-3">
          <span className={cn("size-2.5 shrink-0 animate-pulse rounded-full", run.waiting[0] ? "bg-amber" : "bg-blue")} aria-hidden="true" />
          <div className="min-w-0 flex-1">
            <strong className="block truncate text-md font-semibold">{run.headline}</strong>
            <small className="text-xs text-muted-foreground">
              {clock(now - run.began)} · {plural(run.stats.steps, "step")}
              {run.stats.files.length ? ` · ${plural(run.stats.files.length, "file")} changed` : ""}
            </small>
          </div>
          <Target s={run.lastStep} draft={run.draft} />
        </div>
        {run.rail.length > 1 && (
          <ol className="grid auto-cols-fr grid-flow-col gap-1.5" aria-label="Stages">
            {run.rail.map((p) => (
              <li key={p.phase} className="min-w-0" aria-current={p.status === "now" || p.status === "ask" ? "step" : undefined}>
                <span className={cn("block h-1 rounded-full", RAIL_TONE[p.status])} aria-hidden="true" />
                <span className="mt-1 block truncate text-xs">
                  <span className={p.status === "next" ? "text-faint" : ""}>{p.phase}</span> <small className="text-faint">{p.note}</small>
                </span>
              </li>
            ))}
          </ol>
        )}
        {pane}
        <div className="flex items-start gap-2 text-sm text-muted-foreground">
          <Orbit className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          <span className={cn("line-clamp-1", !run.ticker && "text-faint")}>{run.ticker || "No thinking yet"}</span>
        </div>
        <div>
          <div className="flex items-center justify-between">
            <span className="text-2xs font-medium tracking-wider text-muted-foreground uppercase">Recent steps</span>
            <button type="button" className="text-sm text-muted-foreground hover:text-foreground" aria-expanded={open} onClick={toggle}>
              {open ? "Hide steps" : `Show all ${run.stats.steps}`}
            </button>
          </div>
          <ol className="mt-1 grid gap-0.5">
            {run.recent.map((s, i) =>
              s ? (
                <li key={s.id || i} className="flex items-center gap-2 text-md">
                  <span className={cn("size-1.5 shrink-0 rounded-full bg-current", MARK_TONE[s.status === "error" || s.check === "error" ? "bad" : s.status === "running" ? "now" : s.status === "declined" ? "off" : "ok"])} aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate">
                    <StepText s={s} />
                  </span>
                  <small className="text-xs text-faint">{s.status === "running" ? "now" : duration(s.durationMs)}</small>
                </li>
              ) : (
                <li key={`empty-${i}`} className="h-5" aria-hidden="true" />
              ),
            )}
          </ol>
        </div>
        {notes.map((n, i) => (
          <p key={i} className="flex items-start gap-2 rounded-md bg-muted px-2.5 py-1.5 text-sm">
            <Clock className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
            <span>
              <span className="block text-xs text-muted-foreground">Queued for the next step</span>
              {n}
            </span>
          </p>
        ))}
      </section>
      {open && <Trace m={m} work={work} />}
    </>
  );
}

function ResultCard({ m, s }: { m: Message; s: Stats }) {
  const store = useChat();
  if (!s.files.length) return null;
  const stopped = m.status !== "completed";
  const id = store.route.id;
  const previews = store.capabilities.features?.previews === true;
  return (
    <section className="grid gap-3 rounded-lg border bg-card p-3" aria-label="What changed">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <span className="text-2xs font-medium tracking-wider text-muted-foreground uppercase">
            {stopped ? "Changed so far" : "Done"}
            {s.span ? ` · ${duration(s.span)}` : ""}
          </span>
          <strong className="block text-md font-semibold">
            {plural(s.files.length, "file")} changed <span className="text-success">+{s.added}</span>
            {s.removed ? <span className="text-destructive-foreground"> −{s.removed}</span> : null}
          </strong>
          <ul className="mt-1 grid gap-0.5 text-sm">
            {s.check === "ok" && (
              <li>
                <span className="text-success">✓</span> The preview compiles
              </li>
            )}
            {s.check === "error" && (
              <li>
                <span className="text-destructive-foreground">✗</span> The preview had an error at the end
              </li>
            )}
            {s.fixes > 0 && (
              <li>
                <span className="text-amber">!</span> {s.fixes} problem{s.fixes === 1 ? "" : "s"} found and fixed on the way
              </li>
            )}
            {stopped && (
              <li>
                <span className="text-amber">!</span> Stopped before finishing{m.errorMessage ? `: ${m.errorMessage}` : ""}
              </li>
            )}
          </ul>
        </div>
        <div className="flex flex-wrap gap-1.5">
          {s.served && previews && (
            <Button size="xs" onClick={() => store.workspace.openPreview(id)}>
              <Play /> Open preview
            </Button>
          )}
          <Button size="xs" variant="outline" onClick={() => store.workspace.openFiles(id)}>
            <File /> Files
          </Button>
          {id && (
            <Button size="xs" variant="ghost" title="Download project (.zip, without dependencies)" onClick={() => store.workspace.download(id)}>
              <Download /> Download
            </Button>
          )}
        </div>
      </div>
      <div className="grid gap-0.5">
        {s.files.map((f) => (
          <button key={f.path} type="button" className="flex items-center justify-between gap-2 rounded px-1.5 py-0.5 text-left hover:bg-accent" onClick={() => store.workspace.openFile(id, f.path)}>
            <span className="min-w-0 truncate font-mono text-[12px]">{f.path}</span>
            <span className="shrink-0 text-xs">
              {f.added ? <span className="text-success">+{f.added}</span> : null}
              {f.removed ? <span className="text-destructive-foreground"> −{f.removed}</span> : null}
            </span>
          </button>
        ))}
      </div>
    </section>
  );
}

function Checkpoints({ s }: { s: Stats }) {
  const store = useChat();
  if (!s.checkpoints.length) return null;
  const list = s.checkpoints.slice(-6);
  return (
    <section className="grid gap-2" aria-label="Checkpoints">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="text-2xs font-medium tracking-wider text-muted-foreground uppercase">Checkpoints</span>
        <small className="text-xs text-muted-foreground">
          Saved after each change that compiled{s.checkpoints.length > list.length ? ` · last ${list.length} of ${s.checkpoints.length}` : ""}
        </small>
      </div>
      <div className="flex gap-2 overflow-x-auto pb-1">
        {list.map((c, i) => (
          <div key={c.id || i} className={cn("grid min-w-40 shrink-0 gap-0.5 rounded-md border px-2.5 py-2", i === list.length - 1 && "border-foreground/40")}>
            <strong className="truncate text-sm font-medium">{c.summary}</strong>
            <small className="truncate text-xs text-muted-foreground">{c.stat || "Project saved"}</small>
            <button type="button" className="justify-self-start text-xs text-muted-foreground hover:text-foreground hover:underline" onClick={() => store.workspace.restoreCheckpoint(store.route.id, c.sha, c.summary || "")}>
              {i === list.length - 1 ? "Restore this" : "Restore"}
            </button>
          </div>
        ))}
      </div>
    </section>
  );
}

/** Above the answer: the live card, or the result card. */
export function TurnBefore({ m }: { m: Message }) {
  const work = workOf(m);
  return <div className="mb-3 grid gap-2">{isLive(m) ? <RunCard m={m} work={work} /> : <ResultCard m={m} s={statsOf(work)} />}</div>;
}

/** Below the answer: checkpoints and the "Show work" trace. */
export function TurnAfter({ m }: { m: Message }) {
  const id = m.id || "live";
  const [open, toggle] = useTraceOpen(id);
  if (isLive(m)) return null;
  const work = workOf(m),
    s = statsOf(work);
  const facts = [plural(s.steps, "step"), s.thoughtMs ? `thought for ${duration(s.thoughtMs)}` : "", s.deadEnds ? plural(s.deadEnds, "dead end") : "", s.fixes ? `${s.fixes} fix${s.fixes === 1 ? "" : "es"}` : ""]
    .filter(Boolean)
    .join(" · ");
  return (
    <div className="mt-3 grid gap-2">
      <Checkpoints s={s} />
      <button type="button" className="flex items-center gap-2 rounded-md border px-3 py-1.5 text-left text-sm hover:bg-accent" aria-expanded={open} onClick={toggle}>
        <PanelLeft className="size-4 text-muted-foreground" />
        <strong className="font-medium">{open ? "Hide work" : "Show work"}</strong>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">{facts}</span>
        <ChevronDown className={cn("size-4 text-muted-foreground transition-transform", open && "rotate-180")} />
      </button>
      {open && <Trace m={m} work={work} />}
    </div>
  );
}

export function SteerForm({ m }: { m: Message }) {
  const store = useChat();
  const [text, setText] = useState(steerDraft.text);
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border bg-card px-3 py-2"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!text.trim() || busy) return;
        setBusy(true);
        try {
          await store.steer(text);
          setText("");
          steerDraft.text = "";
        } catch (error) {
          store.run(() => {
            throw error;
          });
        } finally {
          setBusy(false);
        }
      }}
    >
      <label htmlFor="steer-text" className="text-sm font-medium">
        Steer
      </label>
      <input
        id="steer-text"
        name="steer-text"
        autoComplete="off"
        maxLength={2000}
        placeholder={steerPlaceholder(m)}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          steerDraft.text = e.target.value;
        }}
        className="h-8 min-w-0 flex-1 basis-48 rounded-md border bg-background px-2 text-md outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      />
      <Button size="sm" type="submit" disabled={busy}>
        <Check /> Send at next step
      </Button>
    </form>
  );
}
