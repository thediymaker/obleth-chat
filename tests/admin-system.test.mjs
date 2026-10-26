import test from 'node:test';
import assert from 'node:assert/strict';
import { createAdmin } from '../web/admin.js';

async function systemTab(health) {
  globalThis.location = { hash: '#settings/admin/system' };
  const admin = createAdmin({
    api: { request: async path => {
      if (path === '/api/admin/retention') return { conversationDays: null, unusedFileDays: null, auditDays: null };
      if (path === '/api/admin/health') return health;
      return {};
    } },
    state: { user: { role: 'admin' } }, capabilities: { features: {} }, render() {}, close() {}, toast() {}, modal() {},
  });
  await admin.load();
  return admin.view();
}

test('the system tab says where workspaces run', async () => {
  const view = await systemTab({ workspaces: { enabled: true, namespace: 'obleth-chat', browser: true, off: null } });
  assert.match(view, /On, in namespace obleth-chat, with the browser/);
});

test('the system tab says why workspaces are off', async () => {
  const view = await systemTab({ workspaces: { enabled: false, namespace: null, browser: false, off: 'Turned off with OBLETH_CHAT_WORKSPACES=off' } });
  assert.match(view, /Off: Turned off with OBLETH_CHAT_WORKSPACES=off/);
});
