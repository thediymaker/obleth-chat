// What Chat draws in a reply beyond plain Markdown: math, highlighted code, Mermaid
// diagrams and charts, molecules from SMILES, SVG drawings and CSV tables. Each library
// loads the first time a reply needs it. Until then, and while a reply is still
// writing a block, the block shows as code.

import type { HLJSApi, LanguageFn } from "highlight.js";
import type katexType from "katex";
import { svgIcon } from "@/lib/chat/icon-svg";

type Katex = typeof katexType;

// Libraries markdown() uses directly once they have loaded.
export const loaded: { katex: Katex | null; hljs: HLJSApi | null } = { katex: null, hljs: null };

const escape = (value: unknown) =>
  String(value).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
const theme = () => (globalThis.document?.documentElement?.dataset.theme === "dark" ? "dark" : "light");
// Small caches, so a reply drawn again while it streams costs nothing for what is done.
const LIMIT = 300;
function remember<V>(map: Map<string, V>, key: string, value: V): V {
  map.delete(key);
  map.set(key, value);
  if (map.size > LIMIT) map.delete(map.keys().next().value!);
  return value;
}
export interface Drawing {
  html?: string;
  error?: string;
}
const maths = new Map<string, string>(),
  lights = new Map<string, string>(),
  drawings = new Map<string, Drawing>();
// Blocks the person switched to their code, by kind and source.
const showingCode = new Set<string>();

// Fenced blocks Chat draws, by their language, and what each is called.
export const DRAWN: Record<string, string> = { mermaid: "Diagram", smiles: "Molecule", svg: "Drawing", csv: "Table", tsv: "Table" };
const LONGEST = 50_000;

/* ---------- Math ---------- */

const KATEX_OPTIONS = { throwOnError: false, strict: "ignore", trust: false, maxSize: 50, maxExpand: 1000 } as const;
/** HTML for math: KaTeX's once it has loaded, else the TeX as written until it has. */
export function math(tex: string, display: boolean, markup = "$", block = false): string {
  const tag = block ? "div" : "span",
    cls = `math${display ? " display" : ""}`;
  if (loaded.katex) {
    const key = `${display ? 1 : 0}${tex}`;
    const html = maths.get(key) ?? remember(maths, key, loaded.katex.renderToString(tex, { ...KATEX_OPTIONS, displayMode: display }));
    return `<${tag} class="${cls}">${html}</${tag}>`;
  }
  const close = ({ "\\(": "\\)", "\\[": "\\]" } as Record<string, string>)[markup] || markup;
  return `<${tag} class="${cls}" data-render="math" data-tex="${escape(tex)}"${display ? " data-display" : ""}>${escape(markup + tex + close)}</${tag}>`;
}
let katexLoading: Promise<void> | undefined;
/** Loads KaTeX with chemistry (\ce); its stylesheet comes with the chat's CSS. */
export function loadMath(): Promise<void> {
  katexLoading ||= import("katex").then(async (module) => {
    await import("katex/contrib/mhchem");
    loaded.katex = (module.default ?? module) as Katex;
  });
  return katexLoading;
}

/* ---------- Code ---------- */

// Languages the common bundle lacks, by the names models use, to the grammar file.
const GRAMMARS: Record<string, string> = {
  dockerfile: "dockerfile", docker: "dockerfile", julia: "julia", jl: "julia", "julia-repl": "julia-repl", jldoctest: "julia-repl",
  fortran: "fortran", f90: "fortran", f95: "fortran", matlab: "matlab", latex: "latex", tex: "latex",
  powershell: "powershell", ps1: "powershell", pwsh: "powershell", scala: "scala", haskell: "haskell", hs: "haskell",
  elixir: "elixir", ex: "elixir", exs: "elixir", erlang: "erlang", erl: "erlang", nginx: "nginx", cmake: "cmake",
  protobuf: "protobuf", proto: "protobuf", dart: "dart", clojure: "clojure", clj: "clojure", groovy: "groovy",
  mathematica: "mathematica", wolfram: "mathematica", wl: "mathematica", verilog: "verilog", systemverilog: "verilog", sv: "verilog",
  vhdl: "vhdl", x86asm: "x86asm", asm: "x86asm", nasm: "x86asm", llvm: "llvm", ocaml: "ocaml", ml: "ocaml",
  fsharp: "fsharp", fs: "fsharp", scheme: "scheme", lisp: "lisp", elisp: "lisp", awk: "awk", tcl: "tcl",
  apache: "apache", apacheconf: "apache", properties: "properties", http: "http", dos: "dos", bat: "dos", batch: "dos", cmd: "dos",
  stata: "stata", sas: "sas", arduino: "arduino", ino: "arduino", glsl: "glsl", gradle: "gradle", nix: "nix",
};
// Each grammar file by name, so the bundler makes one small chunk per language.
type Grammar = () => Promise<{ default: LanguageFn }>;
const GRAMMAR_FILES: Record<string, Grammar> = {
  dockerfile: () => import("highlight.js/lib/languages/dockerfile"),
  julia: () => import("highlight.js/lib/languages/julia"),
  "julia-repl": () => import("highlight.js/lib/languages/julia-repl"),
  fortran: () => import("highlight.js/lib/languages/fortran"),
  matlab: () => import("highlight.js/lib/languages/matlab"),
  latex: () => import("highlight.js/lib/languages/latex"),
  powershell: () => import("highlight.js/lib/languages/powershell"),
  scala: () => import("highlight.js/lib/languages/scala"),
  haskell: () => import("highlight.js/lib/languages/haskell"),
  elixir: () => import("highlight.js/lib/languages/elixir"),
  erlang: () => import("highlight.js/lib/languages/erlang"),
  nginx: () => import("highlight.js/lib/languages/nginx"),
  cmake: () => import("highlight.js/lib/languages/cmake"),
  protobuf: () => import("highlight.js/lib/languages/protobuf"),
  dart: () => import("highlight.js/lib/languages/dart"),
  clojure: () => import("highlight.js/lib/languages/clojure"),
  groovy: () => import("highlight.js/lib/languages/groovy"),
  mathematica: () => import("highlight.js/lib/languages/mathematica"),
  verilog: () => import("highlight.js/lib/languages/verilog"),
  vhdl: () => import("highlight.js/lib/languages/vhdl"),
  x86asm: () => import("highlight.js/lib/languages/x86asm"),
  llvm: () => import("highlight.js/lib/languages/llvm"),
  ocaml: () => import("highlight.js/lib/languages/ocaml"),
  fsharp: () => import("highlight.js/lib/languages/fsharp"),
  scheme: () => import("highlight.js/lib/languages/scheme"),
  lisp: () => import("highlight.js/lib/languages/lisp"),
  awk: () => import("highlight.js/lib/languages/awk"),
  tcl: () => import("highlight.js/lib/languages/tcl"),
  apache: () => import("highlight.js/lib/languages/apache"),
  properties: () => import("highlight.js/lib/languages/properties"),
  http: () => import("highlight.js/lib/languages/http"),
  dos: () => import("highlight.js/lib/languages/dos"),
  stata: () => import("highlight.js/lib/languages/stata"),
  sas: () => import("highlight.js/lib/languages/sas"),
  arduino: () => import("highlight.js/lib/languages/arduino"),
  glsl: () => import("highlight.js/lib/languages/glsl"),
  gradle: () => import("highlight.js/lib/languages/gradle"),
  nix: () => import("highlight.js/lib/languages/nix"),
};
// Names models use for languages a bundled grammar already reads.
const ALIASES: Record<string, string> = { slurm: "bash", sbatch: "bash", cuda: "cpp", cu: "cpp", jsonc: "json", json5: "json", ipython: "python", cfg: "ini", conf: "ini" };
const PLAIN = new Set(["", "text", "txt", "plain", "plaintext", "output", "console-output"]);
/** The highlight.js language for a fence's language hint, or '' for none. */
export const languageOf = (hint: unknown): string => {
  const name = String(hint || "").toLowerCase();
  if (PLAIN.has(name)) return "";
  // Own entries only: "constructor" is a name a fence can carry, and not a language.
  return (Object.hasOwn(GRAMMARS, name) && GRAMMARS[name]) || (Object.hasOwn(ALIASES, name) && ALIASES[name]) || name;
};
/** Highlighted HTML for code, or null when the highlighter or the language isn't loaded. */
export function highlight(code: string, language: string): string | null {
  const hljs = loaded.hljs;
  if (!hljs || !language || !hljs.getLanguage(language) || code.length > 200_000) return null;
  const key = `${language}\n${code}`;
  return lights.get(key) ?? remember(lights, key, hljs.highlight(code, { language, ignoreIllegals: true }).value);
}
let hljsLoading: Promise<void> | undefined;
const triedGrammars = new Set<string>();
/** Loads the highlighter, and the grammar files of any of `languages` it lacks. */
export async function loadHighlight(languages: string[] = []): Promise<void> {
  hljsLoading ||= import("highlight.js/lib/common").then((module) => {
    loaded.hljs = (module.default ?? module) as HLJSApi;
  });
  await hljsLoading;
  for (const language of languages) {
    const file = Object.hasOwn(GRAMMAR_FILES, language) ? language : null;
    if (!file || loaded.hljs!.getLanguage(file) || triedGrammars.has(file)) continue;
    triedGrammars.add(file);
    try {
      loaded.hljs!.registerLanguage(file, (await GRAMMAR_FILES[file]!()).default);
    } catch {
      /* Shown as plain code. */
    }
  }
}

/* ---------- Drawn blocks ---------- */

/** Rows of a CSV or TSV text, reading quoted fields as RFC 4180 does. */
export function parseDelimited(text: string, separator = ","): string[][] {
  const rows: string[][] = [];
  let row: string[] = [],
    field = "",
    quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"' && field === "") quoted = true;
    else if (c === separator) {
      row.push(field);
      field = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(field);
      field = "";
      if (row.length > 1 || row[0] !== "") rows.push(row);
      row = [];
    } else field += c;
  }
  row.push(field);
  if (row.length > 1 || row[0] !== "") rows.push(row);
  return rows;
}
const NUMBER = /^[-+]?(\d[\d,]*\.?\d*|\.\d+)([eE][-+]?\d+)?%?$/;
const ROWS = 200,
  COLUMNS = 40;
function table(source: string, separator: string): Drawing {
  const rows = parseDelimited(source, separator);
  if (rows.length < 2) return { error: "Chat shows a table once there is a header row and at least one more." };
  const [head, ...body] = rows as [string[], ...string[][]],
    width = Math.min(COLUMNS, Math.max(...rows.map((r) => r.length)));
  const numeric = Array.from({ length: width }, (_, c) => body.every((r) => !r[c]?.trim() || NUMBER.test(r[c]!.trim())));
  const cell = (tag: string, value: string | undefined, c: number) => `<${tag}${numeric[c] ? ' class="number"' : ""}>${escape(value ?? "")}</${tag}>`;
  const shown = body.slice(0, ROWS);
  const wide = rows.some((r) => r.length > COLUMNS);
  const note =
    body.length > ROWS || wide
      ? `<p class="drawing-note">Showing ${Math.min(body.length, ROWS).toLocaleString()} of ${body.length.toLocaleString()} rows${wide ? ` and the first ${COLUMNS} columns` : ""}. Download it for the rest.</p>`
      : "";
  return {
    html: `<div class="table-scroll" tabindex="0" role="region" aria-label="Table"><table><thead><tr>${Array.from({ length: width }, (_, c) => cell("th", head[c], c)).join("")}</tr></thead><tbody>${shown.map((r) => `<tr>${Array.from({ length: width }, (_, c) => cell("td", r[c], c)).join("")}</tr>`).join("")}</tbody></table></div>${note}`,
  };
}
/**
 * Where the opening <svg tag starts when the text is an SVG: after an XML declaration,
 * comments and a doctype, and nothing else. -1 otherwise. Comments are skipped one at
 * a time, since a pattern that repeats over them takes time that doubles with each
 * comment when the text turns out not to be an SVG.
 */
function svgStart(svg: string): number {
  let at = /^<\?xml[^>]*>\s*/i.exec(svg)?.[0].length ?? 0;
  while (svg.startsWith("<!--", at)) {
    const end = svg.indexOf("-->", at + 4);
    if (end < 0) return -1;
    at = end + 3;
    while (/\s/.test(svg.charAt(at))) at++;
  }
  at += /^<!DOCTYPE[^>]*>\s*/i.exec(svg.slice(at))?.[0].length ?? 0;
  return /^<svg[\s>]/i.test(svg.slice(at, at + 5)) ? at : -1;
}
/** An SVG as an image, so its scripts never run and it loads nothing; null if it isn't one. */
export function svgImage(source: string): string | null {
  const svg = source.trim();
  if (svg.length > 200_000) return null;
  const start = svgStart(svg);
  if (start < 0) return null;
  // The namespace is looked for in the opening tag alone.
  const close = svg.indexOf(">", start);
  const tag = svg.slice(start, close < 0 ? svg.length : close);
  const named = /\sxmlns\s*=/i.test(tag) ? svg : `${svg.slice(0, start)}<svg xmlns="http://www.w3.org/2000/svg"${svg.slice(start + 4)}`;
  return `<img src="data:image/svg+xml;charset=utf-8,${encodeURIComponent(named)}" alt="Drawing">`;
}
const keyOf = (kind: string, source: string) => `${kind}\n${["mermaid", "smiles"].includes(kind) ? theme() : ""}\n${source}`;
// What a block of each kind becomes: { html } or { error }. Mermaid and molecules
// load a library and draw in the page, so they come from the cache or are drawn later.
function drawingOf(kind: string, source: string): Drawing | null {
  if (source.length > LONGEST) return { error: `Chat draws blocks of up to ${LONGEST.toLocaleString()} characters.` };
  if (kind === "svg") {
    const html = svgImage(source);
    return html ? { html } : { error: "This isn’t an SVG drawing Chat can show." };
  }
  if (kind === "csv" || kind === "tsv") {
    const key = keyOf(kind, source);
    return drawings.get(key) ?? remember(drawings, key, table(source, kind === "tsv" ? "\t" : ","));
  }
  return drawings.get(keyOf(kind, source)) || null;
}
/** Whether the person switched this block to its code. */
export const showsCode = (kind: string, source: string) => showingCode.has(`${kind}\n${source}`);
/** HTML for a finished fenced block Chat draws: the drawing, or its code until it's drawn. */
export function drawn(kind: string, source: string): string {
  const drawing = drawingOf(kind, source),
    name = DRAWN[kind]!;
  const code = !drawing?.html || showsCode(kind, source);
  const toggle = drawing?.html
    ? `<button type="button" class="row" data-action="toggle-drawing" aria-pressed="${code}">${svgIcon(code ? "image" : "code")} ${code ? name : "Code"}</button>`
    : "";
  const download =
    drawing?.html && (kind !== "smiles" || !source.trim().includes("\n"))
      ? `<button type="button" class="row" data-action="download-drawing" aria-label="Download ${name.toLowerCase()}">${svgIcon("download")} Download</button>`
      : "";
  const status = drawing ? "" : '<span class="drawing-status" role="status">Drawing…</span>';
  return `<div class="code-block drawn${code ? " show-code" : ""}" data-kind="${kind}"${drawing ? "" : ` data-render="${kind}"`}><div class="code-label"><span>${kind}</span>${status}<span class="row">${toggle}${download}<button type="button" class="row" data-action="copy-code" aria-label="Copy code">${svgIcon("copy")} Copy</button><button type="button" class="row code-save" data-action="save-code" aria-label="Save code to Library">${svgIcon("bookmark")} Save</button></span></div>${drawing?.html ? `<div class="drawing drawing-${kind}">${drawing.html}</div>` : ""}${drawing?.error ? `<p class="drawing-note">${escape(drawing.error)}</p>` : ""}<pre><code>${escape(source)}</code></pre></div>\n`;
}

type Mermaid = typeof import("mermaid").default;
let mermaidLoading: Promise<Mermaid> | undefined,
  queue: Promise<unknown> = Promise.resolve(),
  serial = 0;
const firstLine = (error: unknown) =>
  String((error as Error)?.message || error || "unknown error")
    .split("\n")
    .find((l) => l.trim())
    ?.trim()
    .slice(0, 200);
// Mermaid draws one diagram at a time in the page, so diagrams take turns.
function drawMermaid(source: string): Promise<Drawing> {
  const run = async (): Promise<Drawing> => {
    mermaidLoading ||= import("mermaid").then((module) => module.default);
    const mermaid = await mermaidLoading,
      id = `drawing-${++serial}`;
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: theme() === "dark" ? "dark" : "default",
      htmlLabels: false,
      flowchart: { htmlLabels: false },
      fontFamily: getComputedStyle(document.body).fontFamily,
      maxTextSize: LONGEST,
    });
    try {
      return { html: (await mermaid.render(id, source)).svg };
    } catch (error) {
      return { error: `Couldn’t draw this diagram: ${firstLine(error)}` };
    } finally {
      document.getElementById(id)?.remove();
      document.getElementById(`d${id}`)?.remove();
    }
  };
  const result = queue.then(run, run);
  queue = result.catch(() => {});
  return result;
}
type SmilesDrawer = typeof import("smiles-drawer").default;
let smilesLoading: Promise<SmilesDrawer> | undefined;
// One molecule per line: a SMILES string, then an optional name.
async function drawSmiles(source: string): Promise<Drawing> {
  smilesLoading ||= import("smiles-drawer").then((module) => module.default);
  const SmilesDrawer = await smilesLoading;
  const lines = source
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
  if (!lines.length) return { error: "There is no SMILES to draw." };
  if (lines.length > 12) return { error: "Chat draws up to 12 molecules in one block." };
  // The drawer measures text, so it draws into the page, out of sight.
  const stage = document.createElement("div");
  stage.className = "drawing-stage";
  document.body.append(stage);
  try {
    const figures: string[] = [];
    for (const line of lines) {
      const [smiles = "", ...name] = line.split(/\s+/);
      const tree = await new Promise<unknown>((resolve, reject) => SmilesDrawer.parse(smiles, resolve, reject));
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      stage.append(svg);
      // eslint-disable-next-line @typescript-eslint/no-explicit-any -- its typings name the tree type privately
      new (SmilesDrawer.SvgDrawer as any)({ width: 280, height: 220 }).draw(tree, svg, theme());
      figures.push(`<figure>${svg.outerHTML}<figcaption>${escape(name.join(" ") || smiles)}</figcaption></figure>`);
    }
    return { html: figures.join("") };
  } catch (error) {
    return { error: `Couldn’t draw this molecule: ${firstLine(error)}` };
  } finally {
    stage.remove();
  }
}

/* ---------- In the page ---------- */

const pending = new Map<string, Promise<void>>();
/** Loads or draws what one `[data-render]` element waits for. */
export async function prepare(el: HTMLElement): Promise<void> {
  const kind = el.dataset.render!;
  if (kind === "math") return loadMath();
  if (kind === "code") return loadHighlight([el.dataset.language || ""]);
  const source = el.querySelector("code")?.textContent || "",
    key = keyOf(kind, source);
  if (drawings.has(key)) return;
  if (!pending.has(key))
    pending.set(
      key,
      (kind === "mermaid" ? drawMermaid(source) : drawSmiles(source)).then((result) => {
        remember(drawings, key, result);
        pending.delete(key);
      }),
    );
  return pending.get(key);
}
/** Waits for everything the blocks in `root` need. Markdown drawn again afterwards shows it. */
export async function hydrate(root: ParentNode): Promise<boolean> {
  const waiting = [...root.querySelectorAll<HTMLElement>("[data-render]")];
  if (!waiting.length) return false;
  await Promise.all(waiting.map((el) => prepare(el).catch(() => {})));
  return true;
}
/** Switches a drawn block between its drawing and its code, and keeps the choice. */
export function toggleDrawing(kind: string, source: string) {
  const key = `${kind}\n${source}`;
  if (showingCode.has(key)) showingCode.delete(key);
  else showingCode.add(key);
}
/** Saves a drawn block as a file: the SVG of a drawing, or the table's data. */
export function downloadDrawing(block: HTMLElement) {
  const kind = block.dataset.kind!,
    source = block.querySelector("code")?.textContent || "";
  const svg = block.querySelector(".drawing svg");
  const image = block.querySelector<HTMLImageElement>(".drawing img");
  const [body, type, name] =
    kind === "csv" || kind === "tsv"
      ? [source, `text/${kind === "csv" ? "csv" : "tab-separated-values"}`, `table.${kind}`]
      : kind === "svg"
        ? [decodeURIComponent((image?.getAttribute("src") || "").split(",").slice(1).join(",")), "image/svg+xml", "drawing.svg"]
        : [svg ? new XMLSerializer().serializeToString(svg) : "", "image/svg+xml", `${kind === "smiles" ? "molecule" : "diagram"}.svg`];
  const url = URL.createObjectURL(new Blob([body], { type }));
  const link = Object.assign(document.createElement("a"), { href: url, download: name });
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
