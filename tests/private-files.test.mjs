import test from 'node:test';
import assert from 'node:assert/strict';
import { createAttachments } from '../web/attachments.js';
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
