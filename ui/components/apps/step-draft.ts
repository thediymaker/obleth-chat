// The step editor's fields as text, and the step they describe, so a step reads
// back the same.

import { type Branch, type Choice, COMPARES, type Definition, type Field, type Kind, MAX_RULES, parseLoose, pretty, type Rule, type Step, type Test } from "./model";

export interface RuleDraft {
  value: string;
  test: Test;
  to: string;
}
export interface IfPathDraft {
  /** The path it edits, so its steps stay; -1 for a new one. */
  from: number;
  rules: RuleDraft[];
  code: string;
}
export interface StepDraft {
  id: string;
  title: string;
  model: string;
  instructions: string;
  prompt: string;
  json: boolean;
  query: string;
  limit: string;
  code: string;
  question: string;
  input: string;
  paths: string;
  ifPaths: IfPathDraft[];
  items: string;
  concurrency: string;
  method: string;
  url: string;
  headers: string;
  body: string;
  value: string;
}

const blankRule = (): RuleDraft => ({ value: "", test: "is_empty", to: "" });

export function toDraft(s: Step): StepDraft {
  const ifPaths: IfPathDraft[] =
    s.type === "if"
      ? (s.paths as Branch[]).map((p, i) => ({
          from: i,
          rules: p.when.filter((r) => !r.code).map((r) => ({ value: r.value || "", test: r.test || "is_empty", to: r.to || "" })),
          code: p.when.find((r) => r.code)?.code || "",
        }))
      : [];
  return {
    id: s.id,
    title: s.title || "",
    model: s.model || "",
    instructions: s.instructions || "",
    prompt: s.prompt || "",
    json: !!s.json,
    query: s.query || "",
    limit: String(s.limit || 8),
    code: s.code || "",
    question: s.question || "",
    input: s.input || "",
    paths: s.type === "decide" ? (s.paths as Choice[]).map((p) => (p.description ? `${p.name}: ${p.description}` : p.name)).join("\n") : "",
    ifPaths,
    items: s.items || "",
    concurrency: String(s.concurrency || 4),
    method: s.method || "GET",
    url: s.url || "",
    headers: Object.entries(s.headers || {})
      .map(([k, v]) => `${k}: ${v}`)
      .join("\n"),
    body: s.body == null ? "" : pretty(s.body),
    value: s.type === "response" ? pretty(s.value) : "",
  };
}

/** A new path for an If, as the old editor's "Add another path" made it. */
export const newIfPath = (): IfPathDraft => ({ from: -1, rules: [{ value: "{{inputs.question}}", test: "is_empty", to: "" }], code: "" });
export { blankRule };

/** The step the fields describe, built on the one they edit. */
export function fromDraft(d: StepDraft, old: Step): Step {
  const s: Step = { ...old, id: d.id.trim(), title: d.title.trim() };
  if (s.type === "agent") Object.assign(s, { model: d.model, instructions: d.instructions, prompt: d.prompt, json: d.json });
  if (s.type === "search") Object.assign(s, { query: d.query, limit: Number(d.limit) || 8 });
  if (s.type === "code") s.code = d.code;
  if (s.type === "decide") {
    const before = (old.paths || []) as Choice[];
    const lines = d.paths
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    s.paths = lines.map((line) => {
      const [name = "", ...rest] = line.split(":");
      const n = name.trim();
      return { name: n, description: rest.join(":").trim(), steps: before.find((p) => p.name === n)?.steps || [] };
    });
    Object.assign(s, { model: d.model, question: d.question, input: d.input });
  }
  if (s.type === "if") {
    const before = (old.paths || []) as Branch[];
    s.paths = d.ifPaths.map((p): Branch => {
      const rules: Rule[] = p.rules
        .slice(0, MAX_RULES)
        .map((r) => ({ value: r.value.trim(), test: r.test, to: r.to }))
        .filter((r) => r.value)
        .map((r) => (COMPARES.has(r.test) ? r : { value: r.value, test: r.test }));
      if (p.code.trim()) rules.push({ code: p.code.trim() });
      return { ...(before[p.from] || {}), when: rules, steps: before[p.from]?.steps || [] };
    });
  }
  if (s.type === "for_each") Object.assign(s, { items: d.items, concurrency: Number(d.concurrency) || 4 });
  if (s.type === "http") {
    const headers = Object.fromEntries(
      d.headers
        .split("\n")
        .map((l) => l.split(":"))
        .filter(([k, ...rest]) => k!.trim() && rest.length)
        .map(([k, ...rest]) => [k!.trim(), rest.join(":").trim()]),
    );
    Object.assign(s, { method: d.method, url: d.url.trim(), headers, body: d.body.trim() ? parseLoose(d.body) : null });
  }
  if (s.type === "response") s.value = parseLoose(d.value);
  return s;
}

export interface InputDraft {
  name: string;
  type: Kind;
  required: boolean;
}
export const toInputDrafts = (inputs: Field[]): InputDraft[] => inputs.map((f) => ({ name: f.name, type: f.type || "text", required: !!f.required }));
/** The inputs the rows describe; a row without a name is dropped. */
export const fromInputDrafts = (rows: InputDraft[]): Field[] =>
  rows.map((f) => ({ name: f.name.trim(), type: f.type, required: f.required, description: "" })).filter((f) => f.name);

/** What a try sends: each field read by its type; empty ones are left out. Throws on bad JSON. */
export function readTryInputs(inputs: Definition["inputs"], values: Record<string, string | boolean>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of inputs) {
    const raw = values[f.name];
    if (f.type === "boolean") {
      out[f.name] = !!raw;
      continue;
    }
    const text = typeof raw === "string" ? raw : "";
    if (text.trim() === "") continue;
    if (f.type === "number") out[f.name] = Number(text);
    else if (f.type === "json") {
      try {
        out[f.name] = JSON.parse(text);
      } catch {
        throw new Error(`${f.name} isn’t valid JSON`);
      }
    } else out[f.name] = text;
  }
  return out;
}

/** The curl example of the API card. */
export function curlExample(origin: string, endpoint: string, inputs: Field[]): string {
  const example = inputs.reduce<Record<string, unknown>>((o, f) => ({ ...o, [f.name]: f.type === "number" ? 1 : f.type === "boolean" ? true : f.type === "json" ? [] : "…" }), {});
  return `curl ${origin}${endpoint} \\\n  -H "Authorization: Bearer $APP_KEY" \\\n  -H "Content-Type: application/json" \\\n  -d '${JSON.stringify(example)}'`;
}
