// Apps: a project turned into an API. Steps live in a tree: Decide and If hold paths
// of steps, For each holds the steps it runs per item. A list in the tree is
// addressed by a dotted path from the definition, such as "steps.2.paths.0.steps",
// and a step by its list path and index, such as "steps.2.paths.0.steps.1".
// The server (api/src/apps/engine.rs) is the source of truth for these shapes.

export type StepType = "agent" | "search" | "code" | "decide" | "if" | "for_each" | "http" | "response";
export type Test = "is_empty" | "not_empty" | "equals" | "not_equals" | "contains" | "more_than" | "less_than";
export type Kind = "text" | "number" | "boolean" | "json";

export interface Rule {
  value?: string;
  test?: Test;
  to?: string;
  code?: string;
}
export interface Choice {
  name: string;
  description?: string;
  steps: Step[];
}
export interface Branch {
  when: Rule[];
  steps: Step[];
}
export interface Step {
  type: StepType;
  id: string;
  title?: string;
  // agent, decide
  model?: string;
  instructions?: string;
  prompt?: string;
  json?: boolean;
  question?: string;
  input?: string;
  // decide, if
  paths?: (Choice | Branch)[];
  otherwise?: Step[];
  // for_each
  items?: string;
  steps?: Step[];
  concurrency?: number | null;
  // search
  collections?: string[];
  query?: string;
  limit?: number | null;
  // http
  method?: string;
  url?: string;
  headers?: Record<string, string>;
  body?: unknown;
  // code
  code?: string;
  // response
  value?: unknown;
}
export interface Field {
  name: string;
  type?: Kind;
  required?: boolean;
  description?: string;
}
export interface Definition {
  inputs: Field[];
  steps: Step[];
}

/** Step types in the palette's order: [type, name, icon name]. */
export const TYPES: [StepType, string, string][] = [
  ["agent", "Agent", "chat"],
  ["search", "Search knowledge", "book"],
  ["code", "Code", "code"],
  ["decide", "Decide", "compare"],
  ["if", "If", "branch"],
  ["for_each", "For each", "refresh"],
  ["http", "HTTP request", "globe"],
  ["response", "Response", "download"],
];
export const TYPE: Record<string, { name: string; glyph: string }> = Object.fromEntries(TYPES.map(([id, name, glyph]) => [id, { name, glyph }]));
/** What each step type is for, shown in the palette. */
export const TYPE_HELP: Record<StepType, string> = {
  agent: "Ask a model",
  search: "Find passages in the project’s knowledge",
  code: "Run a little JavaScript",
  decide: "A model picks a path",
  if: "Rules pick a path",
  for_each: "Run steps for every item of a list",
  http: "Call a public web address",
  response: "Answer and stop",
};
export const TESTS: [Test, string][] = [
  ["is_empty", "is empty"],
  ["not_empty", "is not empty"],
  ["equals", "equals"],
  ["not_equals", "does not equal"],
  ["contains", "contains"],
  ["more_than", "is more than"],
  ["less_than", "is less than"],
];
export const COMPARES = new Set<string>(["equals", "not_equals", "contains", "more_than", "less_than"]);
export const KINDS: [Kind, string][] = [
  ["text", "Text"],
  ["number", "Number"],
  ["boolean", "True or false"],
  ["json", "JSON"],
];
export const MAX_RULES = 3;
export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];
export const isBranching = (s: Step) => s.type === "decide" || s.type === "if" || s.type === "for_each";

/** The value at a dotted path, such as "steps.0.paths.1.steps". */
export function listAt(definition: unknown, path: string): any {
  return path.split(".").reduce<any>((at, key) => at?.[/^\d+$/.test(key) ? Number(key) : key], definition);
}
/** Every step id in the tree. */
export function ids(steps: Step[] = []): string[] {
  return steps.flatMap((s) => [s.id, ...ids(s.steps), ...(s.paths || []).flatMap((p) => ids(p.steps)), ...ids(s.otherwise)]);
}
/** A step id not yet taken: the type, then type_2, type_3… */
export function freshId(definition: Pick<Definition, "steps">, type: string): string {
  const taken = new Set(ids(definition.steps));
  for (let n = 1; ; n++) {
    const id = n === 1 ? type : `${type}_${n}`;
    if (!taken.has(id)) return id;
  }
}
const DEFAULTS = (model: string): Record<StepType, Partial<Step>> => ({
  agent: { model, instructions: "", prompt: "{{inputs.question}}", json: false },
  search: { query: "{{inputs.question}}", limit: 8 },
  code: { code: "return {};" },
  decide: { model, question: "", input: "{{inputs.question}}", paths: [{ name: "First path", description: "", steps: [] }], otherwise: [] },
  if: { paths: [{ when: [{ value: "{{inputs.question}}", test: "is_empty" }], steps: [] }], otherwise: [] },
  for_each: { items: "{{inputs.items}}", steps: [], concurrency: 4 },
  http: { method: "GET", url: "https://", headers: {}, body: null },
  response: { value: { answer: "{{steps.agent}}" } },
});
/** A new step of a type, with the defaults the old step editor started from. */
export function blank(type: StepType, id: string, model = ""): Step {
  return { type, id, title: "", ...structuredClone(DEFAULTS(model)[type]) } as Step;
}
export const testLabel = (test?: string) => (TESTS.find(([id]) => id === test) || [undefined, test])[1];
export const ruleText = (r: Rule) => (r.code ? `code: ${r.code}` : `${r.value} ${testLabel(r.test)}${COMPARES.has(r.test || "") ? ` “${r.to ?? ""}”` : ""}`);
/** JSON for a value, or the value itself when it is text. */
export const pretty = (value: unknown) => (typeof value === "string" ? value : JSON.stringify(value, null, 2));
/** JSON when it parses, else the text. */
export const parseLoose = (text: string): unknown => {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
};

/** One line on what a step does. */
export function summary(s: Step): string {
  if (s.type === "agent") return `${s.model || "No model"}${s.json ? " · JSON" : ""} · ${s.prompt}`;
  if (s.type === "search") return `${s.collections?.length ? `${s.collections.length} collection${s.collections.length === 1 ? "" : "s"}` : "The project’s knowledge"} · ${s.query}`;
  if (s.type === "code") return (s.code || "").split("\n").find((l) => l.trim()) || "";
  if (s.type === "decide") return `${s.model || "No model"} picks a path for ${s.input}`;
  if (s.type === "if") return "Rules, checked in order";
  if (s.type === "for_each") return `Each of ${s.items}, ${s.concurrency || 4} at a time`;
  if (s.type === "http") return `${s.method} ${s.url}`;
  if (s.type === "response") return typeof s.value === "string" ? s.value : JSON.stringify(s.value);
  return "";
}
/** The paths a branching step holds, each with its label and list path. */
export function branches(s: Step, at: string): [string, string][] {
  if (s.type === "decide") return [...(s.paths as Choice[]).map((p, i): [string, string] => [`When: ${p.name}`, `${at}.paths.${i}.steps`]), ["Otherwise", `${at}.otherwise`]];
  if (s.type === "if")
    return [...(s.paths as Branch[]).map((p, i): [string, string] => [`${i ? "Else if" : "If"} ${p.when.map(ruleText).join(" and ")}`, `${at}.paths.${i}.steps`]), ["Otherwise", `${at}.otherwise`]];
  if (s.type === "for_each") return [["For each item", `${at}.steps`]];
  return [];
}
/** What the trace calls the path a branch took: a Decide path's name, "Path n" for If, or "Otherwise". */
export function traceName(s: Step, listPath: string): string {
  const m = listPath.match(/\.paths\.(\d+)\.steps$/);
  if (!m) return "Otherwise";
  const i = Number(m[1]);
  return s.type === "decide" ? (s.paths as Choice[])[i]!.name : `Path ${i + 1}`;
}

// ---- Edits on a copy of the tree ----

export const copyOf = <T,>(value: T): T => JSON.parse(JSON.stringify(value));
/** Splits a step path into its list's path and its index. */
export function splitPath(stepPath: string): [string, number] {
  const i = stepPath.lastIndexOf(".");
  return [stepPath.slice(0, i), Number(stepPath.slice(i + 1))];
}
export function stepAt(definition: Definition, stepPath: string): Step | undefined {
  const [list, index] = splitPath(stepPath);
  return listAt(definition, list)?.[index];
}
/** A copy with `step` put into the list at `index`. */
export function insertStep(definition: Definition, list: string, index: number, step: Step): Definition {
  const d = copyOf(definition);
  listAt(d, list).splice(index, 0, step);
  return d;
}
/** A copy with the step at `stepPath` replaced. */
export function replaceStep(definition: Definition, stepPath: string, step: Step): Definition {
  const d = copyOf(definition);
  const [list, index] = splitPath(stepPath);
  listAt(d, list)[index] = step;
  return d;
}
/** A copy without the step at `stepPath`, and the step removed. */
export function removeStep(definition: Definition, stepPath: string): [Definition, Step] {
  const d = copyOf(definition);
  const [list, index] = splitPath(stepPath);
  const [gone] = listAt(d, list).splice(index, 1);
  return [d, gone];
}
/** A copy with the step at `stepPath` swapped with its neighbour `by` places away. */
export function moveStep(definition: Definition, stepPath: string, by: number): Definition {
  const d = copyOf(definition);
  const [at, from] = splitPath(stepPath);
  const list = listAt(d, at);
  const to = from + by;
  if (to < 0 || to >= list.length) return d;
  [list[from], list[to]] = [list[to], list[from]];
  return d;
}

/** The path of the step with this id, such as "steps.2.otherwise.0", or null. */
export function pathOf(definition: Pick<Definition, "steps">, id: string): string | null {
  const walk = (list: Step[], at: string): string | null => {
    for (let i = 0; i < list.length; i++) {
      const s = list[i]!;
      const here = `${at}.${i}`;
      if (s.id === id) return here;
      const found =
        (s.steps && walk(s.steps, `${here}.steps`)) ||
        (s.paths || []).reduce<string | null>((f, p, j) => f || walk(p.steps || [], `${here}.paths.${j}.steps`), null) ||
        (s.otherwise && walk(s.otherwise, `${here}.otherwise`));
      if (found) return found;
    }
    return null;
  };
  return walk(definition.steps, "steps");
}
/** Whether the list at `list` lies inside the step at `stepPath` (a step can't move into itself). */
export const isInside = (list: string, stepPath: string) => list === stepPath || list.startsWith(`${stepPath}.`);
/** Whether putting the step at `stepPath` at `list`@`index` would leave it where it is. */
export function staysPut(stepPath: string, list: string, index: number): boolean {
  const [from, i] = splitPath(stepPath);
  return from === list && (index === i || index === i + 1);
}
/** A copy with the step at `stepPath` moved to `list` at `index` (an index in the list as it is now). */
export function moveStepTo(definition: Definition, stepPath: string, list: string, index: number): Definition {
  if (isInside(list, stepPath)) return copyOf(definition);
  const d = copyOf(definition);
  const [fromList] = splitPath(stepPath);
  const step = stepAt(d, stepPath)!;
  const source: Step[] = listAt(d, fromList);
  const target: Step[] = listAt(d, list);
  // Array references stay valid across splices, so mark the place, take the step out, then fill the mark.
  const mark = {} as Step;
  target.splice(index, 0, mark);
  source.splice(source.indexOf(step), 1);
  target[target.indexOf(mark)] = step;
  return d;
}
