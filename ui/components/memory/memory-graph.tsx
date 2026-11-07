"use client";

// The Memory page's graph: you in the middle, the topics your memories are about
// around you, and each memory beside its topics. Drag to move, scroll to zoom, click
// a memory or topic to open it. Laid out with d3-force; drawn as SVG so it follows
// the theme.

import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type Simulation, type SimulationLinkDatum, type SimulationNodeDatum } from "d3-force";
import { Maximize, Minus, Plus } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { MEMORY_KINDS, topicColor, TOPIC_KINDS, type Memory, type Topic } from "./api";

export type Selection = { type: "memory" | "topic"; id: string } | null;

interface Node extends SimulationNodeDatum {
  id: string;
  type: "you" | "topic" | "memory";
  r: number;
  color: string;
  label: string;
  memory?: Memory;
  topic?: Topic;
}
interface Link extends SimulationLinkDatum<Node> {
  source: string | Node;
  target: string | Node;
  kind: "you" | "topic";
}
interface View {
  x: number;
  y: number;
  k: number;
}

const YOU = "you";
const DAY = 86_400_000;
const endId = (end: string | Node) => (typeof end === "string" ? end : end.id);
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const short = (text: string, n = 42) => (text.length > n ? `${text.slice(0, n - 1).trimEnd()}…` : text);

/** Nodes and links for the memories and topics, keeping the places of nodes already drawn. */
function build(memories: Memory[], topics: Topic[], initials: string, places: Map<string, Node>) {
  const byId = new Map(topics.map((t) => [t.id, t]));
  const nodes: Node[] = [{ id: YOU, type: "you", r: 24, color: "var(--foreground)", label: initials, fx: 0, fy: 0 }];
  const links: Link[] = [];
  const seed = (id: string, near?: Node): Partial<Node> => {
    const old = places.get(id);
    if (old) return { x: old.x, y: old.y, vx: old.vx, vy: old.vy };
    const angle = Math.random() * Math.PI * 2;
    const at = near ?? { x: 0, y: 0 };
    return { x: (at.x ?? 0) + Math.cos(angle) * 30, y: (at.y ?? 0) + Math.sin(angle) * 30 };
  };
  const topicNodes = new Map<string, Node>();
  for (const t of topics) {
    const node: Node = { id: `t:${t.id}`, type: "topic", r: clamp(12 + 3 * Math.sqrt(t.count), 13, 34), color: topicColor(t.kind), label: t.name, topic: t, ...seed(`t:${t.id}`) };
    topicNodes.set(t.id, node);
    nodes.push(node);
    links.push({ source: YOU, target: node.id, kind: "you" });
  }
  for (const m of memories) {
    const own = m.topics.map((id) => topicNodes.get(id)).filter((n): n is Node => !!n);
    const color = own[0] ? topicColor(byId.get(m.topics[0]!)?.kind) : "var(--muted-foreground)";
    nodes.push({ id: `m:${m.id}`, type: "memory", r: m.pinned ? 7 : 5.5, color, label: m.content, memory: m, ...seed(`m:${m.id}`, own[0]) });
    if (own.length) for (const t of own) links.push({ source: t.id, target: `m:${m.id}`, kind: "topic" });
    else links.push({ source: YOU, target: `m:${m.id}`, kind: "you" });
  }
  return { nodes, links };
}

export function MemoryGraph({
  memories,
  topics,
  selected,
  onSelect,
  query,
  initials,
  className,
}: {
  memories: Memory[];
  topics: Topic[];
  selected: Selection;
  onSelect: (s: Selection) => void;
  query: string;
  initials: string;
  className?: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const svg = useRef<SVGSVGElement>(null);
  const places = useRef(new Map<string, Node>());
  const sim = useRef<Simulation<Node, Link> | null>(null);
  const fitted = useRef(false);
  // Once the person pans, zooms or drags, the view stays where they put it.
  const touched = useRef(false);
  const sizeRef = useRef({ w: 800, h: 560 });
  const [size, setSize] = useState(sizeRef.current);
  const [view, setView] = useState<View>({ x: 400, y: 280, k: 1 });
  const [, setFrame] = useState(0);
  const [hover, setHover] = useState<string | null>(null);
  const graph = useMemo(() => build(memories, topics, initials, places.current), [memories, topics, initials]);

  // The box's size.
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => {
      const { width, height } = entry!.contentRect;
      if (!width || !height) return;
      sizeRef.current = { w: width, h: height };
      setSize(sizeRef.current);
      if (!fitted.current) setView((v) => ({ ...v, x: width / 2, y: height / 2 }));
      else if (!touched.current) fitRef.current();
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const fit = useCallback(
    (nodes = graph.nodes) => {
      if (!nodes.length) return;
      const size = sizeRef.current;
      let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
      for (const n of nodes) {
        x0 = Math.min(x0, (n.x ?? 0) - n.r);
        y0 = Math.min(y0, (n.y ?? 0) - n.r);
        x1 = Math.max(x1, (n.x ?? 0) + n.r);
        y1 = Math.max(y1, (n.y ?? 0) + n.r);
      }
      const pad = 60;
      const k = clamp(Math.min(size.w / (x1 - x0 + pad * 2), size.h / (y1 - y0 + pad * 2)), 0.35, 1.6);
      setView({ k, x: size.w / 2 - ((x0 + x1) / 2) * k, y: size.h / 2 - ((y0 + y1) / 2) * k });
    },
    [graph.nodes],
  );
  const fitRef = useRef(fit);
  fitRef.current = fit;

  // The layout: a simulation that settles, drawn each frame while it moves.
  useEffect(() => {
    const { nodes, links } = graph;
    const reduced = typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    const s = forceSimulation<Node>(nodes)
      .force(
        "link",
        forceLink<Node, Link>(links)
          .id((n) => n.id)
          .distance((l) => {
            const t = l.target as Node;
            return l.kind === "you" ? (t.type === "topic" ? 120 + t.r * 2 : 150) : 30 + (l.source as Node).r;
          })
          .strength((l) => (l.kind === "you" ? 0.35 : 0.9)),
      )
      .force(
        "charge",
        forceManyBody<Node>().strength((n) => (n.type === "you" ? -900 : n.type === "topic" ? -340 : -60)),
      )
      .force(
        "collide",
        // A topic keeps room for its name, which is drawn under it.
        forceCollide<Node>()
          .radius((n) => (n.type === "topic" ? Math.max(n.r + 14, n.label.length * 3.6 + 6) : n.type === "you" ? n.r + 16 : n.r + 3))
          .iterations(2),
      )
      .force("x", forceX<Node>(0).strength(0.045))
      .force("y", forceY<Node>(0).strength(0.045))
      .alphaDecay(0.035);
    let raf = 0;
    s.on("tick", () => {
      for (const n of nodes) places.current.set(n.id, n);
      if (!raf)
        raf = requestAnimationFrame(() => {
          raf = 0;
          setFrame((f) => f + 1);
        });
      if (!fitted.current && s.alpha() < 0.08) {
        fitted.current = true;
        fitRef.current(nodes);
      }
    });
    // Settled: fit once more, unless the person has moved the view.
    s.on("end", () => {
      if (!touched.current) fitRef.current(nodes);
    });
    if (reduced) {
      s.stop();
      s.tick(300);
      for (const n of nodes) places.current.set(n.id, n);
      fitted.current = true;
      fitRef.current(nodes);
      setFrame((f) => f + 1);
    }
    sim.current = s;
    return () => {
      s.stop();
      cancelAnimationFrame(raf);
    };
  }, [graph]);

  // Scroll to zoom around the pointer (not passive, so the page doesn't scroll).
  useEffect(() => {
    const el = svg.current;
    if (!el) return;
    const wheel = (e: WheelEvent) => {
      e.preventDefault();
      touched.current = true;
      const rect = el.getBoundingClientRect();
      const mx = e.clientX - rect.left,
        my = e.clientY - rect.top;
      setView((v) => {
        const k = clamp(v.k * Math.exp(-e.deltaY * 0.0015), 0.25, 4);
        return { k, x: mx - (mx - v.x) * (k / v.k), y: my - (my - v.y) * (k / v.k) };
      });
    };
    el.addEventListener("wheel", wheel, { passive: false });
    return () => el.removeEventListener("wheel", wheel);
  }, []);

  // Dragging: the background pans; a node moves, and a press without a move selects it.
  const drag = useRef<{ node: Node | null; sx: number; sy: number; vx: number; vy: number; moved: boolean } | null>(null);
  const toGraph = (clientX: number, clientY: number) => {
    const rect = svg.current!.getBoundingClientRect();
    return { x: (clientX - rect.left - view.x) / view.k, y: (clientY - rect.top - view.y) / view.k };
  };
  function down(e: React.PointerEvent, node: Node | null) {
    if (e.button !== 0) return;
    e.stopPropagation();
    (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
    drag.current = { node, sx: e.clientX, sy: e.clientY, vx: view.x, vy: view.y, moved: false };
  }
  function move(e: React.PointerEvent) {
    const d = drag.current;
    if (!d) return;
    if (Math.abs(e.clientX - d.sx) + Math.abs(e.clientY - d.sy) > 4) d.moved = true;
    if (!d.moved) return;
    touched.current = true;
    if (d.node && d.node.type !== "you") {
      const p = toGraph(e.clientX, e.clientY);
      d.node.fx = p.x;
      d.node.fy = p.y;
      sim.current?.alphaTarget(0.25).restart();
    } else if (!d.node) {
      setView((v) => ({ ...v, x: d.vx + e.clientX - d.sx, y: d.vy + e.clientY - d.sy }));
    }
  }
  function up() {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (d.node && d.node.type !== "you") {
      d.node.fx = null;
      d.node.fy = null;
      sim.current?.alphaTarget(0);
    }
    if (!d.moved) select(d.node);
  }
  function select(node: Node | null) {
    if (!node || node.type === "you") onSelect(null);
    else if (node.type === "topic") onSelect({ type: "topic", id: node.topic!.id });
    else onSelect({ type: "memory", id: node.memory!.id });
  }

  // What stands out: the node under the pointer or the one selected, and its neighbours.
  const focus = hover ?? (selected ? `${selected.type === "topic" ? "t" : "m"}:${selected.id}` : null);
  const near = useMemo(() => {
    if (!focus) return null;
    const set = new Set([focus]);
    for (const l of graph.links) {
      const [a, b] = [endId(l.source), endId(l.target)];
      if (a === focus) set.add(b);
      if (b === focus) set.add(a);
    }
    return set;
  }, [focus, graph.links]);
  const q = query.trim().toLowerCase();
  const matches = (n: Node) => !q || n.type === "you" || n.label.toLowerCase().includes(q);
  const dim = (n: Node) => (near ? !near.has(n.id) : !matches(n));
  const now = Date.now();
  const lit = (l: Link) => !!near && near.has(endId(l.source)) && near.has(endId(l.target)) && (endId(l.source) === focus || endId(l.target) === focus);

  function zoom(by: number) {
    touched.current = true;
    setView((v) => {
      const k = clamp(v.k * by, 0.25, 4);
      const [cx, cy] = [size.w / 2, size.h / 2];
      return { k, x: cx - (cx - v.x) * (k / v.k), y: cy - (cy - v.y) * (k / v.k) };
    });
  }

  return (
    <div ref={box} className={cn("relative h-full min-h-[420px] w-full overflow-hidden rounded-lg border bg-card", className)}>
      <svg
        ref={svg}
        role="img"
        aria-label={`Memory graph: ${memories.length} memories about ${topics.length} topics`}
        width={size.w}
        height={size.h}
        className="block cursor-grab touch-none select-none active:cursor-grabbing"
        onPointerDown={(e) => down(e, null)}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
      >
        <defs>
          <radialGradient id="memory-you" cx="50%" cy="40%" r="65%">
            <stop offset="0%" stopColor="var(--foreground)" stopOpacity="1" />
            <stop offset="100%" stopColor="var(--foreground)" stopOpacity="0.78" />
          </radialGradient>
          <pattern id="memory-dots" width="24" height="24" patternUnits="userSpaceOnUse">
            <circle cx="1" cy="1" r="1" fill="var(--line-strong)" />
          </pattern>
        </defs>
        <rect width="100%" height="100%" fill="url(#memory-dots)" opacity="0.55" />
        <g transform={`translate(${view.x},${view.y}) scale(${view.k})`}>
          <g>
            {graph.links.map((l, i) => {
              const s = l.source as Node,
                t = l.target as Node;
              if (s.x == null || t.x == null) return null;
              const on = lit(l);
              const faded = (near && !on) || (!near && (!matches(s) || !matches(t)));
              return (
                <line
                  key={i}
                  x1={s.x}
                  y1={s.y}
                  x2={t.x}
                  y2={t.y}
                  stroke={on ? t.color : l.kind === "you" ? "var(--line-strong)" : "var(--border)"}
                  strokeWidth={(on ? 1.8 : l.kind === "you" ? 1.2 : 1) / Math.sqrt(view.k)}
                  strokeOpacity={faded ? 0.25 : on ? 0.95 : 0.9}
                  className={on ? "memory-flow" : undefined}
                />
              );
            })}
          </g>
          {graph.nodes.map((n) => {
            if (n.x == null || n.y == null) return null;
            const isFocus = n.id === focus;
            const faded = dim(n);
            const showLabel = n.type !== "memory" || isFocus || (near?.has(n.id) && focus?.startsWith("t:")) || (!!q && matches(n)) || view.k > 1.7;
            const recent = n.memory?.usedAt && now - new Date(n.memory.usedAt).getTime() < DAY;
            const name =
              n.type === "you"
                ? "You"
                : n.type === "topic"
                  ? `${TOPIC_KINDS[n.topic!.kind]?.[0] ?? "Topic"}: ${n.label}, ${n.topic!.count} ${n.topic!.count === 1 ? "memory" : "memories"}`
                  : `${MEMORY_KINDS[n.memory!.kind]?.[0] ?? "Memory"}: ${n.label}`;
            return (
              <g
                key={n.id}
                transform={`translate(${n.x},${n.y})`}
                opacity={faded ? 0.22 : 1}
                className="cursor-pointer outline-none transition-opacity duration-200"
                role="button"
                tabIndex={n.type === "you" ? -1 : 0}
                aria-label={name}
                aria-pressed={isFocus}
                onPointerDown={(e) => down(e, n)}
                onPointerEnter={() => setHover(n.id)}
                onPointerLeave={() => setHover((h) => (h === n.id ? null : h))}
                onFocus={() => setHover(n.id)}
                onBlur={() => setHover((h) => (h === n.id ? null : h))}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    select(n);
                  }
                }}
              >
                <title>{name}</title>
                {n.type === "you" && (
                  <>
                    <circle r={n.r + 10} fill="var(--foreground)" opacity="0.06" />
                    <circle r={n.r} fill="url(#memory-you)" />
                    <text textAnchor="middle" dy="0.35em" fontSize={13} fontWeight={600} fill="var(--background)">
                      {n.label}
                    </text>
                  </>
                )}
                {n.type === "topic" && (
                  <>
                    {isFocus && <circle r={n.r + 7} fill="none" stroke={n.color} strokeOpacity={0.5} strokeWidth={1.5} />}
                    <circle r={n.r} fill={n.color} fillOpacity={0.16} stroke={n.color} strokeWidth={1.6} />
                    <circle r={Math.max(3, n.r * 0.28)} fill={n.color} />
                  </>
                )}
                {n.type === "memory" && (
                  <>
                    {recent && <circle r={n.r + 5} fill={n.color} className="memory-glow" />}
                    {isFocus && <circle r={n.r + 5} fill="none" stroke="var(--foreground)" strokeWidth={1.5} />}
                    <circle r={n.r} fill={n.color} stroke="var(--card)" strokeWidth={1.5} />
                    {n.memory!.pinned && <circle r={n.r + 2.5} fill="none" stroke={n.color} strokeWidth={1.2} strokeDasharray="2 2" />}
                  </>
                )}
                {showLabel && n.type !== "you" && (
                  <text
                    textAnchor="middle"
                    y={n.r + (n.type === "topic" ? 15 : 13)}
                    fontSize={(n.type === "topic" ? 12.5 : 11.5) / Math.max(1, Math.sqrt(view.k))}
                    fontWeight={n.type === "topic" ? 600 : 400}
                    fill={n.type === "topic" ? "var(--foreground)" : "var(--muted-foreground)"}
                    stroke="var(--card)"
                    strokeWidth={3}
                    paintOrder="stroke"
                    className="pointer-events-none"
                  >
                    {n.type === "topic" ? n.label : short(n.label, isFocus ? 64 : 42)}
                  </text>
                )}
              </g>
            );
          })}
        </g>
      </svg>
      <div className="absolute top-3 right-3 flex flex-col gap-1">
        <Button variant="outline" size="icon-xs" className="size-7 bg-card" aria-label="Zoom in" title="Zoom in" onClick={() => zoom(1.25)}>
          <Plus />
        </Button>
        <Button variant="outline" size="icon-xs" className="size-7 bg-card" aria-label="Zoom out" title="Zoom out" onClick={() => zoom(0.8)}>
          <Minus />
        </Button>
        <Button variant="outline" size="icon-xs" className="size-7 bg-card" aria-label="Fit to view" title="Fit to view" onClick={() => fit()}>
          <Maximize />
        </Button>
      </div>
      <div className="pointer-events-none absolute bottom-3 left-3 flex flex-wrap gap-x-3 gap-y-1 rounded-md border bg-card/90 px-2.5 py-1.5 text-xs text-muted-foreground backdrop-blur-sm">
        {Object.entries(TOPIC_KINDS)
          .filter(([kind]) => topics.some((t) => t.kind === kind))
          .map(([kind, [label, color]]) => (
            <span key={kind} className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-full border" style={{ background: color, borderColor: color }} />
              {label}
            </span>
          ))}
        {memories.some((m) => m.usedAt && now - new Date(m.usedAt).getTime() < DAY) && (
          <span className="flex items-center gap-1.5">
            <span className="size-2.5 animate-pulse rounded-full bg-foreground/60" />
            Used today
          </span>
        )}
      </div>
    </div>
  );
}
