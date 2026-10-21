import test from 'node:test';
import assert from 'node:assert/strict';
import { canonicalSettingsHash, adminLocation, settingsTabs } from '../web/settings-navigation.js';
import { createAdmin } from '../web/admin.js';

test('legacy infrastructure and administration links resolve inside Settings', () => {
  assert.equal(canonicalSettingsHash('#system'), '#settings/infrastructure');
  assert.equal(canonicalSettingsHash('#admin/models'), '#settings/admin/models');
  assert.equal(canonicalSettingsHash('#chat/abc'), '#chat/abc');
  assert.deepEqual(adminLocation('#settings/admin/groups/group-123'), { section: 'groups', groupId: 'group-123' });
  assert.deepEqual(adminLocation('#admin/groups/group-123'), { section: 'groups', groupId: 'group-123' });
});

test('ordinary Settings navigation omits Administration while retaining infrastructure', () => {
  assert.doesNotMatch(settingsTabs('personal'), /Administration|settings\/admin/);
  assert.match(settingsTabs('infrastructure'), /href="#settings\/infrastructure" aria-current="page"/);
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
