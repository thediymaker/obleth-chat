import { icon } from './icons.js';

// What Chat draws in a reply beyond plain Markdown: math, highlighted code, Mermaid
// diagrams and charts, molecules from SMILES, SVG drawings and CSV tables. Each library
// is a local copy in vendor/ that loads the first time a reply needs it. Until then,
// and while a reply is still writing a block, the block shows as code.

// Libraries markdown() uses directly once they have loaded.
export const loaded = { katex: null, hljs: null };

const escape = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const theme = () => (globalThis.document?.documentElement?.dataset.theme === 'dark' ? 'dark' : 'light');
// Small caches, so a reply drawn again while it streams costs nothing for what is done.
const LIMIT = 300;
const remember = (map, key, value) => { map.delete(key); map.set(key, value); if (map.size > LIMIT) map.delete(map.keys().next().value); return value; };
const maths = new Map(), lights = new Map(), drawings = new Map();
// Blocks the person switched to their code, by kind and source.
const showingCode = new Set();

// Fenced blocks Chat draws, by their language, and what each is called.
export const DRAWN = { mermaid: 'Diagram', smiles: 'Molecule', svg: 'Drawing', csv: 'Table', tsv: 'Table' };
const LONGEST = 50_000;

/* ---------- Math ---------- */

const KATEX_OPTIONS = { throwOnError: false, strict: 'ignore', trust: false, maxSize: 50, maxExpand: 1000 };
/** HTML for math: KaTeX's once it has loaded, else the TeX as written until it has. */
export function math(tex, display, markup = '$', block = false) {
  const tag = block ? 'div' : 'span', cls = `math${display ? ' display' : ''}`;
  if (loaded.katex) {
    const key = `${display ? 1 : 0}${tex}`;
    const html = maths.get(key) ?? remember(maths, key, loaded.katex.renderToString(tex, { ...KATEX_OPTIONS, displayMode: display }));
    return `<${tag} class="${cls}">${html}</${tag}>`;
  }
  const close = { '\\(': '\\)', '\\[': '\\]' }[markup] || markup;
  return `<${tag} class="${cls}" data-render="math" data-tex="${escape(tex)}"${display ? ' data-display' : ''}>${escape(markup + tex + close)}</${tag}>`;
}
let katexLoading;
export function loadMath() {
  katexLoading ||= import('./vendor/katex/katex.mjs').then(async module => {
    await import('./vendor/katex/contrib/mhchem.mjs');
    const document = globalThis.document;
    if (document) {
      const link = document.createElement('link');
      link.rel = 'stylesheet';
      link.href = new URL('./vendor/katex/katex.min.css', import.meta.url).href;
      document.head.append(link);
    }
    loaded.katex = module.default;
  });
  return katexLoading;
}

/* ---------- Code ---------- */

// Languages the common bundle lacks, by the names models use, to the grammar file.
const GRAMMARS = {
  dockerfile: 'dockerfile', docker: 'dockerfile', julia: 'julia', jl: 'julia', 'julia-repl': 'julia-repl', jldoctest: 'julia-repl',
  fortran: 'fortran', f90: 'fortran', f95: 'fortran', matlab: 'matlab', latex: 'latex', tex: 'latex',
  powershell: 'powershell', ps1: 'powershell', pwsh: 'powershell', scala: 'scala', haskell: 'haskell', hs: 'haskell',
  elixir: 'elixir', ex: 'elixir', exs: 'elixir', erlang: 'erlang', erl: 'erlang', nginx: 'nginx', cmake: 'cmake',
  protobuf: 'protobuf', proto: 'protobuf', dart: 'dart', clojure: 'clojure', clj: 'clojure', groovy: 'groovy',
  mathematica: 'mathematica', wolfram: 'mathematica', wl: 'mathematica', verilog: 'verilog', systemverilog: 'verilog', sv: 'verilog',
  vhdl: 'vhdl', x86asm: 'x86asm', asm: 'x86asm', nasm: 'x86asm', llvm: 'llvm', ocaml: 'ocaml', ml: 'ocaml',
  fsharp: 'fsharp', fs: 'fsharp', scheme: 'scheme', lisp: 'lisp', elisp: 'lisp', awk: 'awk', tcl: 'tcl',
  apache: 'apache', apacheconf: 'apache', properties: 'properties', http: 'http', dos: 'dos', bat: 'dos', batch: 'dos', cmd: 'dos',
  stata: 'stata', sas: 'sas', arduino: 'arduino', ino: 'arduino', glsl: 'glsl', gradle: 'gradle', nix: 'nix',
};
// Names models use for languages a bundled grammar already reads.
const ALIASES = { slurm: 'bash', sbatch: 'bash', cuda: 'cpp', cu: 'cpp', jsonc: 'json', json5: 'json', ipython: 'python', cfg: 'ini', conf: 'ini' };
const PLAIN = new Set(['', 'text', 'txt', 'plain', 'plaintext', 'output', 'console-output']);
/** The highlight.js language for a fence's language hint, or '' for none. */
export const languageOf = hint => {
  const name = String(hint || '').toLowerCase();
  return PLAIN.has(name) ? '' : GRAMMARS[name] || ALIASES[name] || name;
};
/** Highlighted HTML for code, or null when the highlighter or the language isn't loaded. */
export function highlight(code, language) {
  const hljs = loaded.hljs;
  if (!hljs || !language || !hljs.getLanguage(language) || code.length > 200_000) return null;
  const key = `${language}\n${code}`;
  return lights.get(key) ?? remember(lights, key, hljs.highlight(code, { language, ignoreIllegals: true }).value);
}
let hljsLoading;
const triedGrammars = new Set();
export async function loadHighlight(languages = []) {
  hljsLoading ||= import('./vendor/highlight/highlight-11.12.0.min.js').then(module => { loaded.hljs = module.default; });
  await hljsLoading;
  for (const language of languages) {
    const file = Object.values(GRAMMARS).includes(language) ? language : null;
    if (!file || loaded.hljs.getLanguage(file) || triedGrammars.has(file)) continue;
    triedGrammars.add(file);
    try { loaded.hljs.registerLanguage(file, (await import(`./vendor/highlight/languages/${file}.min.js`)).default); } catch { /* Shown as plain code. */ }
  }
}

/* ---------- Drawn blocks ---------- */

/** Rows of a CSV or TSV text, reading quoted fields as RFC 4180 does. */
export function parseDelimited(text, separator = ',') {
  const rows = [];
  let row = [], field = '', quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') { field += '"'; i++; }
      else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"' && field === '') quoted = true;
    else if (c === separator) { row.push(field); field = ''; }
    else if (c === '\n' || c === '\r') {
      if (c === '\r' && text[i + 1] === '\n') i++;
      row.push(field); field = '';
      if (row.length > 1 || row[0] !== '') rows.push(row);
      row = [];
    } else field += c;
  }
  row.push(field);
  if (row.length > 1 || row[0] !== '') rows.push(row);
  return rows;
}
const NUMBER = /^[-+]?(\d[\d,]*\.?\d*|\.\d+)([eE][-+]?\d+)?%?$/;
const ROWS = 200, COLUMNS = 40;
function table(source, separator) {
  const rows = parseDelimited(source, separator);
  if (rows.length < 2) return { error: 'Chat shows a table once there is a header row and at least one more.' };
  const [head, ...body] = rows, width = Math.min(COLUMNS, Math.max(...rows.map(r => r.length)));
  const numeric = Array.from({ length: width }, (_, c) => body.every(r => !r[c]?.trim() || NUMBER.test(r[c].trim())));
  const cell = (tag, value, c) => `<${tag}${numeric[c] ? ' class="number"' : ''}>${escape(value ?? '')}</${tag}>`;
  const shown = body.slice(0, ROWS);
  const note = body.length > ROWS || rows.some(r => r.length > COLUMNS)
    ? `<p class="drawing-note">Showing ${Math.min(body.length, ROWS).toLocaleString()} of ${body.length.toLocaleString()} rows${rows.some(r => r.length > COLUMNS) ? ` and the first ${COLUMNS} columns` : ''}. Download it for the rest.</p>` : '';
  return { html: `<div class="table-scroll" tabindex="0" role="region" aria-label="Table"><table><thead><tr>${Array.from({ length: width }, (_, c) => cell('th', head[c], c)).join('')}</tr></thead><tbody>${shown.map(r => `<tr>${Array.from({ length: width }, (_, c) => cell('td', r[c], c)).join('')}</tr>`).join('')}</tbody></table></div>${note}` };
}
/** An SVG as an image, so its scripts never run and it loads nothing; null if it isn't one. */
export function svgImage(source) {
  const svg = source.trim();
  if (svg.length > 200_000 || !/^(<\?xml[^>]*>\s*)?(<!--[\s\S]*?-->\s*)*(<!DOCTYPE[^>]*>\s*)?<svg[\s>]/i.test(svg)) return null;
  const named = /<svg[^>]*\sxmlns\s*=/i.test(svg) ? svg : svg.replace(/<svg/i, '<svg xmlns="http://www.w3.org/2000/svg"');
  return `<img src="data:image/svg+xml;charset=utf-8,${encodeURIComponent(named)}" alt="Drawing">`;
}
const keyOf = (kind, source) => `${kind}\n${['mermaid', 'smiles'].includes(kind) ? theme() : ''}\n${source}`;
// What a block of each kind becomes: { html } or { error }. Mermaid and molecules
// load a library and draw in the page, so they come from the cache or are drawn later.
function drawingOf(kind, source) {
  if (source.length > LONGEST) return { error: `Chat draws blocks of up to ${LONGEST.toLocaleString()} characters.` };
  if (kind === 'svg') { const html = svgImage(source); return html ? { html } : { error: 'This isn’t an SVG drawing Chat can show.' }; }
  if (kind === 'csv' || kind === 'tsv') {
    const key = keyOf(kind, source);
    return drawings.get(key) ?? remember(drawings, key, table(source, kind === 'tsv' ? '\t' : ','));
  }
  return drawings.get(keyOf(kind, source)) || null;
}
/** HTML for a finished fenced block Chat draws: the drawing, or its code until it's drawn. */
export function drawn(kind, source) {
  const drawing = drawingOf(kind, source), name = DRAWN[kind];
  const code = !drawing?.html || showingCode.has(`${kind}\n${source}`);
  const toggle = drawing?.html ? `<button type="button" class="row" data-action="toggle-drawing" aria-pressed="${code}">${icon(code ? 'image' : 'code')} ${code ? name : 'Code'}</button>` : '';
  const download = drawing?.html && (kind !== 'smiles' || !source.trim().includes('\n')) ? `<button type="button" class="row" data-action="download-drawing" aria-label="Download ${name.toLowerCase()}">${icon('download')} Download</button>` : '';
  const status = drawing ? '' : '<span class="drawing-status" role="status">Drawing…</span>';
  return `<div class="code-block drawn${code ? ' show-code' : ''}" data-kind="${kind}"${drawing ? '' : ` data-render="${kind}"`}><div class="code-label"><span>${kind}</span>${status}<span class="row">${toggle}${download}<button type="button" class="row" data-action="copy-code" aria-label="Copy code">${icon('copy')} Copy</button><button type="button" class="row code-save" data-action="save-code" aria-label="Save code to Library">${icon('bookmark')} Save</button></span></div>${drawing?.html ? `<div class="drawing drawing-${kind}">${drawing.html}</div>` : ''}${drawing?.error ? `<p class="drawing-note">${escape(drawing.error)}</p>` : ''}<pre><code>${escape(source)}</code></pre></div>\n`;
}

let mermaidLoading, queue = Promise.resolve(), serial = 0;
const firstLine = error => String(error?.message || error || 'unknown error').split('\n').find(l => l.trim())?.trim().slice(0, 200);
// Mermaid draws one diagram at a time in the page, so diagrams take turns.
function drawMermaid(source) {
  const run = async () => {
    mermaidLoading ||= import('./vendor/mermaid/mermaid.esm.min.mjs').then(module => module.default);
    const mermaid = await mermaidLoading, id = `drawing-${++serial}`;
    mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: theme() === 'dark' ? 'dark' : 'default', htmlLabels: false, flowchart: { htmlLabels: false }, fontFamily: getComputedStyle(document.body).fontFamily, maxTextSize: LONGEST });
    try {
      return { html: (await mermaid.render(id, source)).svg };
    } catch (error) {
      return { error: `Couldn’t draw this diagram: ${firstLine(error)}` };
    } finally {
      document.getElementById(id)?.remove(); document.getElementById(`d${id}`)?.remove();
    }
  };
  const result = queue.then(run, run);
  queue = result.catch(() => {});
  return result;
}
let smilesLoading;
// One molecule per line: a SMILES string, then an optional name.
async function drawSmiles(source) {
  smilesLoading ||= import('./vendor/smiles-drawer/smiles-drawer-2.4.1.min.mjs').then(module => module.default);
  const SmilesDrawer = await smilesLoading;
  const lines = source.split('\n').map(l => l.trim()).filter(Boolean);
  if (!lines.length) return { error: 'There is no SMILES to draw.' };
  if (lines.length > 12) return { error: 'Chat draws up to 12 molecules in one block.' };
  // The drawer measures text, so it draws into the page, out of sight.
  const stage = document.createElement('div');
  stage.className = 'drawing-stage';
  document.body.append(stage);
  try {
    const figures = [];
    for (const line of lines) {
      const [smiles, ...name] = line.split(/\s+/);
      const tree = await new Promise((resolve, reject) => SmilesDrawer.parse(smiles, resolve, reject));
      const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      stage.append(svg);
      new SmilesDrawer.SvgDrawer({ width: 280, height: 220 }).draw(tree, svg, theme());
      figures.push(`<figure>${svg.outerHTML}<figcaption>${escape(name.join(' ') || smiles)}</figcaption></figure>`);
    }
    return { html: figures.join('') };
  } catch (error) {
    return { error: `Couldn’t draw this molecule: ${firstLine(error)}` };
  } finally {
    stage.remove();
  }
}

/* ---------- In the page ---------- */

const pending = new Map();
async function prepare(el) {
  const kind = el.dataset.render;
  if (kind === 'math') return loadMath();
  if (kind === 'code') return loadHighlight([el.dataset.language]);
  const source = el.querySelector('code')?.textContent || '', key = keyOf(kind, source);
  if (drawings.has(key)) return;
  if (!pending.has(key)) pending.set(key, (kind === 'mermaid' ? drawMermaid(source) : drawSmiles(source)).then(result => { remember(drawings, key, result); pending.delete(key); }));
  return pending.get(key);
}
function fill(el) {
  const kind = el.dataset.render;
  if (kind === 'math') {
    if (!loaded.katex) return;
    el.outerHTML = math(el.dataset.tex, 'display' in el.dataset, '$', el.tagName === 'DIV');
  } else if (kind === 'code') {
    const code = el.querySelector('code'), html = code && highlight(code.textContent, el.dataset.language);
    if (html !== null && code) { code.innerHTML = html; code.classList.add('hljs'); }
    el.removeAttribute('data-render');
  } else {
    const source = el.querySelector('code')?.textContent || '';
    if (drawings.has(keyOf(kind, source))) el.outerHTML = drawn(kind, source);
  }
}
let running = false, again = false;
async function hydrate(root) {
  if (running) { again = true; return; }
  running = true;
  try {
    do {
      again = false;
      await Promise.all([...root.querySelectorAll('[data-render]')].map(el => prepare(el).catch(() => {})));
      root.querySelectorAll('[data-render]').forEach(fill);
    } while (again);
  } finally {
    running = false;
  }
}
/** Draws what replies in `root` need, now and whenever more appears. */
export function watchDrawings(root) {
  let queued = false;
  const soon = () => { if (!queued) { queued = true; requestAnimationFrame(() => { queued = false; hydrate(root); }); } };
  new MutationObserver(records => {
    if (records.some(r => [...r.addedNodes].some(n => n.nodeType === 1 && (n.matches('[data-render]') || n.querySelector('[data-render]'))))) soon();
  }).observe(root, { childList: true, subtree: true });
  // Diagrams and molecules follow the theme: draw them again in its colors.
  window.addEventListener('obleth-theme-change', () => {
    root.querySelectorAll('.code-block.drawn[data-kind="mermaid"], .code-block.drawn[data-kind="smiles"]').forEach(block => { block.outerHTML = drawn(block.dataset.kind, block.querySelector('code')?.textContent || ''); });
    soon();
  });
  soon();
}
const sourceOf = block => block.querySelector('code')?.textContent || '';
/** Switches a drawn block between its drawing and its code, and keeps the choice. */
export function toggleDrawing(el) {
  const block = el.closest('.code-block.drawn'), kind = block.dataset.kind, key = `${kind}\n${sourceOf(block)}`;
  if (showingCode.has(key)) showingCode.delete(key); else showingCode.add(key);
  block.outerHTML = drawn(kind, sourceOf(block));
}
/** Saves a drawn block as a file: the SVG of a drawing, or the table's data. */
export function downloadDrawing(el) {
  const block = el.closest('.code-block.drawn'), kind = block.dataset.kind, source = sourceOf(block);
  const svg = block.querySelector('.drawing svg');
  const [body, type, name] = kind === 'csv' || kind === 'tsv' ? [source, `text/${kind === 'csv' ? 'csv' : 'tab-separated-values'}`, `table.${kind}`]
    : kind === 'svg' ? [decodeURIComponent(block.querySelector('.drawing img').src.split(',').slice(1).join(',')), 'image/svg+xml', 'drawing.svg']
    : [new XMLSerializer().serializeToString(svg), 'image/svg+xml', `${kind === 'smiles' ? 'molecule' : 'diagram'}.svg`];
  const url = URL.createObjectURL(new Blob([body], { type }));
  const link = Object.assign(document.createElement('a'), { href: url, download: name });
  document.body.append(link); link.click(); link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
