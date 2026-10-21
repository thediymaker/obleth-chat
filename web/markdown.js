import MarkdownIt from './vendor/markdown-it/markdown-it-15.0.3.js';
import { icon } from './icons.js';

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
  return `<div class="code-block"><div class="code-label"><span>${escape(language)}</span><span class="row"><button type="button" class="row" data-action="copy-code" aria-label="Copy code">${icon('copy')} Copy</button><button type="button" class="row code-save" data-action="save-code" aria-label="Save code to Library">${icon('bookmark')} Save</button></span></div><pre><code>${escape(code)}</code></pre></div>\n`;
};
parser.renderer.rules.fence = (tokens, index) => codeBlock(tokens[index].content, tokens[index].info);
parser.renderer.rules.code_block = (tokens, index) => codeBlock(tokens[index].content);
parser.renderer.rules.table_open = () => '<div class="table-scroll" tabindex="0" role="region" aria-label="Table"><table>\n';
parser.renderer.rules.table_close = () => '</table></div>\n';

export function markdown(text) {
  return parser.render(String(text ?? ''));
}
