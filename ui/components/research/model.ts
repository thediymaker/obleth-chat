// A project's research (GET /api/projects/:id/research, api/src/research): the goal,
// the experiment tree (each node a git branch in a chat's workspace) and the runs,
// with the facts the Research tab derives from them.

export interface Research {
  goal: string;
  metric: string;
  /** "min" when lower is better. */
  direction: "min" | "max" | string;
  runCommand: string;
}

export interface Experiment {
  id: string;
  parentId?: string | null;
  title: string;
  description?: string | null;
  branch?: string;
  frozen?: boolean;
  buildsOn?: string | null;
  placement?: { skipped?: string; fits?: number; confirmed?: boolean; model?: string } | null;
}

export interface Run {
  id: string;
  experimentId: string;
  number: number;
  status: string;
  metric?: number | null;
  metrics?: Record<string, number>;
  commit?: string;
  startedAt: string;
  finishedAt?: string | null;
  exitCode?: number | null;
  diffStat?: string | null;
  decision?: { skipped?: string; tested?: number; model?: string } | null;
}

export interface RunDetail {
  status?: string;
  diff?: string;
  log?: string;
  canCancel?: boolean;
}

export interface ProjectResearchData {
  research: Research | null;
  experiments: Experiment[];
  runs: Run[];
  workspace?: { conversationId: string; title: string } | null;
}

/** A metric for people: fewer digits for big numbers, four significant otherwise. */
export const fmt = (v: number | null | undefined) => (v == null || !Number.isFinite(v) ? "—" : Math.abs(v) >= 100 ? Number(v.toFixed(1)).toLocaleString() : String(Number(v.toPrecision(4))));

export const minutes = (a: string, b?: string | null, now = Date.now()) => {
  const ms = (b ? new Date(b).getTime() : now) - new Date(a).getTime();
  return ms < 60000 ? `${Math.max(1, Math.round(ms / 1000))} s` : `${Math.round(ms / 60000)} min`;
};

export function facts(data: ProjectResearchData) {
  const r = data.research!;
  const better = (a: number, b: number) => (r.direction === "min" ? a < b : a > b);
  const measured = (x: Run) => x.status === "completed" && x.metric != null;
  const runsOf = (id: string) => data.runs.filter((x) => x.experimentId === id);
  const childrenOf = (id: string) => data.experiments.filter((e) => e.parentId === id);
  const bestRun = (runs: Run[] = data.runs) => runs.filter(measured).reduce<Run | null>((b, x) => (!b || better(x.metric!, b.metric!) ? x : b), null);
  const defaultNode = () => {
    const best = bestRun(),
      last = data.runs.at(-1);
    return (best || last)?.experimentId || data.experiments[0]?.id || null;
  };
  /** The run a node shows: the picked one, a running one, its best, or its last. */
  const pickRun = (node: string | null, picked: string | null) => {
    const runs = node ? runsOf(node) : [];
    return runs.find((x) => x.id === picked) || runs.find((x) => x.status === "running") || bestRun(runs) || runs.at(-1) || null;
  };
  /** A node's label in the tree. */
  const chip = (e: Experiment): { text: string; tone: "accent" | "amber" | "green" | "" } => {
    const runs = runsOf(e.id),
      last = runs.at(-1),
      best = bestRun();
    if (runs.some((x) => x.status === "running")) return { text: "● running", tone: "accent" };
    if (!e.parentId && e.frozen) return { text: "baseline", tone: "" };
    if (e.frozen && childrenOf(e.id).length) return { text: "★ promoted", tone: "green" };
    if (e.frozen && best && best.experimentId === e.id) return { text: "★ best", tone: "green" };
    if (e.frozen) return { text: "kept", tone: "" };
    if (last && ["failed", "lost"].includes(last.status)) return { text: `${last.status} · open`, tone: "amber" };
    if (last?.status === "completed") return { text: "no metric · open", tone: "amber" };
    return { text: e.parentId ? "open" : "baseline · not run", tone: "" };
  };
  /** The tree in order, each node with its depth. */
  const tree = () => {
    const rows: { e: Experiment; depth: number }[] = [];
    const walk = (parent: string | null, depth: number) => {
      for (const e of data.experiments.filter((x) => (x.parentId || null) === parent)) {
        rows.push({ e, depth });
        walk(e.id, depth + 1);
      }
    };
    walk(null, 0);
    return rows;
  };
  /** The best result against the baseline's best, in percent. */
  const gain = () => {
    const best = bestRun();
    const base = data.experiments.find((e) => !e.parentId),
      baseBest = base ? bestRun(runsOf(base.id)) : null;
    return { best, baseBest, gain: best && baseBest && baseBest.metric ? ((best.metric! - baseBest.metric) / Math.abs(baseBest.metric)) * 100 : null };
  };
  const promoted = () => new Set(data.experiments.filter((e) => e.frozen && childrenOf(e.id).length).map((e) => e.id));
  return { better, measured, runsOf, childrenOf, bestRun, defaultNode, pickRun, chip, tree, gain, promoted };
}

/** The progress chart's geometry: each run's mark and the best-so-far line. */
export function chartOf(data: ProjectResearchData, W = 640, H = 190) {
  const f = facts(data);
  const runs = data.runs,
    points = runs.filter(f.measured);
  if (!points.length) return null;
  const x0 = 52,
    top = 14,
    bottom = 26,
    n = Math.max(runs.length, 4);
  const values = points.map((r) => r.metric!),
    lo = Math.min(...values),
    hi = Math.max(...values),
    pad = (hi - lo || Math.abs(hi) || 1) * 0.12;
  const min = lo - pad,
    max = hi + pad;
  const X = (i: number) => x0 + ((i - 1) * (W - x0 - 16)) / Math.max(n - 1, 1),
    Y = (v: number) => top + ((max - v) / (max - min)) * (H - top - bottom);
  const promoted = f.promoted(),
    best = f.bestRun();
  const line: string[] = [];
  let b: number | null = null;
  for (const r of runs) {
    if (f.measured(r) && (b == null || f.better(r.metric!, b))) b = r.metric!;
    if (b != null) line.push(`${X(r.number).toFixed(0)},${Y(b).toFixed(0)}`);
  }
  const ticks = [min + pad, (min + max) / 2, max - pad].map((v) => ({ y: Y(v), label: fmt(v) }));
  const marks = runs.map((r) => ({
    run: r,
    x: X(r.number),
    y: f.measured(r) ? Y(r.metric!) : H - bottom - 4,
    measured: f.measured(r),
    kept: f.measured(r) && (promoted.has(r.experimentId) || r.id === best?.id),
  }));
  return { W, H, x0, bottom, line: line.join(" "), ticks, marks, best };
}

/** What the decisions check found: whether the run really tested its idea, or why it wasn't asked. */
export function checkText(d: NonNullable<Run["decision"]>) {
  if (d.skipped) return { text: `Skipped: ${d.skipped}`, doubt: false };
  const found = `${Math.round((d.tested || 0) * 100)}% that it did${d.model ? ` · ${d.model}` : ""}`;
  return (d.tested || 0) >= 0.5 ? { text: `Tested its idea · ${found}`, doubt: false } : { text: found, doubt: true };
}

/** Where the node went in the tree: what the placement check found, or why it wasn't asked. */
export function placementText(d: NonNullable<Experiment["placement"]>) {
  if (d.skipped) return { text: `Skipped: ${d.skipped}`, doubt: false };
  const found = `${Math.round((d.fits || 0) * 100)}% that it fits${d.model ? ` · ${d.model}` : ""}`;
  return d.confirmed ? { text: found, doubt: true } : { text: `${(d.fits || 0) >= 0.5 ? "Fits where it is" : "Unsure it fits here"} · ${found}`, doubt: false };
}

/** A diff line's look in a run's change. */
export const diffLineKind = (l: string) => (l.startsWith("+") && !l.startsWith("+++") ? "add" : l.startsWith("-") && !l.startsWith("---") ? "del" : l.startsWith("@@") ? "hunk" : "");
