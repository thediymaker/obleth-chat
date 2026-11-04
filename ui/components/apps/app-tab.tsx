"use client";

// A project's App tab: make the app, lay out its steps on the
// builder, try the draft, publish it, and hand out keys for its API.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Box, Braces, Copy, ExternalLink, KeyRound, MessageSquare, MousePointerClick, Plus, RefreshCw, Settings2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { ErrorBox, Eyebrow, Loading } from "@/components/app-shell/page";
import { Confirm, FormError, FormField, messageOf } from "@/components/projects/form-field";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { when } from "@/lib/format";
import { keys, useModels } from "@/lib/queries";
import { cn } from "@/lib/utils";
import {
  type App,
  appKeys,
  type AppRun,
  chatPagePath,
  createApp,
  createKey,
  deleteApp,
  getProjectApp,
  getRun,
  listKeys,
  listRuns,
  publishApp,
  revokeKey,
  saveApp,
  tryApp,
  type TryResult,
  unpublishApp,
} from "./api";
import { DRAG_TYPE, FlowCanvas, type Selection } from "./flow-canvas";
import { traceByStep } from "./layout";
import { blank, type Definition, freshId, insertStep, isInside, listAt, moveStep, moveStepTo, pathOf, removeStep, replaceStep, splitPath, staysPut, type Step, stepAt, type StepType, TYPE_HELP, TYPES } from "./model";
import { curlExample } from "./step-draft";
import { StepIcon } from "./step-icon";
import { InputsEditor, StepEditor } from "./step-editor";
import { Json, StatusBadge, Trace, TryPanel } from "./try-panel";

export function AppTab({ projectId }: { projectId: string }) {
  const client = useQueryClient();
  const q = useQuery({ queryKey: appKeys.project(projectId), queryFn: () => getProjectApp(projectId) });
  const [creating, setCreating] = useState(false);
  if (q.isPending) return <Loading label="Loading the app…" />;
  if (q.error && q.data === undefined) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />;
  if (!q.data)
    return (
      <section className="mx-auto grid max-w-2xl justify-items-start gap-3 rounded-xl border bg-card p-6">
        <Eyebrow>App</Eyebrow>
        <h2 className="text-xl font-semibold">Turn this project into an app</h2>
        <p className="text-sm text-muted-foreground">
          An app takes inputs, runs steps you lay out (models, this project’s knowledge, code, rules) and answers with JSON. Publish it, and programs call it with a key. Only
          administrators can make apps for now.
        </p>
        <Button
          disabled={creating}
          onClick={async () => {
            setCreating(true);
            try {
              const app = await createApp(projectId);
              client.setQueryData(appKeys.project(projectId), app);
              client.invalidateQueries({ queryKey: keys.project(projectId) });
              toast("App created");
            } catch (e) {
              toast(messageOf(e));
            } finally {
              setCreating(false);
            }
          }}
        >
          <Plus /> Create app
        </Button>
      </section>
    );
  return <AppEditor app={q.data} projectId={projectId} />;
}

interface Pending {
  list: string;
  index: number;
  step: Step;
}

function AppEditor({ app, projectId }: { app: App; projectId: string }) {
  const client = useQueryClient();
  const { data: models = [] } = useModels();
  const runsQ = useQuery({ queryKey: appKeys.runs(app.id), queryFn: () => listRuns(app.id) });
  const keysQ = useQuery({ queryKey: appKeys.keys(app.id), queryFn: () => listKeys(app.id) });
  const [selection, setSelection] = useState<Selection>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [result, setResult] = useState<TryResult | null>(null);
  const [panel, setPanel] = useState<"edit" | "try">("edit");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [dialog, setDialog] = useState<"" | "json" | "details">("");

  const setApp = useCallback((next: App | null) => client.setQueryData(appKeys.project(projectId), next), [client, projectId]);
  // The newest app, for saves started from a toast after later changes.
  const latest = useRef(app);
  latest.current = app;
  async function save(definition: Definition, details?: { name: string; description: string }) {
    setBusy(true);
    setError("");
    try {
      setApp(await saveApp(latest.current, definition, details));
      return true;
    } catch (e) {
      setError(messageOf(e));
      return false;
    } finally {
      setBusy(false);
    }
  }
  /** Puts the steps back as they were before a change, from its toast. */
  function undoTo(before: Definition, message: string) {
    return {
      action: {
        label: "Undo",
        onClick: async () => {
          if (await save(before)) {
            setSelection(null);
            setPending(null);
            toast(message);
          } else toast("Couldn’t undo: the steps changed since");
        },
      },
    };
  }

  const shown = useMemo(() => (pending ? insertStep(app.definition, pending.list, pending.index, pending.step) : app.definition), [app.definition, pending]);
  const pendingPath = pending ? `${pending.list}.${pending.index}` : null;
  const runs = useMemo(() => (result ? traceByStep(result.trace) : null), [result]);
  const firstModel = models.find((m) => m.id !== "auto")?.id || "";

  /**
   * A new step at a place in the tree: saved at once and opened in the panel. When
   * the server won't take it as it is (an Agent with no model to pick), it waits
   * there as a draft with the reason, to finish and add from the panel.
   */
  const insert = useCallback(
    async (list: string, index: number, type: StepType) => {
      const definition = latest.current.definition;
      const step = blank(type, freshId(definition, type), firstModel);
      const path = `${list}.${index}`;
      setPending(null);
      setPanel("edit");
      setBusy(true);
      setError("");
      try {
        setApp(await saveApp(latest.current, insertStep(definition, list, index, step)));
        setSelection({ kind: "step", path });
      } catch (e) {
        setPending({ list, index, step });
        setSelection({ kind: "step", path });
        setError(messageOf(e));
      } finally {
        setBusy(false);
      }
    },
    [firstModel, setApp],
  );
  /** From the palette: after the selected step, else at the end. */
  function addFromPalette(type: StepType) {
    if (selection?.kind === "step" && !pending) {
      const [list, index] = splitPath(selection.path);
      insert(list, index + 1, type);
    } else insert("steps", app.definition.steps.length, type);
  }
  const select = useCallback(
    (next: Selection) => {
      if (pending && !(next?.kind === "step" && next.path === pendingPath)) setPending(null);
      setSelection(next);
      setError("");
      if (next) setPanel("edit");
    },
    [pending, pendingPath],
  );
  /** A step dragged onto a +. */
  const moveTo = useCallback(
    async (stepPath: string, list: string, index: number) => {
      const before = latest.current.definition;
      const step = stepAt(before, stepPath);
      if (!step || isInside(list, stepPath) || staysPut(stepPath, list, index)) return;
      setPending(null);
      const after = moveStepTo(before, stepPath, list, index);
      if (await save(after)) {
        const path = pathOf(after, step.id);
        setSelection(path ? { kind: "step", path } : null);
        toast(`Moved ${step.title || step.id}`, undoTo(before, "Moved it back"));
      } else toast(`Couldn’t move ${step.title || step.id}`);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps -- save and undoTo read refs
    [],
  );

  const selected = selection?.kind === "step" ? stepAt(shown, selection.path) : undefined;
  const [selList, selIndex] = selection?.kind === "step" ? splitPath(selection.path) : ["", 0];
  const selLength = selection?.kind === "step" ? (listAt(shown, selList)?.length ?? 0) : 0;
  // A selection the steps no longer have (after an undo or an edit elsewhere) is dropped.
  useEffect(() => {
    if (selection?.kind === "step" && !selected) setSelection(null);
  }, [selection, selected]);

  async function saveStep(step: Step) {
    if (pending) {
      if (await save(insertStep(app.definition, pending.list, pending.index, step))) setPending(null);
    } else if (selection?.kind === "step") await save(replaceStep(app.definition, selection.path, step));
  }
  async function move(by: number) {
    if (selection?.kind !== "step") return;
    if (await save(moveStep(app.definition, selection.path, by))) setSelection({ kind: "step", path: `${selList}.${selIndex + by}` });
  }
  async function remove() {
    if (selection?.kind !== "step") return;
    const before = app.definition;
    const [d, gone] = removeStep(before, selection.path);
    if (await save(d)) {
      setSelection(null);
      toast(`Deleted ${gone.title || gone.id}`, undoTo(before, `Put ${gone.title || gone.id} back`));
    }
  }
  async function run(inputs: Record<string, unknown>) {
    setResult(await tryApp(app, inputs));
    client.invalidateQueries({ queryKey: appKeys.runs(app.id) });
  }

  const status = !app.live ? (
    <Badge variant="outline">Draft</Badge>
  ) : app.changed ? (
    <Badge variant="outline" className="border-amber/50 text-amber">
      v{app.version} live · draft changed
    </Badge>
  ) : (
    <Badge variant="outline" className="border-success/50 text-success">
      v{app.version} live
    </Badge>
  );

  async function act(work: () => Promise<App>, message: (a: App) => string) {
    setBusy(true);
    try {
      const next = await work();
      setApp(next);
      toast(message(next));
    } catch (e) {
      toast(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="grid grid-cols-1 gap-4">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 grow basis-72">
          <h2 className="flex flex-wrap items-center gap-2 text-lg font-semibold">
            {app.name}
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs font-normal text-muted-foreground">{app.id}</code>
            {status}
          </h2>
          <p className="mt-0.5 text-sm text-muted-foreground">{app.description || "No description yet."}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="outline" size="sm" onClick={() => setDialog("details")}>
            <Settings2 /> Details
          </Button>
          {app.live && (
            <Button variant="outline" size="sm" disabled={busy} onClick={() => act(() => unpublishApp(app), () => "Unpublished: calls now get 404")}>
              Unpublish
            </Button>
          )}
          <Button size="sm" disabled={busy || (app.live && !app.changed)} onClick={() => act(() => publishApp(app), (a) => `Published v${a.version}`)}>
            {app.live ? "Publish changes" : "Publish"}
          </Button>
        </div>
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)] overflow-hidden rounded-xl border bg-card lg:h-[min(76dvh,820px)] lg:grid-cols-[64px_minmax(0,1fr)_360px] 2xl:grid-cols-[212px_minmax(0,1fr)_360px]">
        <Palette onAdd={addFromPalette} disabled={busy} />
        <div className="relative h-[480px] border-y bg-background lg:h-full lg:border-x lg:border-y-0">
          <FlowCanvas definition={shown} selection={selection} pendingPath={pendingPath} runs={runs} busy={busy} onSelect={select} onInsert={insert} onMove={moveTo} className="h-full w-full" />
          <div className="pointer-events-none absolute top-3 right-3 flex gap-2">
            <Button variant="outline" size="xs" className="pointer-events-auto bg-card" onClick={() => setDialog("json")}>
              <Braces /> Edit everything as JSON
            </Button>
          </div>
          {runs && (
            <div className="pointer-events-none absolute top-3 left-3">
              <span className="pointer-events-auto inline-flex items-center gap-2 rounded-full border bg-card px-2.5 py-1 text-xs shadow-sm">
                Showing the last try
                <button type="button" className="text-muted-foreground hover:text-foreground" onClick={() => setResult(null)}>
                  Clear
                </button>
              </span>
            </div>
          )}
        </div>
        <aside className="min-h-0 lg:overflow-y-auto" aria-label="Properties">
          <Tabs value={panel} onValueChange={(v) => setPanel(v as "edit" | "try")} className="gap-0">
            <div className="sticky top-0 z-10 border-b bg-card px-4 py-2.5">
              <TabsList className="w-full">
                <TabsTrigger value="edit">Edit</TabsTrigger>
                <TabsTrigger value="try">Try</TabsTrigger>
              </TabsList>
            </div>
            <TabsContent value="edit" className="px-4 pt-4">
              {selection?.kind === "inputs" ? (
                <InputsEditor inputs={app.definition.inputs} busy={busy} error={error} onSave={(inputs) => save({ ...app.definition, inputs })} />
              ) : selected ? (
                <StepEditor
                  key={`${selection?.kind === "step" ? selection.path : ""}:${selected.id}:${pending ? "new" : ""}`}
                  step={selected}
                  pending={!!pending}
                  canMoveUp={selIndex > 0}
                  canMoveDown={selIndex < selLength - 1}
                  busy={busy}
                  error={error}
                  onSave={saveStep}
                  onCancel={() => select(null)}
                  onMove={move}
                  onDelete={remove}
                />
              ) : (
                <div className="grid justify-items-center gap-2 px-2 py-10 text-center">
                  <MousePointerClick className="size-5 text-muted-foreground" />
                  <p className="text-md font-medium">Select a step to edit it</p>
                  <p className="text-sm text-muted-foreground">Or the Inputs card for what a call sends. Use a + on a connector, or the palette, to add a step.</p>
                </div>
              )}
            </TabsContent>
            <TabsContent value="try" className="px-4 py-4">
              <TryPanel inputs={app.definition.inputs} result={result} onRun={run} onClear={() => setResult(null)} />
            </TabsContent>
          </Tabs>
        </aside>
      </div>
      <p className="text-sm text-muted-foreground">
        Steps read the run with references: <code className="font-mono text-text-2">{"{{inputs.question}}"}</code>, <code className="font-mono text-text-2">{"{{steps.search}}"}</code>,{" "}
        <code className="font-mono text-text-2">{"{{item}}"}</code> inside For each.
      </p>

      <div className="grid grid-cols-1 items-start gap-4 lg:grid-cols-2">
        <div className="grid gap-4">
          <ChatPageCard app={app} />
          <ApiCard app={app} keys={keysQ.data ?? []} onChanged={() => client.invalidateQueries({ queryKey: appKeys.keys(app.id) })} />
        </div>
        <RunsCard app={app} runs={runsQ.data ?? []} onRefresh={() => runsQ.refetch()} />
      </div>

      <JsonDialog open={dialog === "json"} onOpenChange={(o) => setDialog(o ? "json" : "")} app={app} onSaved={setApp} />
      <DetailsDialog
        open={dialog === "details"}
        onOpenChange={(o) => setDialog(o ? "details" : "")}
        app={app}
        onSaved={setApp}
        onDeleted={() => {
          setApp(null);
          client.invalidateQueries({ queryKey: keys.project(projectId) });
        }}
      />
    </div>
  );
}

function Palette({ onAdd, disabled }: { onAdd: (type: StepType) => void; disabled: boolean }) {
  return (
    <div className="flex min-h-0 flex-col">
      <div className="px-3 pt-3 pb-2 lg:px-0 lg:text-center 2xl:px-3 2xl:text-left">
        <Eyebrow>Steps</Eyebrow>
      </div>
      {/* A row on phones, an icon rail beside the canvas, and names with help on wide screens. */}
      <div className="flex gap-1.5 overflow-x-auto px-3 pb-3 lg:grid lg:justify-items-center lg:overflow-y-auto lg:px-2 2xl:justify-items-stretch 2xl:px-3">
        {TYPES.map(([type, name]) => (
          <button
            key={type}
            type="button"
            draggable
            disabled={disabled}
            onDragStart={(e) => {
              e.dataTransfer.setData(DRAG_TYPE, type);
              e.dataTransfer.effectAllowed = "copy";
            }}
            onClick={() => onAdd(type)}
            title={`Add ${name} (${TYPE_HELP[type].toLowerCase()}): after the selected step, or at the end. Or drag it onto a +.`}
            aria-label={`Add ${name}`}
            className="flex shrink-0 cursor-grab items-center gap-2.5 rounded-lg border border-transparent px-2 py-1.5 text-left hover:border-border hover:bg-accent active:cursor-grabbing disabled:opacity-50 lg:size-11 lg:justify-center lg:p-0 2xl:size-auto 2xl:justify-start 2xl:px-2 2xl:py-1.5"
          >
            <StepIcon type={type} size="sm" />
            <span className="min-w-0 lg:sr-only 2xl:not-sr-only">
              <span className="block text-md whitespace-nowrap 2xl:whitespace-normal">{name}</span>
              <span className="hidden text-2xs leading-snug text-muted-foreground 2xl:block">{TYPE_HELP[type]}</span>
            </span>
          </button>
        ))}
      </div>
      <p className="mt-auto hidden px-3 pb-3 text-2xs text-faint 2xl:block">Click to add after the selected step, or drag onto a + between steps.</p>
    </div>
  );
}

function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  return (
    <Button
      variant="outline"
      size="xs"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          toast("Copied");
        } catch {
          toast("Couldn’t copy. Select the text and copy it instead.");
        }
      }}
    >
      <Copy /> {label}
    </Button>
  );
}

function runLabel(r: AppRun, long = false) {
  return r.trigger === "test" ? "Test" : `${r.trigger === "chat" ? (long ? "Chat message" : "Chat") : long ? "API call" : "API"} · v${r.version}`;
}

/** What keeps the steps from working well on a chat page, if anything. Mirrors chat_fields in api/src/apps/mod.rs. */
export function chatPageHints(inputs: Definition["inputs"]): { blocked: string; tip: string } {
  const isHistory = (f: Definition["inputs"][number]) => f.name === "history" && f.type === "json";
  const message = inputs.find((f) => (f.type ?? "text") === "text" && f.name !== "history");
  const missing = inputs.filter((f) => f.required && f !== message && !isHistory(f)).map((f) => f.name);
  if (!message) return { blocked: "Add a text input: a chat message goes into the first one.", tip: "" };
  if (missing.length) return { blocked: `A chat fills only ${message.name}. Make ${missing.join(", ")} optional, or the page can’t send.`, tip: "" };
  return {
    blocked: "",
    tip: inputs.some(isHistory)
      ? `Messages go into ${message.name}; earlier turns into history.`
      : `Messages go into ${message.name}. Add a JSON input named history to give the app the earlier turns.`,
  };
}

function ChatPageCard({ app }: { app: App }) {
  const path = chatPagePath(app);
  const url = (typeof window === "undefined" ? "" : window.location.origin) + path;
  const { blocked, tip } = chatPageHints(app.definition.inputs);
  return (
    <section className="grid gap-3 rounded-lg border bg-card p-4">
      <div className="flex items-center justify-between gap-2">
        <Eyebrow>Chat page</Eyebrow>
        <div className="flex gap-1.5">
          <CopyButton text={url} label="Copy link" />
          {app.live && (
            <Button asChild variant="outline" size="xs">
              <a href={path} target="_blank" rel="noopener">
                <ExternalLink /> Open
              </a>
            </Button>
          )}
        </div>
      </div>
      <p className="text-sm text-muted-foreground">
        {app.live ? "People chat with the published version. Each message runs as the person chatting, with their access. Administrators only for now." : "Publish the app to turn its chat page on."}
      </p>
      <div className="flex items-center gap-2 overflow-x-auto rounded-md bg-code p-3 font-mono text-xs">
        <MessageSquare className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="whitespace-nowrap">{url}</span>
      </div>
      {(blocked || tip) && <p className={cn("text-sm", blocked ? "text-destructive-foreground" : "text-muted-foreground")}>{blocked || tip}</p>}
    </section>
  );
}

function ApiCard({ app, keys: list, onChanged }: { app: App; keys: { id: string; name: string; createdBy: string | null; lastUsedAt: string | null }[]; onChanged: () => void }) {
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [error, setError] = useState("");
  const [secret, setSecret] = useState("");
  const [revoking, setRevoking] = useState<{ id: string; name: string } | null>(null);
  const curl = curlExample(typeof window === "undefined" ? "" : window.location.origin, app.endpoint, app.definition.inputs);
  return (
    <section className="grid gap-3 rounded-lg border bg-card p-4">
      <div className="flex items-center justify-between">
        <Eyebrow>API</Eyebrow>
        <CopyButton text={curl} />
      </div>
      <p className="text-sm text-muted-foreground">{app.live ? "Programs call the published version with a key. A call runs as the person who made its key." : "Publish the app to turn its address on."}</p>
      <pre className="overflow-x-auto rounded-md bg-code p-3 font-mono text-xs leading-relaxed">{curl}</pre>
      {list.length ? (
        <div className="grid divide-y rounded-md border">
          {list.map((k) => (
            <div key={k.id} className="flex items-center gap-3 px-3 py-2">
              <KeyRound className="size-4 shrink-0 text-muted-foreground" />
              <span className="min-w-0 grow">
                <strong className="block truncate text-md font-medium">{k.name}</strong>
                <small className="text-xs text-muted-foreground">
                  {k.createdBy || "Unknown"} · {k.lastUsedAt ? `last used ${when(k.lastUsedAt)}` : "never used"}
                </small>
              </span>
              <Button size="xs" variant="outline" className="text-destructive-foreground" onClick={() => setRevoking(k)}>
                Revoke
              </Button>
            </div>
          ))}
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">No keys yet.</p>
      )}
      <Button
        variant="outline"
        size="sm"
        className="justify-self-start"
        onClick={() => {
          setName("");
          setError("");
          setCreating(true);
        }}
      >
        <Plus /> Create key
      </Button>

      <Dialog open={creating} onOpenChange={setCreating}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Create a key</DialogTitle>
            <DialogDescription className="sr-only">Name the key so you know where it is used.</DialogDescription>
          </DialogHeader>
          <form
            id="app-key-form"
            className="grid gap-4"
            onSubmit={async (e) => {
              e.preventDefault();
              setError("");
              try {
                const r = await createKey(app.id, name.trim());
                setCreating(false);
                setSecret(r.secret);
                onChanged();
              } catch (err) {
                setError(messageOf(err));
              }
            }}
          >
            <FormField id="app-key-name" label="Name" help="Calls with this key run as you, on your allowance.">
              <Input id="app-key-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} required placeholder="Lab pipeline" autoFocus />
            </FormField>
            <FormError message={error} />
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => setCreating(false)}>
                Cancel
              </Button>
              <Button type="submit">Create key</Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog open={!!secret} onOpenChange={(o) => !o && setSecret("")}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Your new key</DialogTitle>
            <DialogDescription>Copy it now: Chat keeps only a hash and can’t show it again.</DialogDescription>
          </DialogHeader>
          <Input id="app-key-secret" readOnly value={secret} className="font-mono text-sm" onFocus={(e) => e.currentTarget.select()} />
          <DialogFooter>
            <CopyButton text={secret} />
            <Button onClick={() => setSecret("")}>Done</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && setRevoking(null)}
        title={`Revoke ${revoking?.name ?? "this key"}?`}
        text="Programs using it stop working at once."
        confirm="Revoke"
        onConfirm={async () => {
          await revokeKey(app.id, revoking!.id);
          onChanged();
          toast("Key revoked");
        }}
      />
    </section>
  );
}

const runBadge = (r: AppRun) => (
  <Badge
    variant="outline"
    className={cn("w-16 justify-center", r.status === "completed" ? "border-success/50 text-success" : r.status === "running" ? "text-muted-foreground" : "border-destructive/50 text-destructive-foreground")}
  >
    {r.status === "completed" ? "OK" : r.status === "running" ? "Running" : "Failed"}
  </Badge>
);

function RunsCard({ app, runs, onRefresh }: { app: App; runs: AppRun[]; onRefresh: () => void }) {
  const [open, setOpen] = useState<AppRun | null>(null);
  const [detail, setDetail] = useState<AppRun | null>(null);
  async function show(r: AppRun) {
    setOpen(r);
    setDetail(null);
    try {
      setDetail(await getRun(app.id, r.id));
    } catch (e) {
      setOpen(null);
      toast(messageOf(e));
    }
  }
  return (
    <section className="grid gap-3 rounded-lg border bg-card p-4">
      <div className="flex items-center justify-between">
        <Eyebrow>Recent runs</Eyebrow>
        <Button variant="ghost" size="xs" onClick={onRefresh}>
          <RefreshCw /> Refresh
        </Button>
      </div>
      {runs.length ? (
        <>
          <div className="-mx-2 grid">
            {runs.slice(0, 10).map((r) => (
              <button key={r.id} type="button" onClick={() => show(r)} className="flex items-center gap-3 rounded-md px-2 py-1.5 text-left text-md hover:bg-accent">
                {runBadge(r)}
                <span className="grow">{runLabel(r)}</span>
                <span className="text-xs text-faint">
                  {r.ms == null ? "" : `${r.ms} ms · `}
                  {when(r.startedAt)}
                </span>
              </button>
            ))}
          </div>
          <p className="text-xs text-muted-foreground">Runs are kept for 7 days.</p>
        </>
      ) : (
        <p className="text-sm text-muted-foreground">No runs yet. Try the draft, or call the published app.</p>
      )}
      <Dialog open={!!open} onOpenChange={(o) => !o && setOpen(null)}>
        <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {open && `${runLabel(open, true)} · ${when(open.startedAt)}`}
            </DialogTitle>
            <DialogDescription className="flex items-center gap-2">
              {open && <StatusBadge status={open.status} />}
              {open?.ms != null && <span>{open.ms} ms</span>}
            </DialogDescription>
          </DialogHeader>
          {!detail ? (
            <Loading />
          ) : (
            <div className="grid gap-3">
              <Eyebrow>Inputs</Eyebrow>
              <Json value={detail.inputs} />
              <Eyebrow>{detail.error ? "Error" : "Output"}</Eyebrow>
              {detail.error ? <p className="text-sm text-destructive-foreground">{detail.error}</p> : <Json value={detail.output} />}
              <Eyebrow>Steps</Eyebrow>
              <Trace trace={detail.trace ?? []} />
            </div>
          )}
        </DialogContent>
      </Dialog>
    </section>
  );
}

function JsonDialog({ open, onOpenChange, app, onSaved }: { open: boolean; onOpenChange: (o: boolean) => void; app: App; onSaved: (a: App) => void }) {
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!open) return;
    setText(JSON.stringify(app.definition, null, 2));
    setError("");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- fresh each time it opens
  }, [open]);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Everything as JSON</DialogTitle>
          <DialogDescription>The app’s inputs and steps. Saving replaces them all.</DialogDescription>
        </DialogHeader>
        <form
          id="app-json-form"
          className="grid gap-4"
          onSubmit={async (e) => {
            e.preventDefault();
            setError("");
            let d: Definition;
            try {
              d = JSON.parse(text);
            } catch {
              setError("That isn’t valid JSON");
              return;
            }
            setBusy(true);
            try {
              onSaved(await saveApp(app, d));
              onOpenChange(false);
            } catch (err) {
              setError(messageOf(err));
            } finally {
              setBusy(false);
            }
          }}
        >
          <FormField id="app-json-text" label="Inputs and steps">
            <Textarea id="app-json-text" rows={22} spellCheck={false} className="font-mono text-xs" value={text} onChange={(e) => setText(e.target.value)} />
          </FormField>
          <FormError message={error} />
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={busy}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function DetailsDialog({ open, onOpenChange, app, onSaved, onDeleted }: { open: boolean; onOpenChange: (o: boolean) => void; app: App; onSaved: (a: App) => void; onDeleted: () => void }) {
  const [name, setName] = useState(app.name);
  const [description, setDescription] = useState(app.description);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState(false);
  useEffect(() => {
    if (!open) return;
    setName(app.name);
    setDescription(app.description);
    setError("");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- fresh each time it opens
  }, [open]);
  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>App details</DialogTitle>
            <DialogDescription className="sr-only">Rename the app or describe it.</DialogDescription>
          </DialogHeader>
          <form
            id="app-details-form"
            className="grid gap-4"
            onSubmit={async (e) => {
              e.preventDefault();
              setBusy(true);
              setError("");
              try {
                onSaved(await saveApp(app, app.definition, { name: name.trim(), description: description.trim() }));
                onOpenChange(false);
              } catch (err) {
                setError(messageOf(err));
              } finally {
                setBusy(false);
              }
            }}
          >
            <FormField id="app-name" label="Name">
              <Input id="app-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={120} required />
            </FormField>
            <FormField id="app-description" label="Description">
              <Input id="app-description" value={description} onChange={(e) => setDescription(e.target.value)} maxLength={1000} />
            </FormField>
            <p className="text-xs text-muted-foreground">
              Its code, <code className="font-mono">{app.id}</code>, never changes, so a rename never breaks a caller.
            </p>
            <FormError message={error} />
            <DialogFooter className="sm:justify-between">
              <Button type="button" variant="outline" className="text-destructive-foreground" onClick={() => setDeleting(true)}>
                <Box /> Delete app
              </Button>
              <div className="flex gap-2">
                <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                  Cancel
                </Button>
                <Button type="submit" disabled={busy}>
                  Save
                </Button>
              </div>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Confirm
        open={deleting}
        onOpenChange={setDeleting}
        title="Delete this app?"
        text="Its keys stop working and its runs are deleted. The project stays."
        confirm="Delete app"
        onConfirm={async () => {
          await deleteApp(app);
          onOpenChange(false);
          onDeleted();
          toast("App deleted");
        }}
      />
    </>
  );
}
