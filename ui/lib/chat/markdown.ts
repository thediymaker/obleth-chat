// Replies as HTML. Model output is untrusted: no HTML, no URL schemes but web and
// mail, and no image fetches except images Chat stored itself.

import MarkdownIt from "markdown-it";
import type { Token } from "markdown-it";
import { captionHtml, figureHtml, figureOf, tableKey, tableView } from "@/lib/chat/figure";
import { svgIcon } from "@/lib/chat/icon-svg";
import { mathPlugin } from "@/lib/chat/math";
import { DRAWN, drawn, highlight, languageOf, math } from "@/lib/chat/rendered";

interface Env {
  lines?: string[];
  [key: string]: unknown;
}

const parser = new MarkdownIt({ html: false, breaks: true, linkify: false, typographer: false, maxNesting: 32 });
const escape = parser.utils.escapeHtml;
const upstreamValidateLink = parser.validateLink.bind(parser);
parser.validateLink = (value: string) => {
  // eslint-disable-next-line no-control-regex -- control characters are what it rejects
  if (!upstreamValidateLink(value) || /[\u0000-\u001f\u007f]/.test(value)) return false;
  try {
    return ["http:", "https:", "mailto:"].includes(new URL(value, "https://markdown.invalid/").protocol);
  } catch {
    return false;
  }
};

parser.renderer.rules.link_open = (tokens, index, options, _env, renderer) => {
  const token = tokens[index]!;
  token.attrSet("target", "_blank");
  token.attrSet("rel", "noopener noreferrer");
  return renderer.renderToken(tokens, index, options);
};

// A model-provided image URL must never make an automatic network request. It
// remains a normal link which the reader can choose to open. The exception is an
// image Chat stored itself, served from this origin.
parser.renderer.rules.image = (tokens, index, options, env, renderer) => {
  const token = tokens[index]!;
  const label = escape(renderer.renderInlineAsText(token.children || [], options, env) || "Image");
  const source = String(token.attrGet("src") || "");
  if (/^\/api\/images\/[0-9a-f-]{36}$/.test(source))
    return `<a class="generated-image" href="${source}" target="_blank" rel="noopener noreferrer"><img src="${source}" alt="${label}" loading="lazy"></a>`;
  if (!parser.validateLink(source)) return `<span class="markdown-image-link">${label}</span>`;
  return `<a class="markdown-image-link" href="${escape(source)}" target="_blank" rel="noopener noreferrer">${svgIcon("external")}${label}</a>`;
};

const codeBlock = (code: string, info = "", open = false) => {
  const hint = info.trim().split(/\s+/)[0] || "";
  const language = /^[\w.+-]{1,40}$/.test(hint) ? hint : "text";
  // Highlighted once the highlighter has loaded; until then the block asks for it. A
  // block a reply is still writing stays plain until its fence closes: highlighting it
  // on every update would redo the whole block each time and fill the cache.
  const grammar = languageOf(language),
    lit = open ? null : highlight(code, grammar);
  const waiting = !open && grammar && lit === null ? ` data-render="code" data-language="${escape(grammar)}"` : "";
  return `<div class="code-block"${waiting}><div class="code-label"><span>${escape(language)}</span><span class="row"><button type="button" class="row" data-action="copy-code" aria-label="Copy code">${svgIcon("copy")} Copy</button><button type="button" class="row code-save" data-action="save-code" aria-label="Save code to Library">${svgIcon("bookmark")} Save</button></span></div><pre>${lit === null ? `<code>${escape(code)}</code>` : `<code class="hljs">${lit}</code>`}</pre></div>\n`;
};
// A block is drawn only once its closing fence has arrived, so a diagram a reply is
// still writing shows as code instead of failing to draw on every update.
const closed = (token: Token, env: Env | undefined) => {
  const [start = 0, end = 0] = token.map || [],
    last = env?.lines?.[end - 1];
  return end - 1 > start && last !== undefined && new RegExp(`^[\\s>]*\\${token.markup[0]}{${token.markup.length},}\\s*$`).test(last);
};
parser.renderer.rules.fence = (tokens, index, _options, env: Env | undefined) => {
  const token = tokens[index]!,
    kind = (token.info.trim().split(/\s+/)[0] || "").toLowerCase();
  if (kind === "math" && closed(token, env) && token.content.trim()) return `${math(token.content.trim(), true, "$$", true)}\n`;
  if (Object.hasOwn(DRAWN, kind) && closed(token, env)) return drawn(kind, token.content);
  return codeBlock(token.content, token.info, !closed(token, env));
};
parser.renderer.rules.code_block = (tokens, index) => codeBlock(tokens[index]!.content);
// A table. One that lists things with a number each can also show as a figure
// (lib/chat/figure.ts): the bars, the table and a line to switch between them.
const TABLE_OPEN = '<div class="table-scroll" tabindex="0" role="region" aria-label="Table"><table>\n';
/** The text of each cell of the table that opens at `index`: its heading row, then its rows. */
function tableCells(tokens: Token[], index: number) {
  const head: string[] = [],
    rows: string[][] = [];
  let inHead = false;
  for (let i = index + 1; i < tokens.length && tokens[i]!.type !== "table_close"; i++) {
    const token = tokens[i]!;
    if (token.type === "thead_open") inHead = true;
    else if (token.type === "thead_close") inHead = false;
    else if (token.type === "tr_open" && !inHead) rows.push([]);
    else if (token.type === "inline" && (tokens[i - 1]?.type === "th_open" || tokens[i - 1]?.type === "td_open")) {
      const text = (token.children || []).map((child) => (child.type === "text" || child.type === "code_inline" ? child.content : child.type === "softbreak" || child.type === "hardbreak" ? " " : "")).join("");
      (inHead ? head : rows.at(-1))?.push(text);
    }
  }
  return { head, rows };
}
parser.renderer.rules.table_open = (tokens, index, _options, env: Env | undefined) => {
  const { head, rows } = tableCells(tokens, index);
  const figure = figureOf(head, rows);
  const stack = ((env ??= {}).figures ??= []) as string[];
  if (!figure) {
    stack.push("");
    return TABLE_OPEN;
  }
  const view = tableView(head);
  stack.push(captionHtml(head, figure, view, escape));
  return `<div class="table-figure" data-view="${view}" data-key="${escape(tableKey(head))}">${figureHtml(head, rows, figure, escape)}${TABLE_OPEN}`;
};
parser.renderer.rules.table_close = (_tokens, _index, _options, env: Env | undefined) => {
  const caption = ((env?.figures as string[] | undefined) || []).pop();
  return caption ? `</table></div>${caption}</div>\n` : "</table></div>\n";
};

parser.use(mathPlugin, math);

/** A reply's Markdown as safe HTML. */
export function markdown(text: unknown): string {
  const source = String(text ?? "");
  return parser.render(source, { lines: source.replace(/\r\n?/g, "\n").split("\n") } satisfies Env);
}
