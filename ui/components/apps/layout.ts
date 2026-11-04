// Lays an app's step tree out as a flow, top to bottom. Nothing is dragged: every
// position comes from the tree. A sequence is a column of cards; Decide and If open
// one column per path, side by side, that rejoin at a dot; For each wraps the steps
// it runs per item in a box. Every connector is a place a step can go, so each edge
// carries the list and index an inserted step takes.

import { branches, type Definition, type Step, traceName } from "./model";

export const CARD_W = 256;
export const CARD_H = 80;
export const GAP = 56;
export const COL_GAP = 28;
export const LABEL_H = 26;
export const JOIN_H = 12;
export const END_H = 30;
export const PAD = 18;
export const LOOP_END_H = 26;

export type NodeKind = "inputs" | "step" | "label" | "join" | "end" | "loop" | "loopEnd";

export interface LaidNode {
  id: string;
  kind: NodeKind;
  x: number;
  y: number;
  width: number;
  height: number;
  /** For a step: its path in the tree, such as "steps.1.otherwise.0". */
  path?: string;
  step?: Step;
  /** For a path label: its text, the list it heads and the branch it belongs to. */
  label?: string;
  list?: string;
  branch?: Step;
}
export interface LaidEdge {
  id: string;
  source: string;
  target: string;
  /** Where a step put on this connector goes. */
  insert?: { list: string; index: number };
  /** The y where the connector turns sideways, so the paths of a branch line up. */
  centerY?: number;
  /** Where its + sits. */
  plus?: { x: number; y: number };
  kind: "flow" | "branch";
}
export interface Layout {
  nodes: LaidNode[];
  edges: LaidEdge[];
  width: number;
  height: number;
}

interface Size {
  w: number;
  h: number;
}

const stepNode = (s: Step) => `step:${s.id}`;

function measureList(list: Step[], at: string): Size {
  let w = CARD_W;
  let h = 0;
  list.forEach((s, i) => {
    const m = measureStep(s, `${at}.${i}`);
    w = Math.max(w, m.w);
    h += (i ? GAP : 0) + m.h;
  });
  return { w, h };
}
function columns(s: Step, at: string) {
  return branches(s, at).map(([label, list]) => {
    const steps = listOf(s, list, at);
    const size = measureList(steps, list);
    // A column: its label, then each step after a gap.
    return { label, list, steps, w: Math.max(CARD_W * 0.8, size.w), h: LABEL_H + (steps.length ? GAP + size.h : 0) };
  });
}
/** The steps of one of a branching step's lists, by the list's path. */
function listOf(s: Step, list: string, at: string): Step[] {
  const rest = list.slice(at.length + 1).split(".");
  let value: any = s;
  for (const key of rest) value = value?.[/^\d+$/.test(key) ? Number(key) : key];
  return Array.isArray(value) ? value : [];
}
function measureStep(s: Step, at: string): Size {
  if (s.type === "decide" || s.type === "if") {
    const cols = columns(s, at);
    const w = Math.max(CARD_W, cols.reduce((n, c) => n + c.w, 0) + COL_GAP * (cols.length - 1));
    const tallest = Math.max(...cols.map((c) => c.h));
    return { w, h: CARD_H + GAP + tallest + GAP + JOIN_H };
  }
  if (s.type === "for_each") {
    const inner = measureList(s.steps || [], `${at}.steps`);
    const innerH = (s.steps?.length ? GAP + inner.h : 0) + GAP + LOOP_END_H;
    return { w: Math.max(CARD_W, inner.w) + PAD * 2, h: PAD + CARD_H + innerH + PAD };
  }
  return { w: CARD_W, h: CARD_H };
}

/** The flow for a definition. `pending` is a step being added, drawn where it would go. */
export function layout(definition: Definition): Layout {
  const nodes: LaidNode[] = [];
  const edges: LaidEdge[] = [];
  let edgeN = 0;
  const edge = (source: string, target: string, extra: Partial<LaidEdge> = {}) => edges.push({ id: `e${edgeN++}:${source}>${target}`, source, target, kind: "flow", ...extra });

  /** Places a step centred on cx with its top at y; returns the node the flow leaves from. */
  function placeStep(s: Step, at: string, cx: number, y: number): { entry: string; exit: string } {
    const size = measureStep(s, at);
    if (s.type === "decide" || s.type === "if") {
      nodes.push({ id: stepNode(s), kind: "step", x: cx - CARD_W / 2, y, width: CARD_W, height: CARD_H, path: at, step: s });
      const cols = columns(s, at);
      const total = cols.reduce((n, c) => n + c.w, 0) + COL_GAP * (cols.length - 1);
      const top = y + CARD_H + GAP;
      const joinY = y + size.h - JOIN_H;
      const join = `join:${s.id}`;
      nodes.push({ id: join, kind: "join", x: cx - JOIN_H / 2, y: joinY, width: JOIN_H, height: JOIN_H });
      let left = cx - total / 2;
      for (const c of cols) {
        const ccx = left + c.w / 2;
        const label = `label:${c.list}`;
        const lw = Math.min(c.w, CARD_W);
        nodes.push({ id: label, kind: "label", x: ccx - lw / 2, y: top, width: lw, height: LABEL_H, label: c.label, list: c.list, branch: s });
        edge(stepNode(s), label, { kind: "branch", centerY: y + CARD_H + GAP / 2 });
        placeList(c.steps, c.list, ccx, top + LABEL_H + GAP, label, join, joinY - GAP / 2);
        left += c.w + COL_GAP;
      }
      return { entry: stepNode(s), exit: join };
    }
    if (s.type === "for_each") {
      nodes.push({ id: `loop:${s.id}`, kind: "loop", x: cx - size.w / 2, y, width: size.w, height: size.h, step: s, path: at });
      nodes.push({ id: stepNode(s), kind: "step", x: cx - CARD_W / 2, y: y + PAD, width: CARD_W, height: CARD_H, path: at, step: s });
      const end = `loopEnd:${s.id}`;
      const endY = y + size.h - PAD - LOOP_END_H;
      const endW = 120;
      nodes.push({ id: end, kind: "loopEnd", x: cx - endW / 2, y: endY, width: endW, height: LOOP_END_H, step: s });
      placeList(s.steps || [], `${at}.steps`, cx, y + PAD + CARD_H + GAP, stepNode(s), end);
      return { entry: stepNode(s), exit: end };
    }
    nodes.push({ id: stepNode(s), kind: "step", x: cx - CARD_W / 2, y, width: CARD_W, height: CARD_H, path: at, step: s });
    return { entry: stepNode(s), exit: stepNode(s) };
  }

  /** Places a list under `prev`, ending in `after`; each connector inserts into the list. */
  function placeList(list: Step[], at: string, cx: number, y: number, prev: string, after: string, lastCenterY?: number) {
    let top = y;
    list.forEach((s, i) => {
      const path = `${at}.${i}`;
      const { entry, exit } = placeStep(s, path, cx, top);
      edge(prev, entry, { insert: { list: at, index: i } });
      prev = exit;
      top += measureStep(s, path).h + GAP;
    });
    edge(prev, after, { insert: { list: at, index: list.length }, centerY: lastCenterY });
  }

  const body = measureList(definition.steps, "steps");
  const width = Math.max(CARD_W, body.w);
  // The main column is at x = 0, so the view stays put when a branch widens the flow.
  const cx = 0;
  nodes.push({ id: "inputs", kind: "inputs", x: cx - CARD_W / 2, y: 0, width: CARD_W, height: CARD_H });
  const first = CARD_H + GAP;
  const endY = first + (definition.steps.length ? body.h + GAP : 0);
  const endW = 96;
  nodes.push({ id: "end", kind: "end", x: cx - endW / 2, y: endY, width: endW, height: END_H });
  placeList(definition.steps, "steps", cx, first, "inputs", "end");
  // Each + sits just under the node it follows, in that node's column; under a For
  // each box, halfway to the next step so it is outside the box.
  const byId = new Map(nodes.map((n) => [n.id, n]));
  for (const e of edges) {
    if (!e.insert) continue;
    const s = byId.get(e.source)!;
    const t = byId.get(e.target)!;
    const bottom = s.y + s.height;
    const room = t.y - bottom;
    e.plus = { x: s.x + s.width / 2, y: s.kind === "loopEnd" || t.kind === "end" ? bottom + room / 2 : bottom + Math.min(GAP / 2, room / 2) };
  }
  return { nodes, edges, width, height: endY + END_H };
}

export interface TraceEntry {
  id: string;
  type?: string;
  title?: string;
  ms: number;
  ok: boolean;
  output?: string;
  error?: string;
  path?: string;
}
export interface StepRun {
  ok: boolean;
  ms: number;
  /** How many times it ran: more than once inside For each. */
  runs: number;
  failures: number;
  error?: string;
  /** The paths a branch took. */
  paths: Set<string>;
}
/** A try's trace by step id: total time, how often it ran, whether every run was ok. */
export function traceByStep(trace: TraceEntry[] = []): Map<string, StepRun> {
  const out = new Map<string, StepRun>();
  for (const t of trace) {
    const run = out.get(t.id) || { ok: true, ms: 0, runs: 0, failures: 0, paths: new Set<string>() };
    run.runs += 1;
    run.ms += Number(t.ms) || 0;
    if (!t.ok) {
      run.ok = false;
      run.failures += 1;
      run.error ??= t.error;
    }
    if (t.path) run.paths.add(t.path);
    out.set(t.id, run);
  }
  return out;
}
/** Whether a try went down the path a label heads. */
export function tookPath(label: LaidNode, runs: Map<string, StepRun>): boolean {
  if (!label.branch || !label.list) return false;
  const run = runs.get(label.branch.id);
  return !!run && run.paths.has(traceName(label.branch, label.list));
}
