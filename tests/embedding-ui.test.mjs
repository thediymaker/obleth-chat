import test from 'node:test';
import assert from 'node:assert/strict';
import { createAdmin } from '../web/admin.js';

const defaults = { model: 'qwen3-embedding-8b', queryPrefix: 'Instruct: find\nQuery: ', documentPrefix: '' };
const settings = {
  chosen: { model: null, queryPrefix: null, documentPrefix: null },
  defaults, effective: defaults, models: ['embeddinggemma-2', 'qwen3-embedding-8b'],
  index: { documents: 12, pendingDocuments: 0, savedItems: 3, pendingSavedItems: 0 },
};

async function fixture(values) {
  globalThis.location = { hash: '#settings/admin/system' };
  const requests = [], dialogs = [];
  const admin = createAdmin({
    api: { request: async (path, options = {}) => {
      requests.push({ path, ...options });
      if (path === '/api/admin/retention') return { conversationDays: null, unusedFileDays: null, auditDays: null };
      return options.method === 'PATCH' ? { ...settings, reindexed: true } : settings;
    } },
    state: { user: { role: 'admin' } }, capabilities: { features: { knowledge: true } }, render() {}, close() {}, toast() {},
    modal: (title, body) => dialogs.push({ title, body }),
  });
  await admin.load();
  requests.length = 0;
  const form = { id: 'admin-embedding-form', elements: { namedItem: key => ({ value: values[key] ?? '' }) } };
  return { admin, form, requests, dialogs };
}

test('changing the embedding model re-indexes only after confirmation', async () => {
  const f = await fixture({ 'embedding-model': 'embeddinggemma-2', 'embedding-query': 'task: search result | query: ', 'embedding-document': 'title: none | text: ' });
  await f.admin.submit(f.form);
  assert.equal(f.requests.length, 0);
  assert.match(f.dialogs[0].body, /index all 12 documents and 3 saved items again with embeddinggemma-2/);
  await f.admin.actions['admin-confirm']();
  assert.deepEqual(f.requests[0].body, { model: 'embeddinggemma-2', queryPrefix: 'task: search result | query: ', documentPrefix: 'title: none | text: ' });
});

test('values equal to the defaults follow the defaults without a re-index warning', async () => {
  const f = await fixture({ 'embedding-model': 'qwen3-embedding-8b', 'embedding-query': 'Instruct: find\r\nQuery: ', 'embedding-document': '' });
  await f.admin.submit(f.form);
  assert.equal(f.dialogs.length, 0);
  assert.deepEqual(f.requests[0].body, { model: null, queryPrefix: null, documentPrefix: null });
});

test('the system tab warns that a change indexes everything again', async () => {
  const f = await fixture({});
  assert.match(f.admin.view(), /index every document and saved item again/);
  assert.match(f.admin.view(), /12 documents and 3 saved items are indexed with qwen3-embedding-8b/);
});
