import test from 'node:test';
import assert from 'node:assert/strict';
import { markdown } from '../web/markdown.js';

test('renders headings, emphasis, nested lists, blockquotes, and inline code', () => {
  const html = markdown('# Heading\n\nA **strong** and *emphasized* ~~old~~ answer with `x < 2`.\n\n1. First\n   - Nested\n     - Deeper\n2. Second\n\n> Quoted\n> answer');
  assert.match(html, /<h1>Heading<\/h1>/);
  assert.match(html, /<strong>strong<\/strong>/);
  assert.match(html, /<em>emphasized<\/em>/);
  assert.match(html, /<s>old<\/s>/);
  assert.match(html, /<code>x &lt; 2<\/code>/);
  assert.equal((html.match(/<ul>/g) || []).length, 2);
  assert.match(html, /<ol>[\s\S]*<li>First[\s\S]*<li>Second<\/li>[\s\S]*<\/ol>/);
  assert.match(html, /<blockquote>\n<p>Quoted<br>\nanswer<\/p>\n<\/blockquote>/);
});

test('renders tables inside an accessible scroll container', () => {
  const html = markdown('| Name | Status |\n| :--- | ---: |\n| **Obleth** | ready |');
  assert.match(html, /<div class="table-scroll" tabindex="0" role="region" aria-label="Table"><table>/);
  assert.match(html, /<th style="text-align:left">Name<\/th>/);
  assert.match(html, /<td style="text-align:right">ready<\/td>/);
  assert.match(html, /<strong>Obleth<\/strong>/);
  assert.match(html, /<\/table><\/div>/);
});

test('fenced and indented code preserves code and exposes copy controls', () => {
  const source = '```js title="example"\nconst code = "<script>alert(1)</script>";\n\n```\n\n    print("ok")\n';
  const html = markdown(source);
  assert.equal((html.match(/data-action="copy-code"/g) || []).length, 2);
  assert.match(html, /<span>js<\/span>/);
  assert.match(html, /<code>const code = &quot;&lt;script&gt;alert\(1\)&lt;\/script&gt;&quot;;\n\n<\/code>/);
  assert.match(html, /<code>print\(&quot;ok&quot;\)\n<\/code>/);
  assert.doesNotMatch(html, /<script|title="example"/);
  const maliciousFence = markdown('```"><img src=x onerror=alert(1)>\nsecret\n```');
  assert.match(maliciousFence, /<span>text<\/span>/);
  assert.doesNotMatch(maliciousFence, /<img|onerror/);
});

test('renders safe explicit, reference, and autolinks without opener access', () => {
  const html = markdown('[Docs](https://example.com/docs?a=1&b=2 "Documentation") [Mail](mailto:test@example.com) [Local](/guide) [Reference][ref] <https://example.com>\n\n[ref]: https://example.org');
  assert.equal((html.match(/<a /g) || []).length, 5);
  assert.equal((html.match(/rel="noopener noreferrer"/g) || []).length, 5);
  assert.equal((html.match(/target="_blank"/g) || []).length, 5);
  assert.match(html, /href="https:\/\/example.com\/docs\?a=1&amp;b=2"/);
  assert.match(html, /href="mailto:test@example.com"/);
  assert.match(html, /href="\/guide"/);
});

test('raw HTML and user-authored controls remain escaped text', () => {
  const html = markdown('<script>alert(document.cookie)</script>\n\n<img src=x onerror="alert(1)">\n\n<iframe src="https://tracker.test"></iframe>\n\n<button data-action="delete-chat">Delete</button>\n\n<svg onload=alert(1)></svg>');
  assert.doesNotMatch(html, /<(?:script|img|iframe|button|svg)\b/i);
  assert.match(html, /&lt;script&gt;alert\(document.cookie\)&lt;\/script&gt;/);
  assert.match(html, /&lt;button data-action=&quot;delete-chat&quot;&gt;Delete/);
});

test('rejects executable, local file, data, and obfuscated URL schemes', () => {
  for (const source of [
    '[attack](javascript:alert(1))',
    '[attack](JAVASCRIPT:alert(1))',
    '[attack](jav&#x61;script:alert(1))',
    '[attack](javascript&#x3a;alert(1))',
    '[attack](vbscript:alert(1))',
    '[attack](data:text/html;base64,PHNjcmlwdD4=)',
    '[attack](file:///etc/passwd)',
    '[attack](blob:https://example.com/identifier)',
    '[attack](tel:123)',
    '[attack](java\tscript:alert(1))',
    '![attack](data:image/svg+xml;base64,PHN2Zz4=)',
    '<javascript:alert(1)>',
  ]) {
    assert.doesNotMatch(markdown(source), /<(?:a|img)\b/, source);
  }
});

test('images become explicit links without automatic remote requests or HTML in alt text', () => {
  const html = markdown('![A **diagram** with `<img>`](https://tracker.test/pixel?user=abc "private") ![<script>alert(1)</script>](https://example.com/image.png)');
  assert.doesNotMatch(html, /<(?:img|script)\b|\ssrc=/i);
  assert.equal((html.match(/class="markdown-image-link"/g) || []).length, 2);
  assert.match(html, /href="https:\/\/tracker.test\/pixel\?user=abc"/);
  assert.match(html, /A diagram with &lt;img&gt;/);
  assert.match(html, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
});

test('link titles cannot inject attributes or clickable controls', () => {
  const html = markdown('[link](https://example.com "&quot; onclick=&quot;alert(1)")');
  assert.match(html, /title="&quot; onclick=&quot;alert\(1\)"/);
  assert.doesNotMatch(html, /" onclick="|data-action=/);
});

test('every streamed prefix renders safely and incomplete fences remain readable', () => {
  const source = '### A useful answer\n\n- **Careful** work\n\n[link](https://example.com)\n\n```html\n<script>alert("hi")</script>\n```\n\n![diagram](https://example.com/a.svg)';
  for (let length = 0; length <= source.length; length++) {
    const html = markdown(source.slice(0, length));
    assert.doesNotMatch(html, /<(?:script|img|iframe)\b/i, `prefix ${length}`);
    assert.equal((html.match(/<pre>/g) || []).length, (html.match(/<\/pre>/g) || []).length);
    assert.equal((html.match(/<code>/g) || []).length, (html.match(/<\/code>/g) || []).length);
  }
  assert.match(markdown('```python\nprint("still streaming")'), /<code>print\(&quot;still streaming&quot;\)<\/code>/);
  assert.match(markdown('**still streaming'), /\*\*still streaming/);
  assert.match(markdown('**still streaming**'), /<strong>still streaming<\/strong>/);
});

test('empty content and repeated renders do not leak parser state', () => {
  assert.equal(markdown(null), '');
  assert.equal(markdown(undefined), '');
  assert.equal(markdown(''), '');
  markdown('[reference]: https://example.com');
  assert.equal(markdown('[reference]'), '<p>[reference]</p>\n');
  assert.equal(markdown('first\nsecond'), '<p>first<br>\nsecond</p>\n');
});

test('images Chat stored are shown; look-alike paths stay links', () => {
  const html = markdown('![A moth <b>](/api/images/0b0c3a52-5f0e-4c6e-9d43-8c1e0f2a9b11) ![x](/api/images/../admin) ![y](https://example.com/api/images/0b0c3a52-5f0e-4c6e-9d43-8c1e0f2a9b11)');
  assert.match(html, /<a class="generated-image" href="\/api\/images\/0b0c3a52-5f0e-4c6e-9d43-8c1e0f2a9b11"[^>]*><img src="\/api\/images\/0b0c3a52-5f0e-4c6e-9d43-8c1e0f2a9b11" alt="A moth &lt;b&gt;" loading="lazy"><\/a>/);
  assert.equal((html.match(/<img /g) || []).length, 1);
});
