"use client";

// The app's steps drawn with React Flow. Positions come from layout.ts; nothing is
// dragged or connected by hand. Cards select a step, a + on any connector puts a new
// step there (pick a type, or drop one from the palette), and after a try each card
// shows its time and whether it ran.

import "@xyflow/react/dist/style.css";
import "./flow.css";
import {
  Background,
  BackgroundVariant,
  BaseEdge,
  ControlButton,
  Controls,
  EdgeLabelRenderer,
  type Edge,
  type EdgeProps,
  getSmoothStepPath,
  Handle,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  useStore,
} from "@xyflow/react";
import { Check, CircleAlert, Maximize, Plus, Repeat, TextCursorInput } from "lucide-react";
import { createContext, memo, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { isInside, KINDS, type Definition, type Field, staysPut, type StepType, summary, TYPE, TYPES } from "./model";
import { type LaidEdge, type LaidNode, layout, type StepRun, tookPath } from "./layout";
import { StepIcon } from "./step-icon";

export const DRAG_TYPE = "application/x-obleth-app-step";

export type Selection = { kind: "inputs" } | { kind: "step"; path: string } | null;

interface CanvasContext {
  selection: Selection;
  pendingPath: string | null;
  /** The + a dragged step or palette item would land on, as "list@index". */
  dropTarget: string | null;
  /** The step being dragged, whose own steps go with it. */
  dragging: string | null;
  runs: Map<string, StepRun> | null;
  inputs: Field[];
  onInsert: (list: string, index: number, type: StepType) => void;
  busy: boolean;
}
const Ctx = createContext<CanvasContext | null>(null);
const useCanvas = () => useContext(Ctx)!;

const hidden = "!pointer-events-none !size-px !min-h-0 !min-w-0 !border-0 !bg-transparent !opacity-0";
function Handles() {
  return (
    <>
      <Handle type="target" position={Position.Top} isConnectable={false} className={hidden} />
      <Handle type="source" position={Position.Bottom} isConnectable={false} className={hidden} />
    </>
  );
}

type Data = { laid: LaidNode };

const InputsNode = memo(function InputsNode() {
  const { selection, inputs } = useCanvas();
  const on = selection?.kind === "inputs";
  const kind = (f: Field) => (KINDS.find(([k]) => k === (f.type || "text")) || KINDS[0])[1];
  return (
    <div
      className={cn(
        "flex h-full w-full cursor-pointer items-center gap-3 rounded-xl border bg-card px-3 shadow-[0_1px_2px_var(--shadow)] transition-colors hover:border-line-strong",
        on && "border-ring ring-2 ring-ring/40",
      )}
    >
      <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-secondary text-text-2">
        <TextCursorInput className="size-4" />
      </span>
      <div className="min-w-0 grow">
        <div className="text-[15px] font-medium">Inputs</div>
        <div className="truncate text-md text-muted-foreground">
          {inputs.length ? inputs.map((f) => `${f.name}${f.required ? "" : "?"} · ${kind(f).toLowerCase()}`).join(", ") : "No inputs"}
        </div>
      </div>
      <Handles />
    </div>
  );
});

const StepNode = memo(function StepNode({ data }: NodeProps<Node<Data>>) {
  const { selection, pendingPath, runs, dragging } = useCanvas();
  const s = data.laid.step!;
  const path = data.laid.path!;
  const t = TYPE[s.type] || { name: s.type };
  const on = selection?.kind === "step" && selection.path === path;
  const pending = pendingPath === path;
  const run = runs?.get(s.id);
  const skipped = runs && !run && !pending;
  const carried = !!dragging && path.startsWith(`${dragging}.`);
  return (
    <div
      className={cn(
        "group relative flex h-full w-full cursor-grab items-center gap-3 rounded-xl border bg-card px-3.5 shadow-[0_1px_2px_var(--shadow)] transition-[border-color,opacity,box-shadow] hover:border-line-strong active:cursor-grabbing",
        dragging === path && "shadow-[0_12px_28px_var(--shadow)] ring-2 ring-ring/40",
        carried && "opacity-30",
        on && "border-ring ring-2 ring-ring/40",
        pending && "border-dashed border-line-strong",
        run && !run.ok && "border-destructive/60",
        skipped && "opacity-55",
      )}
      title={run?.error || undefined}
      data-step={path}
    >
      <StepIcon type={s.type} />
      <div className="min-w-0 grow">
        <div className="flex items-center gap-1.5">
          <span className="truncate text-[15px] font-medium">{s.title || t.name}</span>
          {pending && <span className="shrink-0 rounded-full bg-secondary px-1.5 text-2xs text-muted-foreground">New</span>}
        </div>
        <div className="truncate text-md text-muted-foreground">{s.title ? `${t.name} · ${summary(s)}` : summary(s)}</div>
        <div className="mt-0.5 flex items-center gap-2 text-xs">
          <code className="truncate font-mono text-faint">{s.id}</code>
        </div>
      </div>
      {run && (
        <span
          className={cn(
            "absolute -top-2.5 right-2 inline-flex items-center gap-1 rounded-full border bg-card px-1.5 py-px text-2xs font-medium",
            run.ok ? "text-success" : "border-destructive/50 text-destructive-foreground",
          )}
        >
          {run.ok ? <Check className="size-3" /> : <CircleAlert className="size-3" />}
          {run.ok ? `${run.ms} ms` : "failed"}
          {run.runs > 1 && <span className="text-faint">×{run.runs}</span>}
        </span>
      )}
      <Handles />
    </div>
  );
});

const LabelNode = memo(function LabelNode({ data }: NodeProps<Node<Data>>) {
  const { runs } = useCanvas();
  const taken = runs ? tookPath(data.laid, runs) : false;
  return (
    <div className="flex h-full w-full items-center justify-center">
      <span
        title={data.laid.label}
        className={cn(
          "max-w-full truncate rounded-full border bg-background px-2.5 py-0.5 text-sm text-text-2",
          taken && "border-success/60 text-success",
          runs && !taken && "text-faint",
        )}
      >
        {data.laid.label}
      </span>
      <Handles />
    </div>
  );
});

const JoinNode = memo(function JoinNode() {
  return (
    <div className="flex h-full w-full items-center justify-center">
      <span className="size-2.5 rounded-full border-2 border-line-strong bg-background" />
      <Handles />
    </div>
  );
});

const EndNode = memo(function EndNode() {
  return (
    <div className="flex h-full w-full items-center justify-center rounded-full border border-dashed bg-background text-xs text-muted-foreground">
      End
      <Handles />
    </div>
  );
});

const LoopNode = memo(function LoopNode({ data }: NodeProps<Node<Data>>) {
  const { selection } = useCanvas();
  const on = selection?.kind === "step" && selection.path === data.laid.path;
  return <div className={cn("h-full w-full rounded-2xl border border-dashed bg-muted/50", on && "border-ring/60")} />;
});

const LoopEndNode = memo(function LoopEndNode() {
  return (
    <div className="flex h-full w-full items-center justify-center gap-1 rounded-full border bg-background text-2xs text-muted-foreground">
      <Repeat className="size-3" /> Next item
      <Handles />
    </div>
  );
});

const nodeTypes = { inputs: InputsNode, step: StepNode, label: LabelNode, join: JoinNode, end: EndNode, loop: LoopNode, loopEnd: LoopEndNode };

/** A menu of step types, for a + on a connector. */
export function TypeMenu({ onPick }: { onPick: (type: StepType) => void }) {
  return (
    <div className="grid gap-0.5" role="menu">
      {TYPES.map(([type, name]) => (
        <button key={type} type="button" role="menuitem" onClick={() => onPick(type)} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-md hover:bg-accent">
          <StepIcon type={type} size="sm" /> {name}
        </button>
      ))}
    </div>
  );
}

type EdgeData = { laid: LaidEdge };
const InsertEdge = memo(function InsertEdge({ id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data }: EdgeProps<Edge<EdgeData>>) {
  const { onInsert, busy, dropTarget, dragging } = useCanvas();
  const [open, setOpen] = useState(false);
  const [over, setOver] = useState(false);
  const laid = data!.laid;
  const [path] = getSmoothStepPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, borderRadius: 12, centerY: laid.centerY, offset: 0 });
  const insert = laid.insert;
  const plus = laid.plus;
  return (
    <>
      <BaseEdge id={id} path={path} style={{ stroke: "var(--line-strong)", strokeWidth: 1.5 }} />
      {insert && plus && !(dragging && isInside(insert.list, dragging)) && (
        <EdgeLabelRenderer>
          <div
            className="nodrag nopan pointer-events-auto absolute"
            style={{ transform: `translate(-50%, -50%) translate(${plus.x}px, ${plus.y}px)` }}
            onDragOver={(e) => {
              if (!e.dataTransfer.types.includes(DRAG_TYPE)) return;
              e.preventDefault();
              e.dataTransfer.dropEffect = "copy";
              setOver(true);
            }}
            onDragLeave={() => setOver(false)}
            onDrop={(e) => {
              const type = e.dataTransfer.getData(DRAG_TYPE) as StepType;
              setOver(false);
              if (!type) return;
              e.preventDefault();
              e.stopPropagation();
              onInsert(insert.list, insert.index, type);
            }}
          >
            <Popover open={open} onOpenChange={setOpen}>
              <PopoverTrigger asChild>
                <button
                  type="button"
                  disabled={busy}
                  aria-label="Add a step here"
                  data-insert={`${insert.list}@${insert.index}`}
                  title="Add a step here"
                  className={cn(
                    "flex size-6 items-center justify-center rounded-full border border-line-strong bg-background text-muted-foreground shadow-sm transition-all hover:scale-110 hover:border-foreground hover:text-foreground",
                    (open || over || dropTarget === `${insert.list}@${insert.index}`) && "scale-150 border-foreground bg-foreground text-background",
                  )}
                >
                  <Plus className="size-3.5" />
                </button>
              </PopoverTrigger>
              <PopoverContent className="w-52 p-1" side="right" align="center">
                <TypeMenu
                  onPick={(type) => {
                    setOpen(false);
                    onInsert(insert.list, insert.index, type);
                  }}
                />
              </PopoverContent>
            </Popover>
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
});
const edgeTypes = { insert: InsertEdge };

export interface FlowCanvasProps {
  definition: Definition;
  selection: Selection;
  pendingPath: string | null;
  runs: Map<string, StepRun> | null;
  busy: boolean;
  onSelect: (selection: Selection) => void;
  onInsert: (list: string, index: number, type: StepType) => void;
  /** A step dragged onto a +: move it there. */
  onMove: (stepPath: string, list: string, index: number) => void;
  className?: string;
}

/** The + nearest to a point, within `reach`, among those `allowed`. */
function nearestInsert(edges: LaidEdge[], x: number, y: number, reach: number, allowed: (e: LaidEdge) => boolean = () => true) {
  let best: LaidEdge | null = null;
  let distance = reach;
  for (const e of edges) {
    if (!e.insert || !e.plus || !allowed(e)) continue;
    const d = Math.hypot(e.plus.x - x, e.plus.y - y);
    if (d < distance) {
      distance = d;
      best = e;
    }
  }
  return best;
}

/** The smallest zoom the canvas fits to, so card text stays readable; wider flows pan. */
export const READABLE_ZOOM = 0.75;

function Canvas({ definition, selection, pendingPath, runs, busy, onSelect, onInsert, onMove, className }: FlowCanvasProps) {
  const laid = useMemo(() => layout(definition), [definition]);
  const [drag, setDrag] = useState<{ id: string; path: string; x: number; y: number } | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const nodes = useMemo<Node<Data>[]>(
    () =>
      laid.nodes.map((n) => ({
        id: n.id,
        type: n.kind,
        position: drag && drag.id === n.id ? { x: drag.x, y: drag.y } : { x: n.x, y: n.y },
        width: n.width,
        height: n.height,
        data: { laid: n },
        draggable: n.kind === "step" && !busy && n.path !== pendingPath,
        connectable: false,
        selectable: false,
        zIndex: n.kind === "loop" ? -1 : drag?.id === n.id ? 10 : 1,
        style: { width: n.width, height: n.height },
      })),
    [laid, drag, busy, pendingPath],
  );
  const edges = useMemo<Edge<EdgeData>[]>(
    () => laid.edges.map((e) => ({ id: e.id, source: e.source, target: e.target, type: "insert", data: { laid: e }, selectable: false, focusable: false })),
    [laid],
  );
  const flow = useReactFlow();
  const paneWidth = useStore((s) => s.width);
  const paneHeight = useStore((s) => s.height);

  /** Fits the flow when it fits at a readable size; otherwise centres the main column at the top. */
  const fit = useCallback(
    (duration = 0) => {
      if (!paneWidth || !paneHeight) return;
      const xs = laid.nodes.map((n) => n.x);
      const left = Math.min(...xs);
      const right = Math.max(...laid.nodes.map((n) => n.x + n.width));
      const width = right - left;
      const zoom = Math.min(1, Math.max(READABLE_ZOOM, Math.min(paneWidth / (width + 96), paneHeight / (laid.height + 96))));
      const fitsWide = width * zoom + 48 <= paneWidth;
      const fitsTall = laid.height * zoom + 96 <= paneHeight;
      const x = fitsWide ? (paneWidth - width * zoom) / 2 - left * zoom : paneWidth / 2;
      const y = fitsTall ? (paneHeight - laid.height * zoom) / 2 : 40;
      flow.setViewport({ x, y, zoom }, { duration });
    },
    [paneWidth, paneHeight, laid, flow],
  );
  // Fit when the canvas first has a size and when its width changes, not when steps
  // change: the main column stays at x = 0, so the view stays put as the flow grows.
  const fitted = useRef(0);
  useEffect(() => {
    if (!paneWidth || Math.abs(fitted.current - paneWidth) < 40) return;
    fitted.current = paneWidth;
    fit();
  }, [paneWidth, fit]);

  const moveTarget = useCallback(
    (path: string, node: { x: number; y: number; width: number }) =>
      nearestInsert(laid.edges, node.x + node.width / 2, node.y, 140, (e) => !isInside(e.insert!.list, path) && !staysPut(path, e.insert!.list, e.insert!.index)),
    [laid.edges],
  );
  const paletteTarget = useCallback(
    (clientX: number, clientY: number) => {
      const p = flow.screenToFlowPosition({ x: clientX, y: clientY });
      return nearestInsert(laid.edges, p.x, p.y, 220);
    },
    [flow, laid.edges],
  );

  const value = useMemo(
    () => ({ selection, pendingPath, runs, inputs: definition.inputs, onInsert, busy, dropTarget, dragging: drag?.path ?? null }),
    [selection, pendingPath, runs, definition.inputs, onInsert, busy, dropTarget, drag?.path],
  );
  const key = (e: LaidEdge | null) => (e?.insert ? `${e.insert.list}@${e.insert.index}` : null);
  return (
    <Ctx.Provider value={value}>
      <div
        className={cn("app-flow relative", className)}
        onDragOver={(e) => {
          if (!e.dataTransfer.types.includes(DRAG_TYPE)) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
          setDropTarget(key(paletteTarget(e.clientX, e.clientY)));
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as HTMLElement | null)) setDropTarget(null);
        }}
        onDrop={(e) => {
          const type = e.dataTransfer.getData(DRAG_TYPE) as StepType;
          setDropTarget(null);
          if (!type) return;
          e.preventDefault();
          // Dropped near a +: there. Anywhere else on the canvas: at the end.
          const target = paletteTarget(e.clientX, e.clientY)?.insert ?? { list: "steps", index: definition.steps.length };
          onInsert(target.list, target.index, type);
        }}
      >
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          nodesConnectable={false}
          elementsSelectable={false}
          nodeDragThreshold={4}
          panOnScroll
          zoomOnDoubleClick={false}
          minZoom={0.25}
          maxZoom={1.5}
          proOptions={{ hideAttribution: true }}
          onNodeDragStart={(_, node) => {
            const n = (node.data as Data).laid;
            if (n.path) setDrag({ id: node.id, path: n.path, x: node.position.x, y: node.position.y });
          }}
          onNodeDrag={(_, node) => {
            const n = (node.data as Data).laid;
            if (!n.path) return;
            setDrag({ id: node.id, path: n.path, x: node.position.x, y: node.position.y });
            setDropTarget(key(moveTarget(n.path, { x: node.position.x, y: node.position.y, width: n.width })));
          }}
          onNodeDragStop={(_, node) => {
            const n = (node.data as Data).laid;
            const target = n.path ? moveTarget(n.path, { x: node.position.x, y: node.position.y, width: n.width }) : null;
            setDrag(null);
            setDropTarget(null);
            if (n.path && target?.insert) onMove(n.path, target.insert.list, target.insert.index);
          }}
          onNodeClick={(_, node) => {
            const n = (node.data as Data).laid;
            if (n.kind === "inputs") onSelect({ kind: "inputs" });
            else if ((n.kind === "step" || n.kind === "loop") && n.path) onSelect({ kind: "step", path: n.path });
          }}
          onPaneClick={() => onSelect(null)}
          aria-label="The app’s steps"
        >
          <Background variant={BackgroundVariant.Dots} gap={18} size={1} color="var(--line-strong)" />
          <Controls showInteractive={false} showFitView={false} position="bottom-left">
            <ControlButton onClick={() => fit(250)} title="Fit to view" aria-label="Fit to view">
              <Maximize />
            </ControlButton>
          </Controls>
        </ReactFlow>
      </div>
    </Ctx.Provider>
  );
}

export function FlowCanvas(props: FlowCanvasProps) {
  return (
    <ReactFlowProvider>
      <Canvas {...props} />
    </ReactFlowProvider>
  );
}
