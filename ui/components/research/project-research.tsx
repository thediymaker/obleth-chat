"use client";

// A project's Research tab: the goal, the experiment tree (each
// node a git branch in a chat's workspace), progress across runs, and the evidence
// behind each result. The chat drives the research with its tools; this page shows
// it, refreshes every 5 seconds while a run is going, and can stop runs.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { BarChart3, FlaskConical, Square, Terminal } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { toast } from "sonner";
import { ErrorBox, Eyebrow } from "@/components/app-shell/page";
import { useStartInProject } from "@/components/code/start-chat";
import { Pill } from "@/components/manage/kit";
import { useProjectFrame } from "@/components/projects/project-frame";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { useFeature } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { type Experiment, type ProjectResearchData, type Run, type RunDetail, chartOf, checkText, diffLineKind, facts, fmt, minutes, placementText } from "./model";

const card = "rounded-lg border bg-card p-4";
export const researchKey = (projectId: string) => ["project-research", projectId] as const;
const runKey = (id: string) => ["research-run", id] as const;

const STEPS: [string, string][] = [
  ["Say the goal", "In a chat in this project, with a workspace: “make decode faster without hurting p95”."],
  ["Answer its questions", "It pins down the metric, whether higher or lower is better, and the one command every run uses."],
  ["Baseline", "Your code as it is becomes the baseline and runs once."],
  ["Rounds", "A few options per decision run side by side; the winner becomes the parent of the next round."],
];

function Start({ projectId, mine }: { projectId: string; mine: boolean }) {
  const workspaces = useFeature("workspaces");
  const startIn = useStartInProject();
  const can = mine && workspaces;
  return (
    <section className={cn(card, "grid gap-6 p-6")}>
      <div className="flex flex-col items-center gap-3 text-center">
        <span className="grid size-10 place-items-center rounded-full border bg-background">
          <FlaskConical className="size-5 text-muted-foreground" />
        </span>
        <h2 className="text-lg font-semibold">No research yet</h2>
        <p className="max-w-lg text-sm text-muted-foreground">Research measures ideas against each other: every experiment is a branch of your code, run with the same command, and the best result carries into the next round.</p>
        {can ? (
          <>
            <Button onClick={() => startIn(projectId, { auto: true, prompt: "Let’s set up research in this project. The goal: " })}>
              <FlaskConical /> Start research in a chat
            </Button>
            <p className="max-w-lg text-sm text-muted-foreground">Already started one? It shows here once the chat has agreed the metric and run command with you and set up the baseline. Until then, follow it in the chat.</p>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">{mine ? "Research runs in a workspace, and workspaces are not enabled for your account." : "Only the project’s owner can start research in it."}</p>
        )}
      </div>
      <ol className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {STEPS.map(([title, text], i) => (
          <li key={title} className="grid content-start gap-1 rounded-md border bg-background p-3">
            <span className="grid size-6 place-items-center rounded-full bg-secondary text-xs font-semibold">{i + 1}</span>
            <strong className="text-md font-medium">{title}</strong>
            <p className="text-sm text-muted-foreground">{text}</p>
          </li>
        ))}
      </ol>
    </section>
  );
}

function Goal({ data }: { data: ProjectResearchData }) {
  const r = data.research!;
  const { best, baseBest, gain } = facts(data).gain();
  const going = data.runs.filter((x) => x.status === "running").length;
  const chip = "inline-flex max-w-full items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-sm";
  return (
    <section className={cn(card, "mb-4 flex flex-wrap gap-6")}>
      <div className="grid min-w-0 grow basis-80 content-start gap-2">
        <Eyebrow>Goal</Eyebrow>
        <p className="text-base">{r.goal}</p>
        <div className="flex flex-wrap gap-1.5">
          <span className={chip}>
            <BarChart3 className="size-3.5 text-muted-foreground" />
            {r.metric} {r.direction === "min" ? "↓ lower is better" : "↑ higher is better"}
          </span>
          <span className={cn(chip, "font-mono text-xs")} title="The same command on every node">
            <span className="truncate">{r.runCommand}</span>
          </span>
          {data.workspace && (
            <Link href={`/chat/${encodeURIComponent(data.workspace.conversationId)}`} className={cn(chip, "hover:bg-accent")}>
              <Terminal className="size-3.5 text-muted-foreground" />
              Runs in “{data.workspace.title}”
            </Link>
          )}
        </div>
      </div>
      <div className="flex gap-6">
        <div className="grid content-start gap-0.5">
          <Eyebrow>Best so far</Eyebrow>
          <strong className="text-2xl font-semibold tabular-nums">{best ? fmt(best.metric) : "—"}</strong>
          <small className="text-xs text-muted-foreground">{best ? `run ${best.number}${gain != null && best.id !== baseBest?.id ? ` · ${gain >= 0 ? "+" : ""}${gain.toFixed(1)}% over baseline` : ""}` : "No measured runs yet"}</small>
        </div>
        <div className="grid content-start gap-0.5">
          <Eyebrow>Runs</Eyebrow>
          <strong className="text-2xl font-semibold tabular-nums">{data.runs.length}</strong>
          <small className="text-xs text-muted-foreground">{going ? `${going} running` : "none running"}</small>
        </div>
      </div>
    </section>
  );
}

const TONE: Record<string, "accent" | "amber" | ""> = { accent: "accent", amber: "amber", green: "accent", "": "" };

function Tree({ data, selected, onSelect }: { data: ProjectResearchData; selected: string | null; onSelect: (id: string) => void }) {
  const f = facts(data);
  return (
    <section className={card}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Eyebrow>Experiment tree</Eyebrow>
        <span className="text-xs text-faint">Each node is a branch; every run is tied to its commit</span>
      </div>
      <div className="mt-2 grid grid-cols-[minmax(0,1fr)] gap-0.5">
        {f.tree().map(({ e, depth }) => {
          const runs = f.runsOf(e.id),
            best = f.bestRun(runs),
            last = runs.at(-1),
            chip = f.chip(e);
          return (
            <button
              key={e.id}
              type="button"
              aria-pressed={selected === e.id}
              onClick={() => onSelect(e.id)}
              style={{ paddingLeft: `${0.5 + depth * 1.25}rem` }}
              className={cn("flex items-center gap-2.5 rounded-md py-1.5 pr-2 text-left hover:bg-accent", selected === e.id && "bg-secondary")}
            >
              <code className="w-14 shrink-0 font-mono text-xs text-faint">{(last?.commit || "").slice(0, 7)}</code>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-md">{e.title}</span>
                {e.description && <small className="block truncate text-xs text-muted-foreground">{e.description}</small>}
              </span>
              <span className="shrink-0 font-mono text-sm tabular-nums">{best ? fmt(best.metric) : last?.status === "running" ? "…" : "—"}</span>
              <Pill tone={TONE[chip.tone]} className={cn(chip.tone === "green" && "text-success")}>
                {chip.text}
              </Pill>
            </button>
          );
        })}
      </div>
    </section>
  );
}

function Chart({ data }: { data: ProjectResearchData }) {
  const c = chartOf(data);
  if (!c) return null;
  return (
    <section className={card}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Eyebrow>Progress</Eyebrow>
        <span className="flex flex-wrap gap-3 text-xs text-muted-foreground">
          <span className="flex items-center gap-1">
            <i className="size-2 rounded-full bg-success" />
            promoted or best
          </span>
          <span className="flex items-center gap-1">
            <i className="size-2 rounded-full bg-muted-foreground/50" />
            not kept
          </span>
          <span>× answered nothing</span>
        </span>
      </div>
      <svg viewBox={`0 0 ${c.W} ${c.H}`} role="img" aria-label={`${data.research!.metric} by run; the best so far is ${fmt(c.best?.metric)}`} className="mt-2 w-full text-muted-foreground">
        {c.ticks.map((t, i) => (
          <g key={i}>
            <line x1={c.x0} x2={c.W - 8} y1={t.y.toFixed(0)} y2={t.y.toFixed(0)} className="stroke-border" strokeWidth={1} />
            <text x={c.x0 - 6} y={(t.y + 4).toFixed(0)} textAnchor="end" className="fill-current text-[10px]">
              {t.label}
            </text>
          </g>
        ))}
        <polyline points={c.line} fill="none" className="stroke-success" strokeWidth={2} strokeLinejoin="round" />
        {c.marks.map((m) =>
          m.measured ? (
            <circle key={m.run.id} cx={m.x.toFixed(0)} cy={m.y.toFixed(0)} r={m.kept ? 5 : 4.5} className={m.kept ? "fill-success" : "fill-muted-foreground/50"}>
              <title>
                Run {m.run.number}: {fmt(m.run.metric)}
              </title>
            </circle>
          ) : (
            <text key={m.run.id} x={m.x.toFixed(0)} y={m.y.toFixed(0)} textAnchor="middle" className={cn("text-[12px]", m.run.status === "running" ? "fill-blue" : "fill-destructive")}>
              {m.run.status === "running" ? "…" : "×"}
              <title>
                Run {m.run.number}: {m.run.status}
              </title>
            </text>
          ),
        )}
        {c.marks.map((m) => (
          <text key={`n${m.run.id}`} x={m.x.toFixed(0)} y={c.H - 6} textAnchor="middle" className="fill-current text-[10px]">
            {m.run.number}
          </text>
        ))}
      </svg>
    </section>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-3 border-b border-line-soft py-1.5 text-sm last:border-b-0">
      <span className="text-muted-foreground">{label}</span>
      <span className="min-w-0 text-right">{children}</span>
    </div>
  );
}

function Detail({ projectId, data, e, picked, onPick }: { projectId: string; data: ProjectResearchData; e: Experiment; picked: string | null; onPick: (id: string) => void }) {
  const queries = useQueryClient();
  const f = facts(data);
  const runs = f.runsOf(e.id);
  const summary = f.pickRun(e.id, picked);
  const metric = data.research!.metric;
  const detail = useQuery({
    queryKey: [...runKey(summary?.id || ""), summary?.status],
    queryFn: async () => (await api.request<{ run: RunDetail }>(`/api/research/runs/${encodeURIComponent(summary!.id)}`)).run,
    enabled: !!summary,
    staleTime: summary?.status === "running" ? 0 : Infinity,
    refetchInterval: summary?.status === "running" ? 5000 : false,
  });
  const run = detail.data;
  const [stopping, setStopping] = useState(false);
  async function cancel(r: Run) {
    setStopping(true);
    try {
      await api.request(`/api/research/runs/${encodeURIComponent(r.id)}/cancel`, { method: "POST" });
      toast("Run stopped");
      await queries.invalidateQueries({ queryKey: researchKey(projectId) });
      await queries.invalidateQueries({ queryKey: runKey(r.id) });
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setStopping(false);
    }
  }
  const values = Object.entries(summary?.metrics || {})
    .sort(([a], [b]) => Number(b === metric) - Number(a === metric))
    .slice(0, 12);
  const placement = e.placement ? placementText(e.placement) : null;
  return (
    <section className={cn(card, "grid content-start gap-3")}>
      <div>
        <Eyebrow>{e.parentId ? "Experiment" : "Baseline"}</Eyebrow>
        <h3 className="mt-0.5 text-lg font-semibold">{e.title}</h3>
        <p className="font-mono text-xs text-faint">
          {e.branch} · {e.frozen ? "frozen" : "open"}
        </p>
        {e.description && <p className="mt-2 text-md text-text-2">{e.description}</p>}
      </div>
      {(e.buildsOn || placement) && (
        <div>
          {e.buildsOn && <Row label="Builds on">{e.buildsOn}</Row>}
          {placement && (
            <Row label="Placement">
              {placement.doubt && <Pill tone="amber">Placed here despite a doubt</Pill>} {placement.text}
            </Row>
          )}
        </div>
      )}
      {runs.length ? (
        <div className="flex flex-wrap gap-1.5">
          {runs.map((r) => (
            <button
              key={r.id}
              type="button"
              aria-pressed={r.id === summary?.id}
              onClick={() => onPick(r.id)}
              className={cn("grid rounded-md border px-2.5 py-1 text-left text-sm hover:bg-accent", r.id === summary?.id && "border-line-strong bg-secondary")}
            >
              Run {r.number}
              <span className="font-mono text-xs text-muted-foreground">{r.status === "completed" ? fmt(r.metric) : r.status}</span>
            </button>
          ))}
        </div>
      ) : (
        !e.frozen && <p className="text-sm text-muted-foreground">Not run yet.</p>
      )}
      {summary && (
        <>
          <div>
            <Row label={metric}>{summary.status === "running" ? "running…" : fmt(summary.metric)}</Row>
            <Row label="Status">{summary.status + (summary.exitCode != null && summary.exitCode !== 0 ? ` · exit ${summary.exitCode}` : "")}</Row>
            <Row label="Commit">
              <span className="font-mono text-xs">{summary.commit}</span>
            </Row>
            <Row label="Time">{`${minutes(summary.startedAt, summary.finishedAt)}${summary.finishedAt ? "" : " so far"}`}</Row>
            {summary.diffStat && <Row label="Change">{summary.diffStat}</Row>}
            {summary.decision && (
              <Row label="Check">
                {(() => {
                  const c = checkText(summary.decision);
                  return (
                    <>
                      {c.doubt && <Pill tone="amber">May not have tested its idea</Pill>} {c.text}
                    </>
                  );
                })()}
              </Row>
            )}
          </div>
          {values.length > 1 && (
            <div className="grid gap-1.5">
              <Eyebrow>Values it printed</Eyebrow>
              <div className="flex flex-wrap gap-1.5">
                {values.map(([k, v]) => (
                  <span key={k} className="rounded-md border px-2 py-0.5 font-mono text-xs">
                    <b className="mr-1 font-medium">{k}</b>
                    {fmt(v)}
                  </span>
                ))}
              </div>
            </div>
          )}
          {run?.diff && (
            <div className="grid gap-1.5">
              <Eyebrow>Change from parent</Eyebrow>
              <pre className="max-h-80 overflow-auto rounded-md bg-code p-2 font-mono text-[12px] leading-relaxed">
                {run.diff
                  .split("\n")
                  .slice(0, 400)
                  .map((l, i) => {
                    const k = diffLineKind(l);
                    return (
                      <span key={i} className={cn("block", k === "add" && "bg-success/12 text-success", k === "del" && "bg-destructive/12 text-destructive-foreground", k === "hunk" && "text-muted-foreground")}>
                        {l || " "}
                      </span>
                    );
                  })}
              </pre>
            </div>
          )}
          <div className="grid gap-1.5">
            <Eyebrow>Output</Eyebrow>
            {run ? (
              <pre className="max-h-80 overflow-auto rounded-md bg-terminal p-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap">{(run.log || "").split("\n").slice(-60).join("\n") || "Nothing printed yet."}</pre>
            ) : (
              <p className="text-sm text-muted-foreground">Loading…</p>
            )}
          </div>
          {run?.canCancel && (
            <div>
              <Button size="sm" variant="destructive" disabled={stopping} onClick={() => cancel(summary)}>
                <Square /> Stop this run
              </Button>
            </div>
          )}
        </>
      )}
    </section>
  );
}

export function ProjectResearch({ projectId }: { projectId: string }) {
  const { detail } = useProjectFrame();
  const q = useQuery({
    queryKey: researchKey(projectId),
    queryFn: async () => {
      const data = await api.request<ProjectResearchData>(`/api/projects/${encodeURIComponent(projectId)}/research`);
      return data.research ? data : null;
    },
    refetchInterval: (query) => (query.state.data?.runs.some((r) => r.status === "running") ? 5000 : false),
  });
  const [selected, setSelected] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  if (q.isPending) return <p role="status" className="text-sm text-muted-foreground">Loading research…</p>;
  if (q.error) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />;
  const data = q.data;
  if (!data) return <Start projectId={projectId} mine={!!detail.project.mine} />;
  const f = facts(data);
  const node = data.experiments.some((e) => e.id === selected) ? selected : f.defaultNode();
  const e = data.experiments.find((x) => x.id === node);
  return (
    <div>
      <Goal data={data} />
      <div className="grid grid-cols-[minmax(0,1fr)] gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,26rem)]">
        <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] content-start gap-4">
          <Tree
            data={data}
            selected={node}
            onSelect={(id) => {
              setSelected(id);
              setPicked(null);
            }}
          />
          <Chart data={data} />
        </div>
        <div className="min-w-0">{e && <Detail key={e.id} projectId={projectId} data={data} e={e} picked={picked} onPick={setPicked} />}</div>
      </div>
    </div>
  );
}
