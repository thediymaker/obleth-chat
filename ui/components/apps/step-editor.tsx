"use client";

// The properties panel: the selected step's fields (the old step editor's), and the
// app's inputs.

import { ArrowDown, ArrowUp, Plus, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { FormError, FormField } from "@/components/projects/form-field";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useModels } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { type Field, KINDS, MAX_RULES, METHODS, type Step, TESTS, TYPE, COMPARES, type Test } from "./model";
import { blankRule, fromDraft, fromInputDrafts, type IfPathDraft, type InputDraft, newIfPath, type StepDraft, toDraft, toInputDrafts } from "./step-draft";
import { StepIcon } from "./step-icon";

const ID_PATTERN = "[a-z][a-z0-9_]*";

function Check({ id, checked, onChange, label, help }: { id: string; checked: boolean; onChange: (v: boolean) => void; label: string; help?: string }) {
  return (
    <label htmlFor={id} className="flex items-start gap-2.5">
      <Checkbox id={id} checked={checked} onCheckedChange={(v) => onChange(!!v)} className="mt-0.5" />
      <span className="grid gap-0.5">
        <strong className="text-md font-medium">{label}</strong>
        {help && <small className="text-xs text-muted-foreground">{help}</small>}
      </span>
    </label>
  );
}
const Mono = (props: React.ComponentProps<typeof Textarea>) => <Textarea {...props} spellCheck={false} className={cn("font-mono text-sm", props.className)} />;

function ModelField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const { data: models = [] } = useModels();
  const options = [...new Set([value, ...models.filter((m) => m.id !== "auto").map((m) => m.id)].filter(Boolean))];
  return (
    <FormField id="app-step-model" label="Model" help={options.length ? undefined : "No models are available right now."}>
      <Select value={value || undefined} onValueChange={onChange}>
        <SelectTrigger id="app-step-model" className="w-full">
          <SelectValue placeholder="Choose a model" />
        </SelectTrigger>
        <SelectContent>
          {options.map((m) => (
            <SelectItem key={m} value={m}>
              {m}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </FormField>
  );
}

function IfPaths({ paths, onChange }: { paths: IfPathDraft[]; onChange: (paths: IfPathDraft[]) => void }) {
  const set = (i: number, path: IfPathDraft) => onChange(paths.map((p, j) => (j === i ? path : p)));
  return (
    <div className="grid gap-3">
      {paths.map((p, i) => (
        <fieldset key={i} className="grid gap-2 rounded-md border p-3">
          <legend className="px-1 text-xs font-medium text-text-2">
            {i ? "Else if" : "If"}, path {i + 1}
          </legend>
          {p.rules.map((r, j) => (
            <div key={j} className="grid gap-1.5 rounded-md bg-muted/60 p-2">
              <div className="flex items-center gap-1.5">
                <Input aria-label={`Value, rule ${j + 1}`} value={r.value} placeholder="{{inputs.question}}" onChange={(e) => set(i, { ...p, rules: p.rules.map((x, k) => (k === j ? { ...x, value: e.target.value } : x)) })} />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`Remove rule ${j + 1}`}
                  title="Remove rule"
                  onClick={() => set(i, { ...p, rules: p.rules.filter((_, k) => k !== j) })}
                >
                  <X />
                </Button>
              </div>
              <div className="grid grid-cols-2 gap-1.5">
                <Select value={r.test} onValueChange={(v) => set(i, { ...p, rules: p.rules.map((x, k) => (k === j ? { ...x, test: v as Test } : x)) })}>
                  <SelectTrigger aria-label={`Test, rule ${j + 1}`} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {TESTS.map(([id, label]) => (
                      <SelectItem key={id} value={id}>
                        {label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Input
                  aria-label={`Compare with, rule ${j + 1}`}
                  placeholder="compare with"
                  value={r.to}
                  disabled={!COMPARES.has(r.test)}
                  onChange={(e) => set(i, { ...p, rules: p.rules.map((x, k) => (k === j ? { ...x, to: e.target.value } : x)) })}
                />
              </div>
            </div>
          ))}
          {p.rules.length < MAX_RULES && (
            <Button type="button" variant="ghost" size="xs" className="justify-self-start" onClick={() => set(i, { ...p, rules: [...p.rules, blankRule()] })}>
              <Plus /> {p.rules.length ? "And another rule" : "Add a rule"}
            </Button>
          )}
          <FormField id={`app-rule-${i}-code`} label="Or one line of JavaScript">
            <Input id={`app-rule-${i}-code`} className="font-mono text-sm" value={p.code} placeholder="steps.rows.length > 20" onChange={(e) => set(i, { ...p, code: e.target.value })} />
          </FormField>
          {i > 0 && (
            <Button type="button" variant="ghost" size="xs" className="justify-self-start text-destructive-foreground" onClick={() => onChange(paths.filter((_, j) => j !== i))}>
              <Trash2 /> Remove this path (its steps go too)
            </Button>
          )}
        </fieldset>
      ))}
      <Button type="button" variant="outline" size="sm" className="justify-self-start" onClick={() => onChange([...paths, newIfPath()])}>
        <Plus /> Add another path
      </Button>
    </div>
  );
}

function TypeFields({ d, set }: { d: StepDraft & { type: string }; set: (patch: Partial<StepDraft>) => void }) {
  switch (d.type) {
    case "agent":
      return (
        <>
          <ModelField value={d.model} onChange={(model) => set({ model })} />
          <FormField id="app-step-instructions" label="Instructions">
            <Textarea id="app-step-instructions" rows={3} value={d.instructions} onChange={(e) => set({ instructions: e.target.value })} />
          </FormField>
          <FormField id="app-step-prompt" label="Prompt" help="What the model is sent. References are filled in.">
            <Textarea id="app-step-prompt" rows={5} value={d.prompt} onChange={(e) => set({ prompt: e.target.value })} />
          </FormField>
          <Check id="app-step-json" checked={d.json} onChange={(json) => set({ json })} label="Read the reply as JSON" />
        </>
      );
    case "search":
      return (
        <>
          <FormField id="app-step-query" label="Search for">
            <Input id="app-step-query" value={d.query} onChange={(e) => set({ query: e.target.value })} />
          </FormField>
          <FormField id="app-step-limit" label="Passages">
            <Input id="app-step-limit" type="number" min={1} max={20} value={d.limit} onChange={(e) => set({ limit: e.target.value })} />
          </FormField>
          <p className="text-xs text-muted-foreground">Searches the project’s knowledge collections.</p>
        </>
      );
    case "code":
      return (
        <FormField id="app-step-code" label="JavaScript" help="The body of a function: it sees inputs, steps, item and index, and returns what the next steps read. No network or files; at most 5 seconds and 64 MB.">
          <Mono id="app-step-code" rows={12} value={d.code} onChange={(e) => set({ code: e.target.value })} />
        </FormField>
      );
    case "decide":
      return (
        <>
          <ModelField value={d.model} onChange={(model) => set({ model })} />
          <FormField id="app-step-question" label="Question">
            <Input id="app-step-question" value={d.question} placeholder="What is the question about?" onChange={(e) => set({ question: e.target.value })} />
          </FormField>
          <FormField id="app-step-input" label="What it judges">
            <Input id="app-step-input" value={d.input} onChange={(e) => set({ input: e.target.value })} />
          </FormField>
          <FormField id="app-step-paths" label="Paths, one per line" help="Name: what it covers. A path keeps its steps while its name stays the same.">
            <Textarea id="app-step-paths" rows={4} value={d.paths} onChange={(e) => set({ paths: e.target.value })} />
          </FormField>
        </>
      );
    case "if":
      return <IfPaths paths={d.ifPaths} onChange={(ifPaths) => set({ ifPaths })} />;
    case "for_each":
      return (
        <>
          <FormField id="app-step-items" label="List" help="A reference to a list, such as {{steps.search}}.">
            <Input id="app-step-items" value={d.items} onChange={(e) => set({ items: e.target.value })} />
          </FormField>
          <FormField id="app-step-concurrency" label="At once">
            <Input id="app-step-concurrency" type="number" min={1} max={8} value={d.concurrency} onChange={(e) => set({ concurrency: e.target.value })} />
          </FormField>
        </>
      );
    case "http":
      return (
        <>
          <div className="grid grid-cols-[110px_1fr] gap-2">
            <FormField id="app-step-method" label="Method">
              <Select value={d.method} onValueChange={(method) => set({ method })}>
                <SelectTrigger id="app-step-method" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {METHODS.map((m) => (
                    <SelectItem key={m} value={m}>
                      {m}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </FormField>
            <FormField id="app-step-url" label="URL">
              <Input id="app-step-url" value={d.url} onChange={(e) => set({ url: e.target.value })} />
            </FormField>
          </div>
          <FormField id="app-step-headers" label="Headers, one per line" help="Name: value">
            <Mono id="app-step-headers" rows={3} value={d.headers} onChange={(e) => set({ headers: e.target.value })} />
          </FormField>
          <FormField id="app-step-body" label="Body" help="JSON, or plain text.">
            <Mono id="app-step-body" rows={4} value={d.body} onChange={(e) => set({ body: e.target.value })} />
          </FormField>
          <p className="text-xs text-muted-foreground">Reaches public addresses only.</p>
        </>
      );
    case "response":
      return (
        <FormField id="app-step-value" label="Answer with" help="JSON, with references. The app stops here.">
          <Mono id="app-step-value" rows={8} value={d.value} onChange={(e) => set({ value: e.target.value })} />
        </FormField>
      );
    default:
      return <p className="text-sm text-muted-foreground">Edit this step with Edit everything as JSON.</p>;
  }
}

export interface StepEditorProps {
  step: Step;
  /** A step not saved yet: Add step instead of Save step. */
  pending: boolean;
  canMoveUp: boolean;
  canMoveDown: boolean;
  busy: boolean;
  error: string;
  onSave: (step: Step) => void;
  onCancel: () => void;
  onMove: (by: number) => void;
  onDelete: () => void;
}

export function StepEditor({ step, pending, canMoveUp, canMoveDown, busy, error, onSave, onCancel, onMove, onDelete }: StepEditorProps) {
  const start = useMemo(() => toDraft(step), [step]);
  const [d, setD] = useState<StepDraft>(start);
  useEffect(() => setD(start), [start]);
  const set = (patch: Partial<StepDraft>) => setD((old) => ({ ...old, ...patch }));
  const dirty = JSON.stringify(d) !== JSON.stringify(start);
  const t = TYPE[step.type] || { name: step.type };
  return (
    <form
      id="app-step-form"
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        onSave(fromDraft(d, step));
      }}
    >
      <div className="flex items-center gap-2.5">
        <StepIcon type={step.type} />
        <div className="min-w-0 grow">
          <h3 className="truncate text-base font-semibold">{pending ? `Add ${t.name}` : step.title || t.name}</h3>
          <p className="text-xs text-muted-foreground">{pending ? "Not saved yet" : t.name}</p>
        </div>
        {!pending && (
          <div className="flex items-center">
            <Button type="button" variant="ghost" size="icon-sm" aria-label="Move up" title="Move up" disabled={!canMoveUp || busy} onClick={() => onMove(-1)}>
              <ArrowUp />
            </Button>
            <Button type="button" variant="ghost" size="icon-sm" aria-label="Move down" title="Move down" disabled={!canMoveDown || busy} onClick={() => onMove(1)}>
              <ArrowDown />
            </Button>
            <Button type="button" variant="ghost" size="icon-sm" aria-label="Delete step" title="Delete step" disabled={busy} onClick={onDelete}>
              <Trash2 />
            </Button>
          </div>
        )}
      </div>
      <div className="grid grid-cols-2 gap-2">
        <FormField id="app-step-title" label="Title">
          <Input id="app-step-title" value={d.title} maxLength={80} placeholder={t.name} onChange={(e) => set({ title: e.target.value })} />
        </FormField>
        <FormField id="app-step-id" label="Id">
          <Input id="app-step-id" value={d.id} maxLength={32} pattern={ID_PATTERN} required className="font-mono text-sm" onChange={(e) => set({ id: e.target.value })} />
        </FormField>
        <p className="col-span-2 -mt-1 text-xs text-muted-foreground">
          Other steps read this one as <code className="font-mono">{`{{steps.${d.id || "id"}}}`}</code>.
        </p>
      </div>
      <TypeFields d={{ ...d, type: step.type }} set={set} />
      <FormError message={error} />
      <div className="sticky bottom-0 -mx-4 flex justify-end gap-2 border-t bg-card px-4 py-3">
        {(pending || dirty) && (
          <Button type="button" variant="ghost" onClick={() => (pending ? onCancel() : setD(start))} disabled={busy}>
            {pending ? "Cancel" : "Revert"}
          </Button>
        )}
        <Button type="submit" disabled={busy || (!pending && !dirty)}>
          {pending ? "Add step" : "Save step"}
        </Button>
      </div>
    </form>
  );
}

export function InputsEditor({ inputs, busy, error, onSave }: { inputs: Field[]; busy: boolean; error: string; onSave: (inputs: Field[]) => void }) {
  const start = useMemo(() => [...toInputDrafts(inputs), { name: "", type: "text" as const, required: false }], [inputs]);
  const [rows, setRows] = useState<InputDraft[]>(start);
  useEffect(() => setRows(start), [start]);
  const set = (i: number, patch: Partial<InputDraft>) => setRows((old) => old.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const dirty = JSON.stringify(fromInputDrafts(rows)) !== JSON.stringify(fromInputDrafts(start));
  return (
    <form
      id="app-inputs-form"
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        onSave(fromInputDrafts(rows));
      }}
    >
      <div>
        <h3 className="text-base font-semibold">Inputs</h3>
        <p className="text-xs text-muted-foreground">
          What a call sends. Steps read them as <code className="font-mono">{"{{inputs.name}}"}</code>. Leave a name empty to remove it.
        </p>
      </div>
      <div className="grid gap-2">
        {rows.map((f, i) => (
          <div key={i} className="grid gap-2 rounded-md border p-2.5">
            <div className="flex items-center gap-1.5">
              <Input
                aria-label={`Name ${i + 1}`}
                value={f.name}
                placeholder="name"
                pattern={ID_PATTERN}
                maxLength={32}
                className="font-mono text-sm"
                onChange={(e) => set(i, { name: e.target.value })}
              />
              <Button type="button" variant="ghost" size="icon-sm" aria-label={`Remove input ${i + 1}`} title="Remove" onClick={() => setRows((old) => old.filter((_, j) => j !== i))}>
                <X />
              </Button>
            </div>
            <div className="flex items-center gap-3">
              <Select value={f.type} onValueChange={(v) => set(i, { type: v as InputDraft["type"] })}>
                <SelectTrigger aria-label={`Type ${i + 1}`} className="w-40">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {KINDS.map(([k, label]) => (
                    <SelectItem key={k} value={k}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Check id={`app-input-${i}-required`} checked={f.required} onChange={(required) => set(i, { required })} label="Required" />
            </div>
          </div>
        ))}
        <Button type="button" variant="ghost" size="sm" className="justify-self-start" onClick={() => setRows((old) => [...old, { name: "", type: "text", required: false }])}>
          <Plus /> Add an input
        </Button>
      </div>
      <FormError message={error} />
      <div className="sticky bottom-0 -mx-4 flex justify-end gap-2 border-t bg-card px-4 py-3">
        {dirty && (
          <Button type="button" variant="ghost" onClick={() => setRows(start)} disabled={busy}>
            Revert
          </Button>
        )}
        <Button type="submit" disabled={busy || !dirty}>
          Save inputs
        </Button>
      </div>
    </form>
  );
}
