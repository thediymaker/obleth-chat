import test from 'node:test';
import assert from 'node:assert/strict';
import { createKnowledge } from '../web/knowledge.js';

const view = () => createKnowledge({ api: {}, state: {}, capabilities: { features: { knowledge: true } }, render() {}, modal() {}, close() {}, toast() {}, go() {} });
const reply = { role: 'assistant', content: 'Heat flows [1]. Both agree [1, 2]. Not a source [7].', activity: [{ id: 'knowledge', kind: 'sources', collections: ['Thermo'], queries: ['heat'], items: [
  { n: 1, documentId: 'd1', name: 'Lecture.pdf', page: 4, excerpt: 'Heat flows from hot to cold.' },
  { n: 2, documentId: 'd2', name: 'Notes.md', page: null, section: 'Laws › Second <law>', excerpt: 'Entropy <rises>.' },
  { n: 3, documentId: 'd2', name: 'Notes.md', page: null, excerpt: 'Unused.' },
] }] };

test('citations link to the page they came from, and code is left alone', () => {
  const html = view().cite('<p>Heat flows [1]. Both agree [1, 2]. Not a source [7].</p><pre><code>a[1]</code></pre>', reply);
  assert.match(html, /<a class="cite" href="\/api\/knowledge\/documents\/d1\/file#page=4"[^>]*title="Lecture.pdf, page 4">1<\/a>/);
  assert.match(html, /<a class="cite" href="\/api\/knowledge\/documents\/d2\/file"[^>]*title="Notes.md, Laws › Second &lt;law&gt;">2<\/a>/);
  assert.match(html, /Not a source \[7\]/);
  assert.match(html, /<code>a\[1\]<\/code>/);
  assert.equal(view().cite('<p>x [1]</p>', { role: 'assistant', content: 'x [1]' }), '<p>x [1]</p>');
});

test('sources list cited passages first and escape excerpts', () => {
  const html = view().sources(reply);
  assert.match(html, /Searched Thermo for “heat” · 3 passages from 2 documents/);
  assert.ok(html.indexOf('Unused.') > html.indexOf('Entropy'));
  assert.match(html, /Entropy &lt;rises&gt;\./);
  assert.match(html, /Notes\.md <span class="faint">· Laws › Second &lt;law&gt;<\/span>/);
  assert.match(html, /Open at page/);
  assert.equal(view().sources({ role: 'assistant', content: 'hi' }), '');
});
