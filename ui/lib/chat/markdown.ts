// Replies as HTML. Model output is untrusted: no HTML, no URL schemes but web and
// mail, and no image fetches except images Chat stored itself.

import MarkdownIt from "markdown-it";
import type { Token } from "markdown-it";
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

const codeBlock = (code: string, info = "") => {
  const hint = info.trim().split(/\s+/)[0] || "";
  const language = /^[\w.+-]{1,40}$/.test(hint) ? hint : "text";
  // Highlighted once the highlighter has loaded; until then the block asks for it.
  const grammar = languageOf(language),
    lit = highlight(code, grammar);
  const waiting = grammar && lit === null ? ` data-render="code" data-language="${escape(grammar)}"` : "";
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
  return codeBlock(token.content, token.info);
};
parser.renderer.rules.code_block = (tokens, index) => codeBlock(tokens[index]!.content);
parser.renderer.rules.table_open = () => '<div class="table-scroll" tabindex="0" role="region" aria-label="Table"><table>\n';
parser.renderer.rules.table_close = () => "</table></div>\n";

parser.use(mathPlugin, math);

/** A reply's Markdown as safe HTML. */
export function markdown(text: unknown): string {
  const source = String(text ?? "");
  return parser.render(source, { lines: source.replace(/\r\n?/g, "\n").split("\n") } satisfies Env);
}
