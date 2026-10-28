import test from 'node:test';
import assert from 'node:assert/strict';
import { createAttachments, describe } from '../web/attachments.js';
import { createHistory } from '../web/history.js';

test('an attachment read that finishes after session reset cannot upload into the next account', async () => {
  let release, calls = 0;
  const file = { name: 'private.txt', type: 'text/plain', size: 5, arrayBuffer: () => new Promise(resolve => { release = resolve; }) };
  const attachments = createAttachments({ api: { request: () => { calls++; } }, state: {}, render() {}, toast() {} });
  const pending = attachments.upload({ files: [file], value: '' });
  attachments.clear();
  release(new TextEncoder().encode('hello').buffer);
  await pending;
  assert.equal(calls, 0);
  assert.deepEqual(attachments.current(), []);
});

test('an import file read that finishes after session reset cannot import into the next account', async () => {
  let release, calls = 0;
  const file = { size: 20, text: () => new Promise(resolve => { release = resolve; }) };
  const history = createHistory({ api: { request: () => { calls++; } }, state: {}, render() {}, toast() {} });
  const pending = history.submit({ id: 'history-import-form', elements: { 'history-file': { files: [file] } } });
  history.clear();
  release('{"conversations":[]}');
  await pending;
  assert.equal(calls, 0);
});

test('long pasted text is attached as a file whose chip shows it', async () => {
  const sent = [], toasts = [], modals = [];
  const attachments = createAttachments({
    api: { request: (path, options) => { sent.push(options.body); return { attachment: { id: 'a1', name: options.body.name, size: 9000, kind: 'text' } }; } },
    state: {}, render() {}, toast: m => toasts.push(m), modal: (title, body) => modals.push([title, body]),
  });
  const text = `<b>line</b>\n${'x'.repeat(9000)}`;
  await attachments.addText(text);
  assert.equal(sent.length, 1);
  assert.match(sent[0].name, /^Pasted text \d\d\.\d\d\.\d\d\.txt$/);
  assert.equal(sent[0].mimeType, 'text/plain');
  assert.equal(new TextDecoder().decode(Uint8Array.from(atob(sent[0].dataBase64), c => c.charCodeAt(0))), text);
  assert.match(attachments.chips(), /data-action="open-pasted" data-id="a1"/);
  assert.match(attachments.chips(), /8\.8 KB · 2 lines/);
  attachments.actions['open-pasted']({ dataset: { id: 'a1' } });
  assert.equal(modals.length, 1);
  assert.match(modals[0][1], /<pre class="pasted-text">&lt;b&gt;line&lt;\/b&gt;/);
  attachments.actions['detach-file']({ dataset: { id: 'a1' } });
  assert.deepEqual(attachments.current(), []);
  assert.deepEqual(toasts, []);
});

test('pasted text a conversation could not hold, or pasted mid-reply, is refused out loud', async () => {
  let calls = 0; const toasts = [], state = {};
  const attachments = createAttachments({ api: { request: () => { calls++; } }, state, render() {}, toast: m => toasts.push(m) });
  await attachments.addText('é'.repeat(60 * 1024));
  state.streaming = true;
  await attachments.addText('short but sent while a reply streams');
  assert.equal(calls, 0);
  assert.deepEqual(toasts, ['That text is 120.0 KB, and a conversation can hold 100.0 KB. Paste a shorter part.', 'Paste it again once the reply has finished.']);
});

test('a document chip names its type and parts, and says when Chat searches it', async () => {
  const toasts = [];
  const attachments = createAttachments({
    api: { request: (path, options) => ({ id: 'd1', name: options.body.name, size: 3 * 1024 * 1024, kind: 'document', parts: 120, searched: true }) },
    state: {}, render() {}, toast: m => toasts.push(m),
  });
  const file = { name: 'Thesis.pdf', type: 'application/pdf', size: 3 * 1024 * 1024, arrayBuffer: async () => new Uint8Array(8).buffer };
  await attachments.upload({ files: [file], value: '' });
  assert.deepEqual(toasts, []);
  assert.match(attachments.chips(), /PDF · 120 pages · 3\.0 MB/);
  assert.match(attachments.chips(), /Large document: Chat searches it instead of reading it all\./);
  assert.equal(describe({ name: 'deck.pptx', kind: 'document', size: 2048, parts: 1 }).detail, 'PowerPoint · 1 slide · 2.0 KB');
  assert.equal(describe({ name: 'Budget.XLSX', kind: 'document', size: 2048, parts: 3 }).glyph, 'grid');
  assert.equal(describe({ name: 'memo.docx', kind: 'document', size: 512 }).detail, 'Word · 512 B');
  // Documents over the limit are refused before they are read.
  await attachments.upload({ files: [{ ...file, size: 21 * 1024 * 1024 }], value: '' });
  assert.deepEqual(toasts, ['Documents must be 20 MB or smaller.']);
});
