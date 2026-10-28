import test from 'node:test';
import assert from 'node:assert/strict';
import { createHistory, importSummary } from '../web/history.js';

test('an import summary says what came over and what was left out, in plain words', () => {
  assert.deepEqual(importSummary({ source: 'chatgpt', imported: 1204, alternativesLeftOut: 37 }), ['Imported 1,204 chats from ChatGPT.', '37 alternative replies or edits were left out.']);
  assert.deepEqual(importSummary({ source: 'open-webui', imported: 1, alreadyImported: 2, overLimit: 1, messagesLeftOut: 30, empty: 1 }), [
    'Imported 1 chat from Open WebUI.',
    '2 chats were already here.',
    '1 older chat was left out because you can keep up to 1,000 chats. Delete some and import the file again to add them.',
    'The earliest 30 messages of very long chats were left out.',
    '1 empty chat was skipped.',
  ]);
  assert.deepEqual(importSummary({ source: 'claude', imported: 0, alreadyImported: 5 }), ['Nothing new to import from Claude. All 5 chats are already here.']);
  assert.deepEqual(importSummary({ source: 'obleth-chat', imported: 2 }), ['Imported 2 chats.']);
  assert.deepEqual(importSummary({ source: 'chatgpt', imported: 100, failed: 20, error: 'The service could not complete this request' }), ['Imported 100 chats from ChatGPT.', '20 chats could not be imported: The service could not complete this request. Import the same file again to add them.']);
});

function fixture(response) {
  const requests = [], modals = [];
  const history = createHistory({
    api: { request: async (path, options) => { requests.push({ path, ...options }); return response; } },
    state: {}, render() {}, toast() {}, close() {}, refreshSidebar() {}, modal: (title, body) => modals.push({ title, body }),
  });
  return { history, requests, modals };
}
const form = file => ({ id: 'history-import-form', elements: { 'history-file': { files: [file] } } });

test('the chosen export goes to Chat as it is, and the summary follows', async () => {
  const f = fixture({ source: 'chatgpt', imported: 2 });
  const file = { name: 'chatgpt-export.zip', type: 'application/zip', size: 1024 };
  await f.history.submit(form(file));
  assert.equal(f.requests[0].path, '/api/conversations/import');
  assert.equal(f.requests[0].raw, file);
  assert.equal(f.requests[0].contentType, 'application/zip');
  assert.equal(f.modals.at(-1).title, 'Import finished');
  assert.match(f.modals.at(-1).body, /Imported 2 chats from ChatGPT\./);
});

test('an export over 200 MB is refused before upload, with a hint for zips', async () => {
  const f = fixture({});
  await assert.rejects(f.history.submit(form({ name: 'export.zip', size: 300 * 1024 * 1024 })), /Unzip it and choose the conversations\.json/);
  await assert.rejects(f.history.submit(form({ name: 'chats.json', size: 300 * 1024 * 1024 })), /200 MB or less/);
  assert.equal(f.requests.length, 0);
});

test('the import dialog names the apps it reads', () => {
  const f = fixture({});
  f.history.actions['history-import']();
  assert.match(f.modals[0].body, /Open WebUI, ChatGPT or Claude/);
  assert.match(f.modals[0].body, /accept="\.json,\.zip/);
});
