"use client";

// Batches: run one prompt over every row of a spreadsheet, in the background, and
// get the spreadsheet back with the answers added. Endpoints in api/src/batches.rs.

import { useQueryClient } from "@tanstack/react-query";
import { Download, FileSpreadsheet, Layers, MoreHorizontal, Pencil, Plus, Search, Sparkles, Tag, Trash2, Upload, X } from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { Chips, Field, FormActions, FormDialog, FormError, Note, Pill, Section, SelectField, TextField, useConfirm } from "@/components/manage/kit";
import { parseTags } from "@/components/library/api";
import { messageOf } from "@/components/projects/form-field";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useModels } from "@/lib/queries";
import { cn } from "@/lib/utils";
import {
  BATCHES_KEY,
  cancelBatch,
  deletable,
  deleteBatch,
  doneOf,
  duration,
  GROUPS,
  groupCounts,
  isActive,
  matches,
  percentOf,
  refreshBatches,
  resultsHref,
  startBatch,
  stateOf,
  tagCounts,
  updateBatch,
  useBatches,
  WINDOWS,
  type Batch,
  type BatchList,
  type Group,
  type Tone,
} from "./api";
import { parseCsv, renderPrompt, toSheet, unknownPlaceholders, usedColumns, type Sheet } from "./csv";
import { EXAMPLE_CSV, EXAMPLE_FILE_NAME, EXAMPLE_NAME, EXAMPLE_SYSTEM, EXAMPLE_TEMPLATE, exampleHref } from "./example";

const MAX_ROWS = 50_000;
const MAX_FILE_BYTES = 90 * 1024 * 1024;

function ago(iso: string | null, now: number): string {
  if (!iso) return "";
  const s = (now - Date.parse(iso)) / 1000;
  return Number.isFinite(s) ? `${duration(s)} ago` : "";
}

export function BatchesPage() {
  usePageChrome("Batches");
  const list = useBatches();
  const [creating, setCreating] = useState(false);
  const data = list.data;
  const batches = data?.batches ?? [];

  return (
    <Page>
      <PageHeader
        title="Batches"
        description="Run one prompt over every row of a spreadsheet. Batches run in the background while models have room to spare, so they can take minutes or hours. You can close this page."
        actions={
          !creating && data?.available !== false ? (
            <Button onClick={() => setCreating(true)}>
              <Plus /> New batch
            </Button>
          ) : null
        }
      />
      <ErrorBox error={list.error} onRetry={() => list.refetch()} />
      {data?.available === false && (
        <EmptyState icon={Layers} title="Batches aren't available here" text="The gateway this Chat uses doesn't run batches. Ask the people who run it to turn them on." />
      )}
      {creating && <NewBatchPanel onClose={() => setCreating(false)} />}
      {list.isLoading ? (
        <Loading label="Loading your batches…" />
      ) : data?.available && batches.length === 0 && !creating ? (
        <EmptyState
          icon={FileSpreadsheet}
          title="No batches yet"
          text="Upload a spreadsheet, write a prompt that uses its columns, and pick a model. Each row becomes one request, and the answers come back as a new column. There's a short example to try first."
          action={
            <Button onClick={() => setCreating(true)}>
              <Plus /> New batch
            </Button>
          }
        />
      ) : batches.length > 0 ? (
        <BatchList batches={batches} more={!!data?.more} now={list.dataUpdatedAt || Date.now()} />
      ) : null}
    </Page>
  );
}

/** Puts changed batches into the cached list, and drops deleted ones. */
function patchList(client: ReturnType<typeof useQueryClient>, changed: Batch[], removed: string[] = []) {
  const next = new Map(changed.map((b) => [b.id, b]));
  client.setQueryData<BatchList>(BATCHES_KEY, (old) => (old ? { ...old, batches: old.batches.filter((b) => !removed.includes(b.id)).map((b) => next.get(b.id) ?? b) } : old));
}

/** Runs `work` over every item, four at a time, and counts what failed. */
async function eachOf<T>(items: T[], work: (item: T) => Promise<unknown>): Promise<number> {
  let failed = 0;
  for (let i = 0; i < items.length; i += 4) {
    const done = await Promise.allSettled(items.slice(i, i + 4).map(work));
    failed += done.filter((r) => r.status === "rejected").length;
  }
  return failed;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

function BatchList({ batches, more, now }: { batches: Batch[]; more: boolean; now: number }) {
  const client = useQueryClient();
  const confirm = useConfirm();
  const [group, setGroup] = useState<Group>("all");
  const [tag, setTag] = useState("");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState<Batch | null>(null);
  const [tagging, setTagging] = useState(false);

  const counts = groupCounts(batches);
  const tags = tagCounts(batches);
  const shown = batches.filter((b) => matches(b, { group, tag, query }));
  const picked = shown.filter((b) => selected.has(b.id));
  const running = batches.filter(isActive).length;
  const filtered = group !== "all" || !!tag || !!query.trim();
  const allPicked = shown.length > 0 && picked.length === shown.length;

  const toggle = (id: string, on: boolean) =>
    setSelected((old) => {
      const next = new Set(old);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  const clearFilters = () => {
    setGroup("all");
    setTag("");
    setQuery("");
  };

  const remove = (targets: Batch[]) => {
    const stopped = targets.filter(deletable);
    const skipped = targets.length - stopped.length;
    if (!stopped.length) {
      toast("Running batches can't be deleted. Cancel them first.");
      return;
    }
    const one = stopped.length === 1 ? stopped[0] : null;
    confirm.ask({
      title: one ? `Delete “${one.name}”?` : `Delete ${plural(stopped.length, "batch", "batches")}?`,
      text: `${one ? "Its answers and the spreadsheet you uploaded are" : "Their answers and the spreadsheets you uploaded are"} removed for good. Download the results first if you still need them.${skipped ? ` ${plural(skipped, "running batch is", "running batches are")} left alone; cancel ${skipped === 1 ? "it" : "them"} first.` : ""}`,
      label: one ? "Delete batch" : `Delete ${plural(stopped.length, "batch", "batches")}`,
      cancel: "Keep",
      action: async () => {
        const gone: string[] = [];
        const failed = await eachOf(stopped, async (b) => {
          await deleteBatch(b.id);
          gone.push(b.id);
        });
        patchList(client, [], gone);
        setSelected((old) => new Set([...old].filter((id) => !gone.includes(id))));
        refreshBatches(client);
        if (failed) toast(`${plural(gone.length, "batch", "batches")} deleted; ${plural(failed, "batch", "batches")} couldn't be.`);
        else toast(gone.length === 1 ? "Batch deleted" : `${plural(gone.length, "batch", "batches")} deleted`);
      },
    });
  };

  return (
    <div className="grid gap-3">
      {confirm.element}
      <div className="grid gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <label className="flex min-w-[220px] flex-1 items-center gap-2 rounded-lg border bg-card px-3 py-1 focus-within:ring-2 focus-within:ring-ring/40">
            <Search className="size-4 shrink-0 text-muted-foreground" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search by name or #tag"
              maxLength={200}
              aria-label="Search batches"
              className="h-8 border-0 bg-transparent px-0 shadow-none focus-visible:ring-0 dark:bg-transparent"
            />
            {query && (
              <Button variant="ghost" size="xs" onClick={() => setQuery("")}>
                Clear
              </Button>
            )}
          </label>
          <Chips label="Show" value={group} onChange={setGroup} options={GROUPS.filter(([g]) => g === "all" || counts[g] > 0).map(([g, name]) => [g, `${name} ${counts[g]}`] as const)} />
        </div>
        {tags.length > 0 && (
          <div className="flex flex-wrap items-center gap-1.5">
            <Tag className="size-3.5 text-faint" aria-hidden />
            {tags.slice(0, 24).map((t) => (
              <button
                key={t.tag}
                type="button"
                aria-pressed={tag === t.tag}
                onClick={() => setTag(tag === t.tag ? "" : t.tag)}
                className={cn("inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground", tag === t.tag && "bg-secondary text-foreground")}
              >
                #{t.tag}
                <span className="text-faint">{t.count}</span>
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="flex min-h-9 flex-wrap items-center justify-between gap-3 rounded-md px-1">
        <label className="flex items-center gap-2 text-sm text-muted-foreground">
          <Checkbox
            checked={allPicked ? true : picked.length ? "indeterminate" : false}
            onCheckedChange={(on) => setSelected(on === true ? new Set([...selected, ...shown.map((b) => b.id)]) : new Set([...selected].filter((id) => !shown.some((b) => b.id === id))))}
            aria-label="Select every batch shown"
            disabled={!shown.length}
          />
          {picked.length ? `${plural(picked.length, "batch", "batches")} selected` : filtered ? `${plural(shown.length, "batch", "batches")} of ${batches.length.toLocaleString()}` : running ? `${plural(running, "batch is", "batches are")} running. This page updates on its own.` : plural(batches.length, "batch", "batches")}
        </label>
        {picked.length > 0 && (
          <span className="flex flex-wrap items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => setTagging(true)}>
              <Tag /> Add tags
            </Button>
            <Button variant="outline" size="sm" onClick={() => remove(picked)}>
              <Trash2 /> Delete
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setSelected(new Set())}>
              Clear
            </Button>
          </span>
        )}
      </div>

      {shown.length ? (
        shown.map((b) => <BatchRow key={b.id} batch={b} now={now} selected={selected.has(b.id)} onSelect={(on) => toggle(b.id, on)} onEdit={() => setEditing(b)} onDelete={() => remove([b])} onTag={setTag} />)
      ) : (
        <p className="flex flex-wrap items-center gap-2 py-6 text-sm text-muted-foreground">
          Nothing matches.
          <Button variant="outline" size="xs" onClick={clearFilters}>
            Clear the filters
          </Button>
        </p>
      )}
      {more && <p className="py-2 text-sm text-muted-foreground">Showing your newest {batches.length.toLocaleString()} batches. Delete old ones to see further back.</p>}
      {editing && <EditBatchDialog batch={editing} known={tags.map((t) => t.tag)} onClose={() => setEditing(null)} />}
      {tagging && <AddTagsDialog batches={picked} known={tags.map((t) => t.tag)} onClose={() => setTagging(false)} />}
    </div>
  );
}

/** Tags already in use, offered as one-click additions to a tags field. */
function TagSuggestions({ known, used, onPick }: { known: string[]; used: string[]; onPick: (tag: string) => void }) {
  const left = known.filter((t) => !used.includes(t)).slice(0, 12);
  if (!left.length) return null;
  return (
    <div className="flex flex-wrap gap-1.5">
      {left.map((t) => (
        <button key={t} type="button" onClick={() => onPick(t)} className="rounded-md border px-2 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground">
          + #{t}
        </button>
      ))}
    </div>
  );
}

const tagText = (tags: string[]) => tags.map((t) => `#${t}`).join(" ");

function EditBatchDialog({ batch, known, onClose }: { batch: Batch; known: string[]; onClose: () => void }) {
  const client = useQueryClient();
  const [name, setName] = useState(batch.name);
  const [tags, setTags] = useState(tagText(batch.tags));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const used = parseTags(tags);
  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const next = await updateBatch(batch.id, { name: name.trim(), tags: used });
      patchList(client, [next]);
      onClose();
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title="Rename and tag" description="Only you see these. Tags help you find batches later.">
      <form onSubmit={save} className="grid gap-4">
        <TextField id="batch-edit-name" label="Name" value={name} onChange={(e) => setName(e.target.value)} maxLength={200} help="Leave it empty to go back to the name it started with." />
        <Field id="batch-edit-tags" label="Tags" help="Separate tags with spaces, up to 12.">
          <Input id="batch-edit-tags" value={tags} onChange={(e) => setTags(e.target.value)} maxLength={400} placeholder="#survey #round-2" autoFocus />
        </Field>
        <TagSuggestions known={known} used={used} onPick={(t) => setTags((v) => `${v.trim()} #${t}`.trim())} />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            {busy ? "Saving…" : "Save"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function AddTagsDialog({ batches, known, onClose }: { batches: Batch[]; known: string[]; onClose: () => void }) {
  const client = useQueryClient();
  const [tags, setTags] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const adding = parseTags(tags);
  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!adding.length) return setError("Type at least one tag.");
    setBusy(true);
    setError("");
    const changed: Batch[] = [];
    const failed = await eachOf(batches, async (b) => {
      changed.push(await updateBatch(b.id, { tags: [...new Set([...b.tags, ...adding])].slice(0, 12) }));
    });
    patchList(client, changed);
    setBusy(false);
    if (failed) return setError(`${plural(failed, "batch", "batches")} couldn't be tagged. Try again.`);
    toast(`Tagged ${plural(changed.length, "batch", "batches")}`);
    onClose();
  };
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title={`Add tags to ${plural(batches.length, "batch", "batches")}`} description="Tags they already have stay. A batch holds up to 12.">
      <form onSubmit={save} className="grid gap-4">
        <Field id="batch-add-tags" label="Tags" help="Separate tags with spaces.">
          <Input id="batch-add-tags" value={tags} onChange={(e) => setTags(e.target.value)} maxLength={400} placeholder="#survey #round-2" autoFocus />
        </Field>
        <TagSuggestions known={known} used={adding} onPick={(t) => setTags((v) => `${v.trim()} #${t}`.trim())} />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            {busy ? "Tagging…" : "Add tags"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

/** The bar: rows that worked solid, rows that failed striped, the rest empty. */
function Progress({ b, tone }: { b: Batch; tone: Tone }) {
  const ok = b.total ? (b.completed / b.total) * 100 : 0;
  const bad = b.total ? (b.failed / b.total) * 100 : 0;
  return (
    <span role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percentOf(b)} aria-label={`${percentOf(b)}% done`} className="flex h-2 w-full overflow-hidden rounded-full bg-secondary">
      <span className={cn("h-full transition-[width] duration-700", tone === "waiting" ? "bg-amber" : tone === "stopped" ? "bg-muted-foreground" : "bg-chart-1")} style={{ width: `${ok}%` }} />
      <span className="h-full bg-[repeating-linear-gradient(135deg,var(--destructive)_0_3px,transparent_3px_6px)]" style={{ width: `${bad}%` }} />
    </span>
  );
}

function BatchRow({
  batch: b,
  now,
  selected,
  onSelect,
  onEdit,
  onDelete,
  onTag,
}: {
  batch: Batch;
  now: number;
  selected: boolean;
  onSelect: (on: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
  onTag: (tag: string) => void;
}) {
  const client = useQueryClient();
  const confirm = useConfirm();
  const s = stateOf(b, now);
  const done = doneOf(b);
  const stoppable = b.status === "in_progress" || b.status === "validating";
  const tone = s.tone === "problem" ? "danger" : s.tone === "waiting" ? "amber" : s.tone === "done" ? "accent" : "";

  const stop = () =>
    confirm.ask({
      title: `Cancel “${b.name}”?`,
      text: `The ${(b.total - done).toLocaleString()} rows that haven't run yet won't run. Rows that finished stay in the results.`,
      label: "Cancel batch",
      cancel: "Keep running",
      action: async () => {
        const next = await cancelBatch(b.id);
        client.setQueryData<BatchList>(BATCHES_KEY, (old) => (old ? { ...old, batches: old.batches.map((x) => (x.id === b.id ? next : x)) } : old));
        refreshBatches(client);
      },
    });

  return (
    <section className={cn("grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-3 rounded-lg border bg-card p-4 sm:grid-cols-[auto_minmax(0,1.2fr)_minmax(0,1.4fr)_auto] sm:items-center sm:gap-x-5", selected && "border-line-strong bg-accent/30")}>
      {confirm.element}
      <Checkbox checked={selected} onCheckedChange={(on) => onSelect(on === true)} aria-label={`Select “${b.name}”`} className="mt-1 sm:mt-0" />
      <div className="min-w-0">
        <h2 className="truncate text-base font-semibold" title={b.name}>
          {b.name}
        </h2>
        <p className="mt-0.5 truncate text-sm text-muted-foreground">
          {[b.model, b.rows != null ? `${b.rows.toLocaleString()} rows` : `${b.total.toLocaleString()} requests`, ago(b.createdAt, now)].filter(Boolean).join(" · ")}
        </p>
        {b.tags.length > 0 && (
          <div className="mt-1 flex flex-wrap gap-x-2 gap-y-0.5 text-xs">
            {b.tags.map((t) => (
              <button key={t} type="button" onClick={() => onTag(t)} className="text-faint hover:text-foreground" title={`Show only #${t}`}>
                #{t}
              </button>
            ))}
          </div>
        )}
      </div>
      <div className="col-span-2 grid min-w-0 gap-1.5 sm:col-span-1">
        <div className="flex items-center justify-between gap-3">
          <span className="flex min-w-0 items-center gap-2">
            <Pill tone={tone}>{s.label}</Pill>
            <span className="truncate text-sm text-muted-foreground">
              {done.toLocaleString()} of {b.total.toLocaleString()}
              {b.failed ? ` · ${b.failed.toLocaleString()} failed` : ""}
            </span>
          </span>
          <span className="text-sm font-medium tabular-nums">{percentOf(b)}%</span>
        </div>
        <Progress b={b} tone={s.tone} />
        {s.detail && <p className="text-xs text-muted-foreground">{s.detail}</p>}
      </div>
      <div className="col-span-2 flex flex-wrap items-center gap-2 sm:col-span-1 sm:justify-end">
        {done > 0 && (
          <Button asChild variant={isActive(b) ? "outline" : "default"} size="sm">
            <a href={resultsHref(b.id)} download>
              <Download /> {isActive(b) ? "Results so far" : "Download results"}
            </a>
          </Button>
        )}
        {stoppable && (
          <Button variant="ghost" size="sm" onClick={stop}>
            Cancel
          </Button>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label={`More for “${b.name}”`}>
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={onEdit}>
              <Pencil /> Rename and tag
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" disabled={!deletable(b)} onSelect={onDelete}>
              <Trash2 /> {deletable(b) ? "Delete" : "Delete (cancel it first)"}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </section>
  );
}

function NewBatchPanel({ onClose }: { onClose: () => void }) {
  const client = useQueryClient();
  const models = useModels();
  const file = useRef<HTMLInputElement>(null);
  const prompt = useRef<HTMLTextAreaElement>(null);
  const [cells, setCells] = useState<string[][] | null>(null);
  const [fileName, setFileName] = useState("");
  const [headers, setHeaders] = useState(true);
  const [template, setTemplate] = useState("");
  const [system, setSystem] = useState("");
  const [showSystem, setShowSystem] = useState(false);
  const [model, setModel] = useState("");
  const [window, setWindow] = useState("24h");
  const [name, setName] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);

  const choices = (models.data ?? []).filter((m) => m.id !== "auto" && m.available !== false);
  const chosen = model || choices[0]?.id || "";
  const sheet: Sheet | null = useMemo(() => (cells ? toSheet(cells, headers) : null), [cells, headers]);
  const unknown = sheet ? unknownPlaceholders(template, sheet.columns) : [];
  const used = sheet ? usedColumns(template, sheet.columns) : [];
  const sendable = sheet ? sheet.rows.filter((r) => r.some((c) => c.trim())) : [];
  const examples = sheet && template.trim() ? sendable.slice(0, 2).map((r) => renderPrompt(template, sheet.columns, r)) : [];

  async function read(f: File | undefined) {
    setError("");
    if (!f) return;
    if (/\.(xlsx|xls|numbers|ods)$/i.test(f.name)) {
      setError("Save the spreadsheet as CSV first (File › Save As › CSV), then upload that.");
      return;
    }
    if (f.size > MAX_FILE_BYTES) {
      setError("That file is larger than 90 MB. Split it into smaller batches.");
      return;
    }
    const parsed = parseCsv(await f.text());
    if (parsed.length < 2) {
      setError("That file has no rows to send. It should be CSV, with a row of column names first.");
      return;
    }
    if (parsed.length - 1 > MAX_ROWS) {
      setError(`A batch can hold up to ${MAX_ROWS.toLocaleString()} rows; this file has ${(parsed.length - 1).toLocaleString()}. Split it into smaller files.`);
      return;
    }
    setCells(parsed);
    setFileName(f.name);
    if (!name) setName(f.name.replace(/\.[^.]+$/, ""));
  }

  function loadExample() {
    setError("");
    setCells(parseCsv(EXAMPLE_CSV));
    setFileName(EXAMPLE_FILE_NAME);
    setHeaders(true);
    setTemplate(EXAMPLE_TEMPLATE);
    setSystem(EXAMPLE_SYSTEM);
    setShowSystem(true);
    setName(EXAMPLE_NAME);
  }

  function insert(column: string) {
    const token = `{${column}}`;
    const el = prompt.current;
    if (!el) return setTemplate((t) => t + token);
    const start = el.selectionStart ?? template.length;
    const end = el.selectionEnd ?? template.length;
    const next = template.slice(0, start) + token + template.slice(end);
    setTemplate(next);
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(start + token.length, start + token.length);
    });
  }

  async function start() {
    if (!sheet) return;
    setError("");
    if (!chosen) return setError("Choose a model.");
    if (!template.trim()) return setError("Write the prompt each row is sent with.");
    setBusy(true);
    try {
      const batch = await startBatch({ name: name.trim() || fileName, model: chosen, system: showSystem ? system : "", template, columns: sheet.columns, rows: sheet.rows, window, fileName });
      client.setQueryData<BatchList>(BATCHES_KEY, (old) => ({ available: true, batches: [batch, ...(old?.batches ?? []).filter((b) => b.id !== batch.id)] }));
      refreshBatches(client);
      toast(`Started “${batch.name}”. It runs in the background.`);
      onClose();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Section
      className="mb-6"
      title="New batch"
      description="Each row of the spreadsheet becomes one request. The answers come back as a new column."
      actions={
        <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
          <X />
        </Button>
      }
    >
      {!sheet ? (
        <label
          onDragOver={(e) => {
            e.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragging(false);
            void read(e.dataTransfer.files[0]);
          }}
          className={cn("flex cursor-pointer flex-col items-center gap-2 rounded-lg border border-dashed px-6 py-10 text-center transition-colors hover:bg-accent/40", dragging && "bg-accent/60")}
        >
          <Upload className="size-6 text-muted-foreground" />
          <span className="font-medium">Choose a spreadsheet, or drop it here</span>
          <span className="text-sm text-muted-foreground">CSV or TSV with a row of column names first. Up to {MAX_ROWS.toLocaleString()} rows.</span>
          <input ref={file} type="file" accept=".csv,.tsv,.txt,text/csv,text/tab-separated-values" className="sr-only" onChange={(e) => void read(e.target.files?.[0])} />
        </label>
      ) : null}
      {!sheet && (
        <div className="mt-3 flex flex-wrap items-center justify-between gap-3 rounded-md border bg-background px-3 py-2.5">
          <span className="text-sm text-muted-foreground">New to batches? Try a 10-row example with the prompt filled in.</span>
          <span className="flex items-center gap-2">
            <Button variant="ghost" size="sm" asChild>
              <a href={exampleHref()} download={EXAMPLE_FILE_NAME}>
                <Download /> Download it
              </a>
            </Button>
            <Button variant="outline" size="sm" onClick={loadExample}>
              <Sparkles /> Try the example
            </Button>
          </span>
        </div>
      )}
      {sheet && (
        <div className="grid gap-5">
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border bg-background px-3 py-2">
            <span className="flex min-w-0 items-center gap-2 text-sm">
              <FileSpreadsheet className="size-4 shrink-0 text-muted-foreground" />
              <span className="truncate font-medium">{fileName}</span>
              <span className="text-muted-foreground">
                {sheet.rows.length.toLocaleString()} rows · {sheet.columns.length} columns
              </span>
            </span>
            <span className="flex items-center gap-3">
              <label className="flex items-center gap-2 text-sm">
                <Switch checked={headers} onCheckedChange={setHeaders} aria-label="First row is column names" />
                First row is column names
              </label>
              <Button variant="ghost" size="sm" onClick={() => setCells(null)}>
                Change file
              </Button>
            </span>
          </div>

          <div className="overflow-x-auto rounded-md border">
            <table className="w-full text-left text-xs">
              <thead className="bg-secondary/50">
                <tr>
                  {sheet.columns.map((c) => (
                    <th key={c} className="max-w-[220px] truncate px-2.5 py-1.5 font-medium">
                      {c}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {sheet.rows.slice(0, 3).map((r, i) => (
                  <tr key={i} className="border-t">
                    {r.map((c, j) => (
                      <td key={j} className="max-w-[220px] truncate px-2.5 py-1.5 text-muted-foreground" title={c}>
                        {c}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <Field id="batch-prompt" label="Prompt for each row" help="Click a column to put its value into the prompt. Each row gets the prompt with its own values filled in.">
            <div className="flex flex-wrap gap-1.5">
              {sheet.columns.map((c) => (
                <button
                  key={c}
                  type="button"
                  onClick={() => insert(c)}
                  className={cn("rounded-full border px-2.5 py-0.5 font-mono text-xs transition-colors hover:bg-accent", used.includes(c) && "border-chart-1/50 bg-chart-1/10")}
                >
                  {`{${c}}`}
                </button>
              ))}
            </div>
            <Textarea id="batch-prompt" ref={prompt} rows={5} value={template} onChange={(e) => setTemplate(e.target.value)} placeholder={`Summarize this abstract in two sentences:\n\n{${sheet.columns[0]}}`} />
          </Field>
          {template.trim() && used.length === 0 && <Note className="text-amber">The prompt uses no column, so every row would get the same request.</Note>}
          {unknown.length > 0 && <Note className="text-amber">No column is named {unknown.map((u) => `{${u}}`).join(", ")}. That text is sent as it is.</Note>}

          {showSystem ? (
            <Field id="batch-system" label="Instructions for every row" help="Sent before each prompt, like a system prompt: the role, the format to answer in, what to leave out.">
              <Textarea id="batch-system" rows={3} value={system} onChange={(e) => setSystem(e.target.value)} placeholder="Answer with one word: positive, negative or neutral." />
            </Field>
          ) : (
            <div>
              <Button variant="ghost" size="sm" onClick={() => setShowSystem(true)}>
                <Plus /> Add instructions for every row
              </Button>
            </div>
          )}

          <div className="grid gap-4 sm:grid-cols-3">
            <SelectField id="batch-model" label="Model" value={chosen} onChange={(e) => setModel(e.target.value)} options={choices.map((m) => [m.id, m.name ?? m.id] as const)} />
            <SelectField id="batch-window" label="Finish" value={window} onChange={(e) => setWindow(e.target.value)} options={WINDOWS} help="Rows still waiting then don't run." />
            <TextField id="batch-name" label="Name" value={name} onChange={(e) => setName(e.target.value)} maxLength={200} />
          </div>

          {examples.length > 0 && (
            <div className="grid gap-2">
              <span className="text-sm font-medium">What the first {examples.length === 1 ? "row sends" : "rows send"}</span>
              {examples.map((ex, i) => (
                <pre key={i} className="max-h-40 overflow-auto rounded-md border bg-background p-3 font-mono text-xs whitespace-pre-wrap text-muted-foreground">
                  {ex}
                </pre>
              ))}
            </div>
          )}

          <FormError message={error} />
          <div className="flex flex-wrap items-center justify-between gap-3">
            <Note>Batches wait whenever people are chatting with the model, so a busy model takes longer. You can download the results so far at any time.</Note>
            <Button onClick={start} disabled={busy || !template.trim() || !chosen || sendable.length === 0}>
              {busy ? "Starting…" : `Start batch · ${sendable.length.toLocaleString()} ${sendable.length === 1 ? "request" : "requests"}`}
            </Button>
          </div>
        </div>
      )}
      {!sheet && <FormError message={error} />}
    </Section>
  );
}
