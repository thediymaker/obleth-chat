#!/usr/bin/env node
// Repeatable API/concurrency smoke test. It refuses non-loopback origins and
// requires credentials created by run-load-fixture.py for a disposable database.
import assert from 'node:assert/strict';
import { readFile, writeFile, stat, mkdir } from 'node:fs/promises';
import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { performance } from 'node:perf_hooks';
import { readEvents } from '../web/client.js';

const options = new Map();
for (let i = 2; i < process.argv.length; i += 2) options.set(process.argv[i], process.argv[i + 1]);
const credentialsFile = options.get('--credentials');
if (!credentialsFile) throw new Error('Use --credentials with a disposable local fixture credential file.');
const permissions = await stat(credentialsFile);
assert.equal(permissions.mode & 0o077, 0, 'Credential file must not be accessible to other users.');
const config = JSON.parse(await readFile(credentialsFile, 'utf8'));
const origin = new URL(config.url);
assert(['http:', 'https:'].includes(origin.protocol), 'Only HTTP(S) origins are supported.');
assert(['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname), 'Load testing requires a literal loopback target.');
assert(!origin.username && !origin.password && !origin.search && !origin.hash && origin.pathname === '/', 'Use a plain loopback origin.');
assert(config.kind === 'obleth-chat-disposable-load' && config.database === 'obleth_load', 'A disposable load fixture is required.');
assert.equal(origin.port, '18087', 'This fixture owns port18087; other local QA services must remain untouched.');
const output = path.resolve(options.get('--output') || '.local/foundation-load.json');
const userCount = Number(options.get('--users') || 16);
const rounds = Number(options.get('--rounds') || 3);
assert(Number.isInteger(userCount) && userCount >= 1 && userCount <= 32, 'Use1–32 users.');
assert(Number.isInteger(rounds) && rounds >= 1 && rounds <= 10, 'Use1–10 rounds.');
const statuses = {}, timings = [], checks = [], accounts = [];
const runId = randomBytes(8).toString('hex');
let administrator, failure = null;

function check(name, condition) {
  assert(condition, name);
  checks.push(name);
}
async function request(session, method, endpoint, body, stream = false) {
  assert(endpoint.startsWith('/api/'));
  const response = await fetch(new URL(endpoint, origin), {
    method, redirect: 'error', signal: AbortSignal.timeout(30_000),
    headers: { Origin: origin.origin, Accept: stream ? 'text/event-stream' : 'application/json',
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      ...(session ? { Cookie: session.cookie, 'X-CSRF-Token': session.csrfToken } : {}) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  statuses[response.status] = (statuses[response.status] || 0) + 1;
  if (stream) return response;
  const value = response.status === 204 ? null : await response.json();
  return { status: response.status, value, headers: response.headers };
}
function requireStatus(result, status, action) {
  // Deliberately exclude response bodies, account names, and credentials.
  assert.equal(result.status, status, `${action} returned HTTP${result.status}`);
  return result.value;
}
async function login(username, password) {
  const result = await request(null, 'POST', '/api/auth/login', { username, password });
  const value = requireStatus(result, 200, 'Login');
  const cookie = result.headers.get('set-cookie')?.split(';')[0];
  assert(cookie && value.csrfToken, 'Login must return its cookie and CSRF token.');
  return { cookie, csrfToken: value.csrfToken };
}
async function mapLimited(items, limit, fn) {
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(items.length, limit) }, async () => {
    while (next < items.length) { const index = next++; await fn(items[index], index); }
  }));
}
async function rss() {
  if (!Number.isInteger(config.servicePid) || config.servicePid <= 1) return null;
  try {
    const status = await readFile(`/proc/${config.servicePid}/status`, 'utf8');
    return Number(status.match(/^VmRSS:\s+(\d+) kB$/m)?.[1] || 0) * 1024;
  } catch { return null; }
}
function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted.length ? Math.round(sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)] * 100) / 100 : null;
}
const memory = { beforeBytes: null, peakBytes: null, afterBytes: null };
let sample, loadMs = 0;
const totalStarted = performance.now();
try {
  administrator = await login(config.username, config.password);
  const catalog = requireStatus(await request(administrator, 'GET', '/api/models'), 200, 'Fixture discovery');
  check('Deterministic local gateway model is available', catalog.models.some(m => m.id === 'fixture-reasoning'));
  await mapLimited(Array.from({ length: userCount }), 4, async (_, index) => {
    const username = `load_${runId}_${index}`;
    const password = randomBytes(24).toString('base64url');
    const identity = requireStatus(await request(administrator, 'POST', '/api/admin/users', {
      username, password, displayName: `Load fixture ${index}`, role: 'user',
    }), 201, 'Create isolated account');
    const account = { id: identity.id, username, password, index, prompts: [] };
    accounts.push(account);
    account.session = await login(username, password);
    account.chat = requireStatus(await request(account.session, 'POST', '/api/conversations', {
      title: `Disposable load ${index}`, model: 'fixture-reasoning', reasoningEffort: 'medium',
    }), 201, 'Create isolated conversation').conversation.id;
  });
  // Retain credentials only in the private temporary fixture file; the runner
  // removes this file even when validation fails.
  await writeFile(credentialsFile, JSON.stringify({ ...config, accounts: accounts.map(({ id, username, password }) => ({ id, username, password })) }), { mode: 0o600 });
  memory.beforeBytes = await rss(); memory.peakBytes = memory.beforeBytes;
  sample = setInterval(async () => { const value = await rss(); if (value != null) memory.peakBytes = Math.max(memory.peakBytes || 0, value); }, 50);
  const loadStarted = performance.now();
  for (let round = 0; round < rounds; round++) {
    const outcomes = await Promise.allSettled(accounts.map(async account => {
      const prompt = `Fixture ${runId} account${account.index} round${round}. Reply with the deterministic fixture response.`;
      account.prompts.push(prompt);
      const started = performance.now();
      const response = await request(account.session, 'POST', `/api/conversations/${account.chat}/messages`, {
        content: prompt, model: 'fixture-reasoning', reasoningEffort: 'medium', maxTokens: 256,
      }, true);
      const headerMs = performance.now() - started;
      assert.equal(response.status, 200, `Generation returned HTTP${response.status}`);
      assert(response.headers.get('content-type')?.includes('text/event-stream'));
      let complete = false, answer = '', reasoning = '', usage;
      for await (const event of readEvents(response.body)) {
        assert.notEqual(event.event, 'error', 'Generation emitted an error event.');
        if (event.event === 'delta') answer += event.data.content || '';
        if (event.event === 'reasoning') reasoning += event.data.content || '';
        if (event.event === 'done') {
          assert.equal(event.data.status, 'completed'); complete = true; usage = event.data.usage; break;
        }
      }
      assert(complete && answer.includes('## A saved response') && reasoning.length > 0, 'Stream must finish with answer and reasoning.');
      assert.equal(usage?.total_tokens, 83, 'Fixture usage must survive the stream.');
      timings.push({ headerMs, streamMs: performance.now() - started });
    }));
    const failed = outcomes.filter(o => o.status === 'rejected');
    assert.equal(failed.length, 0, `${failed.length} concurrent streams failed in round${round + 1}.`);
  }
  loadMs = performance.now() - loadStarted;
  clearInterval(sample); memory.afterBytes = await rss();
  memory.peakBytes = Math.max(memory.peakBytes || 0, memory.afterBytes || 0);
  check('All parallel streams completed successfully', timings.length === userCount * rounds);
  await mapLimited(accounts, 8, async (account, index) => {
    const saved = requireStatus(await request(account.session, 'GET', `/api/conversations/${account.chat}`), 200, 'Read saved conversation').conversation;
    assert.equal(saved.messages.length, rounds * 2, 'Conversation must have exactly its submitted turns.');
    for (let round = 0; round < rounds; round++) {
      assert.equal(saved.messages[round * 2].content, account.prompts[round], 'User message crossed account or turn boundaries.');
      const answer = saved.messages[round * 2 + 1];
      assert.equal(answer.status, 'completed');
      assert(answer.content.includes('## A saved response') && answer.reasoningContent.length > 0);
      assert.equal(answer.usage?.total_tokens, 83);
    }
    const usage = requireStatus(await request(account.session, 'GET', '/api/usage'), 200, 'Read private usage');
    assert.equal(usage.today.requests, rounds); assert.equal(usage.today.totalTokens, rounds * 83);
    assert.equal(usage.today.activeGenerations, 0); assert.equal(usage.today.reservedTokens, 0);
    const list = requireStatus(await request(account.session, 'GET', '/api/conversations'), 200, 'Read private history');
    assert.equal(list.total, 1); assert.equal(list.conversations[0].id, account.chat);
    if (accounts.length > 1) {
      const foreign = accounts[(index + 1) % accounts.length].chat;
      requireStatus(await request(account.session, 'GET', `/api/conversations/${foreign}`), 404, 'Foreign conversation read');
      requireStatus(await request(account.session, 'POST', '/api/conversations/export', { ids: [foreign] }), 404, 'Foreign conversation export');
    }
    requireStatus(await request(account.session, 'GET', '/api/admin/users'), 403, 'Ordinary account administration');
  });
  check('Every answer, reasoning block, prompt, and usage record persisted', true);
  check('User history and export ownership are isolated', true);
  check('Ordinary users cannot access account administration', true);
  check('Usage reservations were released and totals match actual generations', true);
  check('No HTTP500 responses occurred', !Object.keys(statuses).some(s => Number(s) >= 500));
} catch (error) {
  // Assertion messages above contain counts/action names only, never content.
  failure = error instanceof assert.AssertionError ? error.message.split('\n')[0] : error.name || 'Load test failed';
} finally {
  clearInterval(sample);
  await mapLimited(accounts, 4, async account => {
    try { if (account.chat && account.session) await request(account.session, 'DELETE', `/api/conversations/${account.chat}`); } catch {}
    try { if (administrator && account.id) await request(administrator, 'PATCH', `/api/admin/users/${account.id}`, { enabled: false }); } catch {}
  });
  if (Object.keys(statuses).some(status => Number(status) >= 500)) failure ||= 'An HTTP500 response occurred during validation or cleanup.';
  const result = {
    kind: 'local-fixture-concurrency-smoke', upstream: 'deterministic loopback gateway; no real inference',
    buildProfile: config.buildProfile || 'unspecified', users: userCount, rounds,
    concurrentStreamsPerRound: userCount, completedGenerations: timings.length, expectedGenerations: userCount * rounds,
    loadDurationMs: Math.round(loadMs), totalDurationMs: Math.round(performance.now() - totalStarted),
    latencyMs: { responseHeaders: { p50: percentile(timings.map(t => t.headerMs), .5), p95: percentile(timings.map(t => t.headerMs), .95) },
      completeStream: { p50: percentile(timings.map(t => t.streamMs), .5), p95: percentile(timings.map(t => t.streamMs), .95) } },
    rustResidentMemory: memory, httpStatuses: statuses, checks, passed: failure === null,
    ...(failure ? { failure } : {}),
  };
  await mkdir(path.dirname(output), { recursive: true });
  await writeFile(output, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
  process.stdout.write(`${JSON.stringify(result)}\n`);
  if (failure) process.exitCode = 1;
}
