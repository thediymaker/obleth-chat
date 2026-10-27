import test from 'node:test';
import assert from 'node:assert/strict';
import { createAdmin } from '../web/admin.js';

const saved = { endpoint: 'gateway', url: null, hasKey: false, keyUnreadable: false, model: null, checkRuns: true, checkTree: true, checkReports: false, canStoreKey: true, gateway: true, models: ['judge-31b'] };

async function fixture(decisions = saved, answer = { ok: true, model: 'judge-31b', yes: 0.98, confidence: 0.9, ms: 412 }) {
  globalThis.location = { hash: '#settings/admin/system' };
  const requests = [];
  const admin = createAdmin({
    api: { request: async (path, options = {}) => {
      requests.push({ path, ...options });
      if (path === '/api/admin/retention') return { conversationDays: null, unusedFileDays: null, auditDays: null };
      if (path === '/api/admin/decisions/check') return answer;
      if (path === '/api/admin/decisions') return options.method === 'PATCH' ? { ...decisions, ...options.body } : decisions;
      return null;
    } },
    state: { user: { role: 'admin' } }, capabilities: { features: {} }, render() {}, close() {}, toast() {}, modal() {},
  });
  await admin.load();
  requests.length = 0;
  return { admin, requests };
}
const form = (values, checked = {}) => ({ id: 'admin-decisions-form', elements: { namedItem: key => ({ value: values[key] ?? '', checked: !!checked[key] }) } });

test('the system tab says where decisions go and whose usage they count toward', async () => {
  const { admin } = await fixture();
  const view = admin.view();
  assert.match(view, /<h2>Decisions<\/h2>/);
  assert.match(view, /<option value="gateway" selected>This gateway<\/option>/);
  assert.match(view, /count toward that person’s usage/);
  assert.match(view, /<div class="decisions-custom" hidden>/);
  assert.match(view, /<option value="judge-31b"><\/option>/);
  assert.match(view, /name="decisions-check-tree"[^>]*checked/);
  assert.doesNotMatch(view, /name="decisions-check-reports"[^>]*checked/);
});

test('saving the gateway sends no URL or key', async () => {
  const { admin, requests } = await fixture();
  await admin.submit(form({ 'decisions-endpoint': 'gateway', 'decisions-url': 'https://left.over', 'decisions-key': 'sk-old' }, { 'decisions-check-runs': true, 'decisions-check-reports': true }));
  assert.deepEqual(requests[0], { path: '/api/admin/decisions', method: 'PATCH', body: { endpoint: 'gateway', model: null, checkRuns: true, checkTree: false, checkReports: true } });
});

test('another endpoint sends its URL, and a blank key keeps the saved one', async () => {
  const { admin, requests } = await fixture({ ...saved, endpoint: 'custom', url: 'https://d.example.com/v1/verdicts', hasKey: true });
  const values = { 'decisions-endpoint': 'custom', 'decisions-url': ' https://d.example.com ', 'decisions-model': 'judge-31b' };
  await admin.submit(form(values));
  assert.deepEqual(requests[0].body, { endpoint: 'custom', model: 'judge-31b', checkRuns: false, checkTree: false, checkReports: false, url: 'https://d.example.com' });
  await admin.submit(form({ ...values, 'decisions-key': 'sk-new' }));
  assert.equal(requests[1].body.key, 'sk-new');
  await admin.submit(form(values, { 'decisions-remove-key': true }));
  assert.equal(requests[2].body.key, null);
});

test('Check asks with the form as it is and says what came back', async () => {
  const out = { textContent: '' };
  const values = form({ 'decisions-endpoint': 'custom', 'decisions-url': 'https://d.example.com' });
  const button = { closest: () => ({ ...values, querySelector: () => out }) };
  const ok = await fixture();
  await ok.admin.actions['admin-decisions-check'](button);
  assert.deepEqual(ok.requests[0], { path: '/api/admin/decisions/check', method: 'POST', body: { endpoint: 'custom', model: null, checkRuns: false, checkTree: false, checkReports: false, url: 'https://d.example.com' } });
  assert.equal(out.textContent, 'Working. judge-31b answered a sample true statement correctly (98% yes) in 412 ms.');
  const wrong = await fixture(saved, { ok: true, model: 'tiny', yes: 0.2, confidence: 0.4, ms: 90 });
  await wrong.admin.actions['admin-decisions-check'](button);
  assert.match(out.textContent, /tiny answered a sample true statement wrongly \(20% yes\)\. Try another model\./);
  const failed = await fixture(saved, { ok: false, error: 'The endpoint answered 404', ms: 30 });
  await failed.admin.actions['admin-decisions-check'](button);
  assert.equal(out.textContent, 'No answer: The endpoint answered 404');
});

test('choosing another endpoint shows its fields and who its usage counts against', async () => {
  const { admin } = await fixture();
  const section = { hidden: true }, help = { textContent: '' };
  admin.input({ target: { id: 'decisions-endpoint', value: 'custom', form: { querySelector: () => section }, closest: () => ({ querySelector: () => help }) } });
  assert.equal(section.hidden, false);
  assert.match(help.textContent, /counts against the key, not against people/);
});

test('without a secret key no key can be entered', async () => {
  const { admin } = await fixture({ ...saved, endpoint: 'custom', url: 'https://d.example.com/v1/verdicts', canStoreKey: false });
  const view = admin.view();
  assert.match(view, /Saving a key needs OBLETH_CHAT_SECRET_KEY_FILE/);
  assert.match(view, /id="decisions-key"[^>]* disabled/);
});
