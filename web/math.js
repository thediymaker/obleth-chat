// Math in replies, for markdown-it: $…$ and \(…\) inline, $$…$$ and \[…\] for display,
// on their own lines or inside a paragraph. Dollar signs follow Pandoc's rule so prices
// stay text: no space just inside the dollars, and a closing dollar is never followed
// by a digit. A delimiter that isn't closed yet, as in a reply still being written,
// stays text until it is.

// \[1\] or \(a\) in prose is more often an escaped bracket than math: bracket forms
// need something that reads as math, or a single letter.
const MATHY = /[\\^_=+\-*/<>{}|]|\s|^[A-Za-z]$/;

function inline(state, silent) {
  const src = state.src, start = state.pos;
  let open;
  if (src.startsWith('\\(', start)) open = ['\\(', '\\)', false];
  else if (src.startsWith('\\[', start)) open = ['\\[', '\\]', true];
  else if (src.startsWith('$$', start)) open = ['$$', '$$', true];
  else if (src[start] === '$') open = ['$', '$', false];
  else return false;
  const [left, right, display] = open, from = start + left.length;
  let end = src.indexOf(right, from);
  if (left === '$') {
    if (/\s/.test(src[from] || ' ')) return false;
    while (end !== -1 && (/\s/.test(src[end - 1]) || src[end - 1] === '\\' || /\d/.test(src[end + 1] || ''))) end = src.indexOf('$', end + 1);
  }
  if (end === -1 || end === from || end + right.length > state.posMax) return false;
  const content = src.slice(from, end);
  if (left.startsWith('\\') && !MATHY.test(content.trim())) return false;
  if (!silent) {
    const token = state.push('math_inline', 'math', 0);
    token.content = content;
    token.markup = left;
    token.meta = { display };
  }
  state.pos = end + right.length;
  return true;
}

function block(state, startLine, endLine, silent) {
  if (state.sCount[startLine] - state.blkIndent >= 4) return false;
  const lineText = n => state.src.slice(state.bMarks[n] + state.tShift[n], state.eMarks[n]);
  const first = lineText(startLine);
  const left = first.startsWith('$$') ? '$$' : first.startsWith('\\[') ? '\\[' : null;
  if (!left) return false;
  const right = left === '$$' ? '$$' : '\\]';
  const rest = first.slice(2).trim();
  let last = startLine, body;
  if (rest.endsWith(right) && rest.length >= right.length) {
    body = rest.slice(0, -right.length);
  } else {
    const lines = [rest];
    for (last = startLine + 1; last < endLine; last++) {
      if (state.sCount[last] < state.blkIndent) return false;
      const text = lineText(last).trim();
      if (text.endsWith(right)) { lines.push(text.slice(0, -right.length)); break; }
      lines.push(text);
    }
    if (last >= endLine) return false;
    body = lines.join('\n');
  }
  if (!body.trim()) return false;
  if (silent) return true;
  const token = state.push('math_block', 'math', 0);
  token.block = true;
  token.content = body.trim();
  token.markup = left;
  token.map = [startLine, last + 1];
  state.line = last + 1;
  return true;
}

/** Adds math to a markdown-it parser; `render(tex, display)` returns its HTML. */
export function mathPlugin(md, render) {
  md.inline.ruler.before('escape', 'math_inline', inline);
  md.block.ruler.before('fence', 'math_block', block, { alt: ['paragraph', 'reference', 'blockquote', 'list'] });
  md.renderer.rules.math_inline = (tokens, index) => render(tokens[index].content, tokens[index].meta.display, tokens[index].markup);
  md.renderer.rules.math_block = (tokens, index) => `${render(tokens[index].content, true, tokens[index].markup, true)}\n`;
}
