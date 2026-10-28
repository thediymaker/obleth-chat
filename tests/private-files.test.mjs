import test from 'node:test';
import assert from 'node:assert/strict';
import { createAttachments, describe } from '../web/attachments.js';
import { createHistory } from '../web/history.js';
import { icon } from '../web/manage-ui.js';

test('an upload waiting on the limits after session reset cannot upload into the next account', async () => {
  let release; const requests = [];
  const file = { name: 'private.txt', type: 'text/plain', size: 5 };
  const attachments = createAttachments({ api: { request: (path, options) => { requests.push([path, options]); return new Promise(resolve => { release = resolve; }); } }, state: {}, render() {}, toast() {} });
  const pending = attachments.upload({ files: [file], value: '' });
  assert.equal(requests.length, 1);
  attachments.clear();
  release({ attachments: [], limits: { maxFileBytes: 1024 } });
  await pending;
  assert.equal(requests.length, 1);
  assert.deepEqual(attachments.current(), []);
});

test('an import sends the file unread, and a reply that arrives after session reset shows nothing', async () => {
  let release; const requests = [], modals = [];
  const file = { name: 'conversations.json', type: 'application/json', size: 20 };
  const history = createHistory({ api: { request: (path, options) => { requests.push(options); return new Promise(resolve => { release = resolve; }); } }, state: {}, render() {}, toast() {}, close() {}, refreshSidebar() {}, modal: (title, body) => modals.push([title, body]) });
  const pending = history.submit({ id: 'history-import-form', elements: { 'history-file': { files: [file] } } });
  assert.equal(requests[0].raw, file);
  history.clear();
  release({ source: 'chatgpt', imported: 3 });
  await pending;
  assert.deepEqual(modals, []);
});

test('long pasted text is attached as a file whose chip shows it', async () => {
  const sent = [], toasts = [], modals = [];
  const attachments = createAttachments({
    api: { request: (path, options) => { if (!options) return { attachments: [], limits: { maxFileBytes: 50 * 1024 * 1024 } }; sent.push([path, options]); return { attachment: { id: 'a1', name: options.raw.name, size: 9000, kind: 'text' } }; } },
    state: {}, render() {}, toast: m => toasts.push(m), modal: (title, body) => modals.push([title, body]),
  });
  const text = `<b>line</b>\n${'x'.repeat(9000)}`;
  await attachments.addText(text);
  assert.equal(sent.length, 1);
  // The file's own bytes go up as they are, named in the query.
  const [path, options] = sent[0];
  const query = new URLSearchParams(path.split('?')[1]);
  assert.match(query.get('name'), /^Pasted text \d\d\.\d\d\.\d\d\.txt$/);
  assert.equal(query.get('type'), 'text/plain');
  assert.equal(options.contentType, 'application/octet-stream');
  assert.equal(await options.raw.text(), text);
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
    api: { request: (path, options) => options ? ({ id: 'd1', name: options.raw.name, size: 3 * 1024 * 1024, kind: 'document', parts: 120, searched: true }) : { attachments: [], limits: { maxFileBytes: 20 * 1024 * 1024 } } },
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
  // Files over the server's limit are refused before they are sent.
  await attachments.upload({ files: [{ ...file, size: 21 * 1024 * 1024 }], value: '' });
  assert.deepEqual(toasts, ['Thesis.pdf is 21.0 MB, and files can be at most 20.0 MB.']);
});

test('any file can be attached, and an archive chip says how many files it holds', async () => {
  const sent = [];
  const attachments = createAttachments({
    api: { request: (path, options) => { if (!options) return { attachments: [], limits: { maxFileBytes: 50 * 1024 * 1024 } }; sent.push(path); return { id: 'z1', name: 'site.zip', size: 1.8 * 1024 * 1024, kind: 'file', parts: 14 }; } },
    state: {}, render() {}, toast: m => assert.fail(m),
  });
  assert.doesNotMatch(attachments.toolbar(), /accept=/);
  await attachments.upload({ files: [{ name: 'site.zip', type: 'application/zip', size: 1.8 * 1024 * 1024 }], value: '' });
  assert.equal(sent.length, 1);
  assert.match(attachments.chips(), /ZIP · 14 files · 1\.8 MB/);
  assert.ok(attachments.chips().includes(icon('archive')));
  assert.equal(describe({ name: 'code.tar.gz', kind: 'file', size: 2048, parts: 1 }).detail, 'TAR.GZ · 1 file · 2.0 KB');
  assert.equal(describe({ name: 'code.tar.gz', kind: 'file', size: 2048 }).glyph, 'archive');
  assert.deepEqual(describe({ name: 'results.parquet', kind: 'file', size: 4096 }), { glyph: 'file', detail: 'PARQUET · 4.0 KB', note: '' });
});
