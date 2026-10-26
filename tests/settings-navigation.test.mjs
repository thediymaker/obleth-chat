import test from 'node:test';
import assert from 'node:assert/strict';
import { canonicalSettingsHash, adminLocation, settingsTabs } from '../web/settings-navigation.js';
import { createAdmin } from '../web/admin.js';

test('legacy infrastructure and administration links resolve inside Settings', () => {
  assert.equal(canonicalSettingsHash('#system'), '#settings');
  assert.equal(canonicalSettingsHash('#settings/infrastructure'), '#settings');
  assert.equal(canonicalSettingsHash('#admin/models'), '#settings/admin/models');
  assert.equal(canonicalSettingsHash('#chat/abc'), '#chat/abc');
  assert.deepEqual(adminLocation('#settings/admin/groups/group-123'), { section: 'groups', groupId: 'group-123' });
  assert.deepEqual(adminLocation('#admin/groups/group-123'), { section: 'groups', groupId: 'group-123' });
});

test('ordinary Settings navigation omits Administration and infrastructure', () => {
  assert.doesNotMatch(settingsTabs('personal'), /Administration|settings\/admin|Infrastructure/);
  assert.match(settingsTabs('admin', true), /href="#settings\/admin\/users" aria-current="page"/);
  assert.doesNotMatch(settingsTabs('personal'), /settings\/mcp/);
  assert.match(settingsTabs('connections', false, true), /href="#settings\/mcp" aria-current="page"><svg[^]*MCP servers/);
});

test('nested administration deep links remain denied without issuing privileged API reads', async () => {
  const old = globalThis.location, calls = [];
  globalThis.location = { hash: '#settings/admin/models' };
  try {
    const admin = createAdmin({ api: { request: path => { calls.push(path); return {}; } }, state: { user: { role: 'user' } }, capabilities: {}, render() {} });
    await admin.load();
    assert.match(admin.view(), /Administrator access required/);
    assert.deepEqual(calls, []);
  } finally { if (old === undefined) delete globalThis.location; else globalThis.location = old; }
});

test('a token refused for an old sign-in offers to sign in again', async () => {
  const { ApiError } = await import('../web/client.js');
  const { createSettings } = await import('../web/settings.js');
  const values = { 'token-name': { value: 'Laptop' }, 'token-days': { value: '7' }, 'token-read-only': { checked: false } };
  const form = { id: 'personal-token-form', elements: { namedItem: name => values[name] } };
  const shown = [];
  const settings = response => createSettings({ api: { request: async (path, options) => { if (options?.method === 'POST') return response(); return { sessions: [], tokens: [] }; } }, state: { user: { username: 'ana' } }, render() {}, modal: (title, body) => shown.push({ title, body }), close() {}, toast() {} });
  await settings(() => { throw new ApiError('Sign in again to create a token', 403); }).submit(form);
  assert.equal(shown.at(-1).title, 'Sign in again');
  assert.match(shown.at(-1).body, /data-action="logout"/);
  assert.match(shown.at(-1).body, /Sign in again to create a token\./);
  await assert.rejects(settings(() => { throw new ApiError('A token lasts 1, 7 or 30 days', 400); }).submit(form), /A token lasts/);
  const old = globalThis.location;
  globalThis.location = { origin: 'https://chat.example.com', hash: '#settings' };
  try {
    await settings(() => ({ token: 'obc_secret' })).submit(form);
    assert.equal(shown.at(-1).title, 'Copy your token now');
  } finally { if (old === undefined) delete globalThis.location; else globalThis.location = old; }
});
