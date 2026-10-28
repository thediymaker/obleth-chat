import MarkdownIt from './vendor/markdown-it/markdown-it-15.0.3.js';
import { icon } from './icons.js';
import { mathPlugin } from './math.js';
import { DRAWN, drawn, highlight, languageOf, math } from './rendered.js';

// Keep model output untrusted: no HTML, arbitrary URL schemes, or image fetches.
// The local bundle contains the parser and its dependencies; no CDN is needed.
const parser = new MarkdownIt({ html: false, breaks: true, linkify: false, typographer: false, maxNesting: 32 });
const escape = parser.utils.escapeHtml;
const upstreamValidateLink = parser.validateLink.bind(parser);
parser.validateLink = value => {
  if (!upstreamValidateLink(value) || /[\u0000-\u001f\u007f]/.test(value)) return false;
  try {
    return ['http:', 'https:', 'mailto:'].includes(new URL(value, 'https://markdown.invalid/').protocol);
  } catch {
    return false;
  }
};

parser.renderer.rules.link_open = (tokens, index, options, env, renderer) => {
  const token = tokens[index];
  token.attrSet('target', '_blank');
  token.attrSet('rel', 'noopener noreferrer');
  return renderer.renderToken(tokens, index, options);
};

// A model-provided image URL must never make an automatic network request. It
// remains a normal link which the reader can choose to open. The exception is an
// image Chat stored itself.
parser.renderer.rules.image = (tokens, index, options, env, renderer) => {
  const token = tokens[index];
  const label = escape(renderer.renderInlineAsText(token.children || [], options, env) || 'Image');
  const source = token.attrGet('src') || '';
  // Images the model generated are stored by Chat and served from this origin.
  if (/^\/api\/images\/[0-9a-f-]{36}$/.test(source)) return `<a class="generated-image" href="${source}" target="_blank" rel="noopener noreferrer"><img src="${source}" alt="${label}" loading="lazy"></a>`;
  if (!parser.validateLink(source)) return `<span class="markdown-image-link">${label}</span>`;
  return `<a class="markdown-image-link" href="${escape(source)}" target="_blank" rel="noopener noreferrer">${icon('external')}${label}</a>`;
};

const codeBlock = (code, info = '') => {
  const hint = info.trim().split(/\s+/)[0];
  const language = /^[\w.+-]{1,40}$/.test(hint) ? hint : 'text';
  // Highlighted once the highlighter has loaded; until then the block asks for it.
  const grammar = languageOf(language), lit = highlight(code, grammar);
  const waiting = grammar && lit === null ? ` data-render="code" data-language="${escape(grammar)}"` : '';
  return `<div class="code-block"${waiting}><div class="code-label"><span>${escape(language)}</span><span class="row"><button type="button" class="row" data-action="copy-code" aria-label="Copy code">${icon('copy')} Copy</button><button type="button" class="row code-save" data-action="save-code" aria-label="Save code to Library">${icon('bookmark')} Save</button></span></div><pre>${lit === null ? `<code>${escape(code)}</code>` : `<code class="hljs">${lit}</code>`}</pre></div>\n`;
};
// A block is drawn only once its closing fence has arrived, so a diagram a reply is
// still writing shows as code instead of failing to draw on every update.
const closed = (token, env) => {
  const [start, end] = token.map || [], last = env?.lines?.[end - 1];
  return end - 1 > start && last !== undefined && new RegExp(`^[\\s>]*\\${token.markup[0]}{${token.markup.length},}\\s*$`).test(last);
};
parser.renderer.rules.fence = (tokens, index, options, env) => {
  const token = tokens[index], kind = token.info.trim().split(/\s+/)[0].toLowerCase();
  if (kind === 'math' && closed(token, env) && token.content.trim()) return `${math(token.content.trim(), true, '$$', true)}\n`;
  if (Object.hasOwn(DRAWN, kind) && closed(token, env)) return drawn(kind, token.content);
  return codeBlock(token.content, token.info);
};
parser.renderer.rules.code_block = (tokens, index) => codeBlock(tokens[index].content);
parser.renderer.rules.table_open = () => '<div class="table-scroll" tabindex="0" role="region" aria-label="Table"><table>\n';
parser.renderer.rules.table_close = () => '</table></div>\n';

parser.use(mathPlugin, math);

export function markdown(text) {
  const source = String(text ?? '');
  return parser.render(source, { lines: source.replace(/\r\n?/g, '\n').split('\n') });
}
