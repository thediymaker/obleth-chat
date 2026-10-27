import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiClient, capabilities, readEvents } from '../web/client.js';
import { markdown, pastesAsFile, promptCount } from '../web/connected.js';

const json = (data, status = 200) => new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } });
const stream = chunks => new ReadableStream({ start(controller) { chunks.forEach(chunk => controller.enqueue(chunk)); controller.close(); } });

test('SSE reader preserves fragmented UTF-8, CRLF, comments, and multiline data', async () => {
  const bytes = new TextEncoder().encode(':keepalive\r\nevent: delta\r\ndata: {\r\ndata: "content": "Hello 🌱"\r\ndata: }\r\n\r\nevent: done\ndata: {"status":"completed"}\n\n');
  const events = [];
  for await (const event of readEvents(stream([...bytes].map(byte => new Uint8Array([byte]))))) events.push(event);
  assert.deepEqual(events, [{ event: 'delta', data: { content: 'Hello 🌱' } }, { event: 'done', data: { status: 'completed' } }]);
});

test('SSE reader rejects malformed data rather than displaying a made-up response', async () => {
  const events = readEvents(stream([new TextEncoder().encode('event: delta\ndata: invalid\n\n')]));
  await assert.rejects(events.next(), /invalid stream event/);
});

test('SSE reader cancels an upstream stream when its consumer stops', async () => {
  let cancelled = false;
  const body = new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode('event: done\ndata: {}\n\n')); }, cancel() { cancelled = true; } });
  for await (const event of readEvents(body)) { assert.equal(event.event, 'done'); break; }
  assert.equal(cancelled, true);
});

test('authenticated mutations send same-origin cookies and an in-memory CSRF token', async () => {
  let captured;
  const client = new ApiClient({ fetcher: async (path, options) => { captured = { path, options }; return json({ conversation: { id: 'one' } }); } });
  client.csrfToken = 'csrf-test';
  await client.request('/api/conversations', { method: 'POST', body: { title: 'Hello' } });
  assert.equal(captured.options.credentials, 'same-origin');
  assert.equal(captured.options.headers['X-CSRF-Token'], 'csrf-test');
  assert.equal(captured.options.body, '{"title":"Hello"}');
  await assert.rejects(client.request('https://another-origin.test/api/conversations'), /this origin/);
});

test('session expiration clears UI session through callback but invalid login does not', async () => {
  let expired = 0;
  const client = new ApiClient({ fetcher: async () => json({ error: { message: 'Sign in again' } }, 401), onUnauthorized: () => expired++ });
  await assert.rejects(client.request('/api/conversations'), error => error.status === 401 && error.message === 'Sign in again');
  await assert.rejects(client.request('/api/auth/login', { method: 'POST', body: {} }), /Sign in again/);
  assert.equal(expired, 1);
});

test('sign-out invalidates in-flight private responses before another account can receive them', async () => {
  let finish;
  const client = new ApiClient({ fetcher: () => new Promise(resolve => { finish = resolve; }) });
  client.csrfToken = 'old-session-token';
  const pending = client.request('/api/conversations');
  client.clearSession();
  finish(json({ conversations: [{ id: 'other-users-private-chat' }] }));
  await assert.rejects(pending, error => error.name === 'AbortError');
  assert.equal(client.csrfToken, '');
});

test('capabilities allow an explicit or static preview but never hide a service failure', async () => {
  assert.equal((await capabilities(async () => json({ mode: 'preview' }))).mode, 'preview');
  assert.equal((await capabilities(async () => new Response('Not found', { status: 404 }))).mode, 'preview');
  assert.equal((await capabilities(async () => new Response('<html></html>', { headers: { 'Content-Type': 'text/html' } }))).mode, 'preview');
  await assert.rejects(capabilities(async () => json({ error: 'unavailable' }, 503)), /could not be reached/);
  await assert.rejects(capabilities(async () => { throw new Error('network unavailable'); }), /network unavailable/);
});

test('HTML and code fences from model content remain inert', () => {
  const html = markdown('<img src=x onerror=alert(1)>\n\n**hello**\n\n```<script>\n</code><svg onload=alert(1)>\n```');
  assert(!html.includes('<img'));
  assert(!html.includes('<svg onload'));
  assert(!html.includes('<script>'));
  assert(html.includes('&lt;svg'));
  assert(html.includes('<strong>hello</strong>'));
});

test('a paste goes in as a file when it is long or would pass the box limit', () => {
  const box = (value, start = value.length, end = start) => ({ value, selectionStart: start, selectionEnd: end });
  assert.equal(pastesAsFile('a'.repeat(4000), box('')), false);
  assert.equal(pastesAsFile('a'.repeat(4001), box('')), true);
  assert.equal(pastesAsFile('a'.repeat(3000), box('b'.repeat(9500))), true);
  // Replacing a selection frees what it covered.
  assert.equal(pastesAsFile('a'.repeat(3000), box('b'.repeat(9500), 0, 1000)), false);
  assert.equal(promptCount(10799), '');
  assert.equal(promptCount(12000), `${(12000).toLocaleString()} / ${(12000).toLocaleString()}`);
});
