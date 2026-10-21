import test from 'node:test';
import assert from 'node:assert/strict';
import { createAdmin } from '../web/admin.js';

function fixture(values) {
  const requests = [], dialogs = [];
  const admin = createAdmin({
    api: { request: async (path, options) => { requests.push({ path, ...options }); return options.body; } },
    state: { user: { role: 'admin' } }, capabilities: {}, render() {}, close() {}, toast() {},
    modal: (title, body) => dialogs.push({ title, body }),
  });
  const form = { id: 'admin-retention-form', elements: { namedItem: key => ({ value: values[key] || '' }) } };
  return { admin, form, requests, dialogs };
}

test('automatic deletion cannot be enabled before explicit confirmation', async () => {
  const f = fixture({ 'retention-conversations': '365', 'retention-audit': '90' });
  await f.admin.submit(f.form);
  assert.equal(f.requests.length, 0);
  assert.match(f.dialogs[0].body, /permanently deletes/);
  await f.admin.actions['admin-confirm']();
  assert.equal(f.requests.length, 1);
  assert.deepEqual(f.requests[0].body, { conversationDays: 365, unusedFileDays: null, auditDays: 90 });
});

test('blank retention fields save keep-forever values without deletion confirmation', async () => {
  const f = fixture({});
  await f.admin.submit(f.form);
  assert.equal(f.dialogs.length, 0);
  assert.deepEqual(f.requests[0].body, { conversationDays: null, unusedFileDays: null, auditDays: null });
});

test('identity reset discards a pending retention confirmation', async () => {
  const f = fixture({ 'retention-files': '30' });
  await f.admin.submit(f.form);
  f.admin.clear();
  await f.admin.actions['admin-confirm']();
  assert.equal(f.requests.length, 0);
});
