import test from 'node:test';
import assert from 'node:assert/strict';
import { markdown } from '../web/markdown.js';
import { loadMath, loadHighlight, parseDelimited, svgImage, languageOf } from '../web/rendered.js';

// The libraries load in the tests that need them, so the first tests see the page as
// it is before any reply has asked for one.

test('math waits for KaTeX as the TeX that was written, and prices stay text', () => {
  const html = markdown('Energy $E = mc^2$, and \\(x\\) where it costs $5 and $10, or $5-$10.');
  assert.match(html, /<span class="math" data-render="math" data-tex="E = mc\^2">\$E = mc\^2\$<\/span>/);
  assert.match(html, /<span class="math" data-render="math" data-tex="x">\\\(x\\\)<\/span>/);
  assert.match(html, /costs \$5 and \$10, or \$5-\$10\./);
  // An escaped bracket in prose is not math.
  assert.match(markdown('See \\[1\\].'), /<p>See \[1\]\.<\/p>/);
  // Math in code stays code.
  assert.match(markdown('Run `echo $HOME $PATH`'), /<code>echo \$HOME \$PATH<\/code>/);
});

test('code waits for the highlighter, and plain text never asks for it', () => {
  const html = markdown('```python\nprint(1)\n```');
  assert.match(html, /<div class="code-block" data-render="code" data-language="python">/);
  assert.match(html, /<code>print\(1\)\n<\/code>/);
  assert.doesNotMatch(markdown('```\nplain\n```'), /data-render/);
  assert.doesNotMatch(markdown('```text\nplain\n```'), /data-render/);
});

test('a diagram is drawn only once its block is closed', () => {
  const open = markdown('Here:\n\n```mermaid\ngraph TD; A-->B');
  assert.doesNotMatch(open, /class="code-block drawn/);
  const closed = markdown('Here:\n\n```mermaid\ngraph TD; A-->B\n```\n\nDone.');
  assert.match(closed, /<div class="code-block drawn show-code" data-kind="mermaid" data-render="mermaid">/);
  assert.match(closed, /Drawing…/);
  assert.match(closed, /<code>graph TD; A--&gt;B\n<\/code>/);
  // Inside a list or a quote too.
  assert.match(markdown('- step\n\n  ```smiles\n  CCO ethanol\n  ```'), /data-kind="smiles"/);
  assert.match(markdown('> ```mermaid\n> graph TD; A-->B\n> ```'), /data-kind="mermaid"/);
  // A block's source can't escape its code.
  const hostile = markdown('```mermaid\n</code></pre><img src=x onerror=alert(1)>\n```');
  assert.doesNotMatch(hostile, /<img src=x/);
});

test('CSV and TSV blocks become tables', () => {
  const html = markdown('```csv\nname,score,note\n"Smith, J",3.5,"said ""hi"""\nLee,4,\n```');
  assert.match(html, /<div class="code-block drawn" data-kind="csv">/);
  assert.match(html, /<th>name<\/th><th class="number">score<\/th><th>note<\/th>/);
  assert.match(html, /<td>Smith, J<\/td><td class="number">3.5<\/td><td>said &quot;hi&quot;<\/td>/);
  assert.match(html, /data-action="download-drawing"/);
  assert.match(markdown('```tsv\na\tb\n1\t2\n```'), /<th class="number">a<\/th><th class="number">b<\/th>/);
  const long = `x\n${Array.from({ length: 250 }, (_, i) => i).join('\n')}`;
  assert.match(markdown(`\`\`\`csv\n${long}\n\`\`\``), /Showing 200 of 250 rows/);
  assert.match(markdown('```csv\nonly a header\n```'), /show-code[\s\S]*Chat shows a table once there is a header row/);
});

test('CSV fields follow RFC 4180 quoting', () => {
  assert.deepEqual(parseDelimited('a,"b,c"\r\n"line\none",""""\n\n'), [['a', 'b,c'], ['line\none', '"']]);
  assert.deepEqual(parseDelimited('a\tb', '\t'), [['a', 'b']]);
});

test('SVG blocks show as an image, so their scripts never run', () => {
  const html = markdown('```svg\n<svg viewBox="0 0 10 10"><script>alert(1)</script><circle r="4"/></svg>\n```');
  assert.match(html, /<img src="data:image\/svg\+xml;charset=utf-8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20viewBox/);
  assert.doesNotMatch(html.replace(/<pre>[\s\S]*<\/pre>/, ''), /<script/);
  assert.equal(svgImage('<div>not svg</div>'), null);
  assert.match(svgImage('<?xml version="1.0"?>\n<svg xmlns="http://www.w3.org/2000/svg"></svg>'), /^<img /);
  assert.match(markdown('```svg\n<p>nope</p>\n```'), /This isn’t an SVG drawing Chat can show/);
});

test('once KaTeX loads, math is drawn as the reply is, including chemistry', async () => {
  await loadMath();
  const inline = markdown('Energy $E = mc^2$.');
  assert.match(inline, /<span class="math"><span class="katex">/);
  assert.doesNotMatch(inline, /data-render/);
  const display = markdown('Before\n\n$$\n\\int_0^1 x^2\\,dx\n$$\n\nAfter');
  assert.match(display, /<div class="math display"><span class="katex-display">/);
  assert.match(markdown('\\[ a^2 + b^2 = c^2 \\]'), /class="math display"/);
  assert.match(markdown('```math\n\\frac{1}{2}\n```'), /<div class="math display">/);
  assert.match(markdown('$\\ce{H2O + CO2 -> H2CO3}$'), /class="katex"/);
  // A display block a reply hasn't closed yet stays text.
  assert.match(markdown('Before\n\n$$\n\\int_0^1 x'), /<p>\$\$<br>\n\\int_0\^1 x<\/p>/);
  // Bad TeX shows as an error in place, not a broken reply.
  assert.match(markdown('$\\frac{1}{$'), /katex-error|class="katex"/);
});

test('once the highlighter loads, code is highlighted, with grammars loaded by name', async () => {
  await loadHighlight(['julia']);
  assert.match(markdown('```python\nreturn 1  # done\n```'), /<code class="hljs"><span class="hljs-keyword">return<\/span> <span class="hljs-number">1<\/span>  <span class="hljs-comment"># done<\/span>/);
  assert.match(markdown('```jl\nfunction f() end\n```'), /<span class="hljs-keyword">function<\/span>/);
  assert.equal(languageOf('slurm'), 'bash');
  assert.equal(languageOf('Dockerfile'), 'dockerfile');
  assert.equal(languageOf('text'), '');
  assert.match(markdown('```sbatch\n#SBATCH -N 1\necho hi\n```'), /<code class="hljs">/);
  // Highlighted code still escapes what it shows.
  assert.doesNotMatch(markdown('```html\n<script>alert(1)</script>\n```'), /<script>/);
});
