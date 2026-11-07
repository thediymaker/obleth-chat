"use client";

// Batches: run one prompt over every row of a spreadsheet, in the background, and
// get the spreadsheet back with the answers added. Endpoints in api/src/batches.rs.

import { useQueryClient } from "@tanstack/react-query";
import { Download, FileSpreadsheet, Layers, Plus, Upload, X } from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { Field, FormError, Note, Pill, Section, SelectField, TextField, useConfirm } from "@/components/manage/kit";
import { messageOf } from "@/components/projects/form-field";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useModels } from "@/lib/queries";
import { cn } from "@/lib/utils";
import {
  BATCHES_KEY,
  cancelBatch,
  doneOf,
  duration,
  isActive,
  percentOf,
  refreshBatches,
  resultsHref,
  startBatch,
  stateOf,
  useBatches,
  WINDOWS,
  type Batch,
  type BatchList,
  type Tone,
} from "./api";
import { parseCsv, renderPrompt, toSheet, unknownPlaceholders, usedColumns, type Sheet } from "./csv";

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
  const running = batches.filter(isActive);

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
          text="Upload a spreadsheet, write a prompt that uses its columns, and pick a model. Each row becomes one request, and the answers come back as a new column."
          action={
            <Button onClick={() => setCreating(true)}>
              <Plus /> New batch
            </Button>
          }
        />
      ) : batches.length > 0 ? (
        <div className="grid gap-3">
          {running.length > 0 && <Note>{running.length === 1 ? "1 batch is running." : `${running.length} batches are running.`} This page updates on its own.</Note>}
          {batches.map((b) => (
            <BatchRow key={b.id} batch={b} now={list.dataUpdatedAt || Date.now()} />
          ))}
        </div>
      ) : null}
    </Page>
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

function BatchRow({ batch: b, now }: { batch: Batch; now: number }) {
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
    <section className="grid gap-3 rounded-lg border bg-card p-4 sm:grid-cols-[minmax(0,1.2fr)_minmax(0,1.4fr)_auto] sm:items-center sm:gap-5">
      {confirm.element}
      <div className="min-w-0">
        <h2 className="truncate text-base font-semibold" title={b.name}>
          {b.name}
        </h2>
        <p className="mt-0.5 truncate text-sm text-muted-foreground">
          {[b.model, b.rows != null ? `${b.rows.toLocaleString()} rows` : `${b.total.toLocaleString()} requests`, ago(b.createdAt, now)].filter(Boolean).join(" · ")}
        </p>
      </div>
      <div className="grid min-w-0 gap-1.5">
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
      <div className="flex flex-wrap gap-2 sm:justify-end">
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
      ) : (
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
