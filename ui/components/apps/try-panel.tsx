"use client";

// Try the draft: a field per input, Run, then the answer and each step's trace.
// The canvas shows the same trace on its cards.

import { CircleAlert, CircleCheck, Play, X } from "lucide-react";
import { useState } from "react";
import { FormError, FormField, messageOf } from "@/components/projects/form-field";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import type { TraceEntry } from "./layout";
import { type Field, pretty } from "./model";
import { readTryInputs } from "./step-draft";
import type { TryResult } from "./api";

export function Json({ value, className }: { value: unknown; className?: string }) {
  return <pre className={cn("max-h-80 overflow-auto rounded-md bg-code p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words", className)}>{pretty(value)}</pre>;
}

export function StatusBadge({ status }: { status: string }) {
  const ok = status === "completed";
  return (
    <Badge variant="outline" className={cn(ok ? "border-success/50 text-success" : status === "running" ? "text-muted-foreground" : "border-destructive/50 text-destructive-foreground")}>
      {ok ? "Completed" : status === "running" ? "Running" : "Failed"}
    </Badge>
  );
}

/** The steps a run went through: time, the path a branch took, and on opening its output or error. */
export function Trace({ trace, open = false }: { trace: TraceEntry[]; open?: boolean }) {
  if (!trace.length) return null;
  return (
    <ol className="grid divide-y rounded-md border">
      {trace.map((t, i) => (
        <li key={i}>
          <details open={open && !t.ok} className="group">
            <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-sm hover:bg-accent [&::-webkit-details-marker]:hidden">
              {t.ok ? <CircleCheck className="size-3.5 shrink-0 text-success" /> : <CircleAlert className="size-3.5 shrink-0 text-destructive-foreground" />}
              <span className="min-w-0 grow truncate">
                {t.title || t.id}
                {t.path && <span className="text-muted-foreground"> → {t.path}</span>}
              </span>
              <span className="shrink-0 text-xs text-faint">{t.ok ? `${t.ms} ms` : "failed"}</span>
            </summary>
            <div className="px-3 pb-3">
              <Json value={t.ok ? t.output : t.error} />
            </div>
          </details>
        </li>
      ))}
    </ol>
  );
}

export function TryPanel({ inputs, result, onRun, onClear }: { inputs: Field[]; result: TryResult | null; onRun: (inputs: Record<string, unknown>) => Promise<void>; onClear: () => void }) {
  const [values, setValues] = useState<Record<string, string | boolean>>({});
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const set = (name: string, value: string | boolean) => setValues((v) => ({ ...v, [name]: value }));
  return (
    <div className="grid gap-4">
      <form
        id="app-test-form"
        className="grid gap-3"
        onSubmit={async (e) => {
          e.preventDefault();
          setError("");
          setBusy(true);
          try {
            await onRun(readTryInputs(inputs, values));
          } catch (err) {
            setError(messageOf(err));
          } finally {
            setBusy(false);
          }
        }}
      >
        <div>
          <h3 className="text-base font-semibold">Try the draft</h3>
          <p className="text-xs text-muted-foreground">Runs the steps as they are now, published or not. Each card shows how its step went.</p>
        </div>
        {inputs.map((f) => {
          const id = `app-try-${f.name}`;
          const label = `${f.name}${f.required ? "" : " (optional)"}`;
          if (f.type === "boolean")
            return (
              <label key={f.name} htmlFor={id} className="flex items-center gap-2.5 text-md">
                <Checkbox id={id} checked={!!values[f.name]} onCheckedChange={(v) => set(f.name, !!v)} />
                <strong className="font-medium">{f.name}</strong>
              </label>
            );
          if (f.type === "json")
            return (
              <FormField key={f.name} id={id} label={label}>
                <Textarea id={id} rows={3} className="font-mono text-sm" placeholder='["a", "b"]' value={String(values[f.name] ?? "")} onChange={(e) => set(f.name, e.target.value)} />
              </FormField>
            );
          return (
            <FormField key={f.name} id={id} label={label}>
              <Input id={id} type={f.type === "number" ? "number" : "text"} step={f.type === "number" ? "any" : undefined} value={String(values[f.name] ?? "")} onChange={(e) => set(f.name, e.target.value)} />
            </FormField>
          );
        })}
        <FormError message={error} />
        <div className="flex items-center gap-2">
          <Button type="submit" size="sm" disabled={busy}>
            <Play /> {busy ? "Running…" : "Run"}
          </Button>
          {result && (
            <Button type="button" size="sm" variant="ghost" onClick={onClear}>
              <X /> Clear
            </Button>
          )}
        </div>
      </form>
      {result && (
        <div className="grid gap-3" aria-live="polite">
          <div className="flex items-center justify-between">
            <StatusBadge status={result.status} />
            <span className="text-xs text-faint">{result.ms} ms</span>
          </div>
          {result.error ? <p className="text-sm text-destructive-foreground">{result.error}</p> : <Json value={result.output} />}
          <Trace trace={result.trace ?? []} open />
        </div>
      )}
    </div>
  );
}
