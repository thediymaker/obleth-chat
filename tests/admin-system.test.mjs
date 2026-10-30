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

test('the banner card shows the saved banner and saves what the form says', async () => {
  globalThis.location = { hash: '#settings/admin/system' };
  const sent = [];
  let changed;
  const admin = createAdmin({
    api: { request: async (path, options) => {
      if (path === '/api/admin/banner' && options?.method === 'PATCH') { sent.push(options.body); return { ...options.body, updatedAt: '2026-10-29T12:00:00Z' }; }
      if (path === '/api/admin/banner') return { text: 'In development', link: null, tone: 'warning', signedOut: true, dismissible: false, updatedAt: '2026-10-29T00:00:00Z' };
      if (path === '/api/admin/retention') return { conversationDays: null, unusedFileDays: null, auditDays: null };
      return {};
    } },
    state: { user: { role: 'admin' } }, capabilities: { features: {} }, render() {}, close() {}, toast() {}, modal() {},
    onBannerChanged: banner => { changed = banner; },
  });
  await admin.load();
  const view = admin.view();
  assert.match(view, /<h2>Banner<\/h2><span class="pill ">On<\/span>/);
  assert.match(view, /id="banner-text"[^>]*value="In development"/);
  assert.match(view, /<option value="warning" selected>Warning<\/option>/);
  const values = { 'banner-text': 'Maintenance Friday', 'banner-link-label': 'Details', 'banner-link-url': 'https://status.example.edu', 'banner-tone': 'info' };
  const checks = { 'banner-signed-out': false, 'banner-dismissible': true };
  const form = { id: 'admin-banner-form', elements: { namedItem: name => name in values ? { value: values[name] } : name in checks ? { checked: checks[name] } : null } };
  await admin.submit(form);
  assert.deepEqual(sent, [{ text: 'Maintenance Friday', link: { label: 'Details', url: 'https://status.example.edu' }, tone: 'info', signedOut: false, dismissible: true }]);
  assert.equal(changed.text, 'Maintenance Friday');
  values['banner-link-url'] = '';
  await assert.rejects(admin.submit(form), /A link needs both its text and its address/);
  assert.equal(sent.length, 1);
});
