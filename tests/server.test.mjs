import test from 'node:test';
import assert from 'node:assert/strict';
import { request as httpRequest } from 'node:http';
import { mkdtemp, mkdir, writeFile, symlink, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createApp, readConfig } from '../server/app.mjs';

async function fixture(t, { index = true } = {}) {
  const temporary = await mkdtemp(join(tmpdir(), 'obleth-chat-test-'));
  const staticDir = join(temporary, 'web');
  await mkdir(join(staticDir, 'assets'), { recursive: true });
  if (index) await writeFile(join(staticDir, 'index.html'), '<!doctype html><title>Preview</title>');
  await writeFile(join(staticDir, 'app.js'), 'export const preview = true;');
  await writeFile(join(staticDir, '.env'), 'fixture-only-secret');
  await writeFile(join(temporary, 'outside.txt'), 'outside-static-root');
  await symlink(join(temporary, 'outside.txt'), join(staticDir, 'assets', 'escaped.txt'));
  const server = createApp({ staticDir });
  await new Promise((accept, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', accept);
  });
  t.after(async () => {
    await new Promise(accept => {
      server.close(accept);
      server.closeAllConnections();
    });
    await rm(temporary, { recursive: true, force: true });
  });
  return {
    server,
    request(path, { method = 'GET', body } = {}) {
      return new Promise((accept, reject) => {
        // Use the raw HTTP path so URL normalization cannot hide traversal cases.
        const request = httpRequest({ host: '127.0.0.1', port: server.address().port, path, method }, response => {
          const chunks = [];
          response.on('data', chunk => chunks.push(chunk));
          response.on('end', () => accept({
            status: response.statusCode,
            headers: response.headers,
            body: Buffer.concat(chunks).toString(),
          }));
        });
        request.on('error', reject);
        request.end(body);
      });
    },
  };
}

test('health, readiness, and capabilities accurately describe an unconnected preview', async t => {
  const app = await fixture(t);
  const health = await app.request('/healthz');
  assert.equal(health.status, 200);
  assert.deepEqual(JSON.parse(health.body), { status: 'ok', mode: 'preview' });
  assert.equal((await app.request('/readyz')).status, 200);

  const capabilities = await app.request('/api/capabilities');
  assert.equal(capabilities.status, 200);
  const data = JSON.parse(capabilities.body);
  assert.equal(data.mode, 'preview');
  assert.equal(data.liveExecution, false);
  assert.deepEqual(Object.keys(data.integrations).sort(), ['gateway', 'kubernetes', 'oidc', 'rag', 'slurm']);
  for (const integration of Object.values(data.integrations)) assert.equal(integration.configured, false);
  assert.equal((await app.request('/api/chat')).status, 404);
});

test('readiness fails when the preview entry point is unavailable while liveness succeeds', async t => {
  const app = await fixture(t, { index: false });
  const response = await app.request('/readyz');
  assert.equal(response.status, 503);
  assert.equal(JSON.parse(response.body).status, 'not_ready');
  assert.equal((await app.request('/healthz')).status, 200);
});

test('serves static assets and HEAD responses with local-only browser security policy', async t => {
  const app = await fixture(t);
  const index = await app.request('/');
  assert.equal(index.status, 200);
  assert.match(index.headers['content-type'], /^text\/html/);
  assert.match(index.body, /<title>Preview<\/title>/);
  assert.equal(index.headers['x-content-type-options'], 'nosniff');
  assert.equal(index.headers['cache-control'], 'no-store');
  assert.match(index.headers['content-security-policy'], /script-src 'self'/);
  assert.match(index.headers['content-security-policy'], /frame-ancestors 'none'/);

  const script = await app.request('/app.js?cache-bust=123');
  assert.equal(script.status, 200);
  assert.match(script.headers['content-type'], /^text\/javascript/);
  const head = await app.request('/app.js', { method: 'HEAD' });
  assert.equal(head.status, 200);
  assert.equal(head.body, '');
  assert.equal(Number(head.headers['content-length']), Buffer.byteLength(script.body));
  const apiHead = await app.request('/healthz', { method: 'HEAD' });
  assert.equal(apiHead.status, 200);
  assert.equal(apiHead.body, '');
  assert.equal((await app.request('/missing')).status, 404);
});

test('rejects traversal and malformed paths, and denies dotfiles and escaped symlinks', async t => {
  const app = await fixture(t);
  for (const path of ['/../outside.txt', '/%2e%2e/outside.txt', '/assets/%2E%2E/%2e%2e/outside.txt', '/assets%2f..%2f..%2foutside.txt', '/%5c..%5coutside.txt', '/bad%ZZ', '/%00']) {
    const response = await app.request(path);
    assert.equal(response.status, 400, path);
    assert.doesNotMatch(response.body, /outside-static-root|fixture-only-secret/);
  }
  for (const path of ['/.env', '/%2eenv', '/assets/escaped.txt', '/assets/', '/%252e%252e/outside.txt']) {
    const response = await app.request(path);
    assert.equal(response.status, 404, path);
    assert.doesNotMatch(response.body, /outside-static-root|fixture-only-secret/);
  }
});

test('unsupported methods cannot mutate or execute anything', async t => {
  const app = await fixture(t);
  for (const path of ['/', '/healthz', '/metrics', '/api/capabilities', '/api/chat']) {
    const response = await app.request(path, { method: 'POST', body: '{"prompt":"test"}' });
    assert.equal(response.status, 405, path);
    assert.equal(response.headers.allow, 'GET, HEAD');
  }
  assert.equal((await app.request('/app.js', { method: 'DELETE' })).status, 405);
  assert.equal((await app.request('/app.js')).status, 200);
});

test('Prometheus metrics reflect requests and durations without user-controlled label values', async t => {
  const app = await fixture(t);
  await app.request('/healthz?user=private-person');
  await app.request('/app.js');
  await app.request('/untrusted-user-path-1');
  await app.request('/untrusted-user-path-2');
  await app.request('/healthz', { method: 'POST' });
  const response = await app.request('/metrics');
  assert.equal(response.status, 200);
  assert.equal(response.headers['content-type'], 'text/plain; version=0.0.4; charset=utf-8');
  assert.match(response.body, /obleth_chat_http_requests_total\{method="GET",route="\/healthz",status="200"\} 1/);
  assert.match(response.body, /obleth_chat_http_requests_total\{method="GET",route="not_found",status="404"\} 2/);
  assert.match(response.body, /obleth_chat_http_requests_total\{method="OTHER",route="\/healthz",status="405"\} 1/);
  assert.match(response.body, /obleth_chat_http_request_duration_seconds_bucket\{method="GET",route="static",le="\+Inf"\} 1/);
  assert.match(response.body, /obleth_chat_http_request_duration_seconds_count\{method="GET",route="static"\} 1/);
  assert.match(response.body, /obleth_chat_http_active_requests 1\n/);
  assert.doesNotMatch(response.body, /private-person|untrusted-user-path|app\.js/);

  const isolated = await fixture(t);
  const isolatedMetrics = await isolated.request('/metrics');
  assert.doesNotMatch(isolatedMetrics.body, /route="\/healthz"/);
});

test('configuration defaults to loopback and validates explicit ports and host syntax', () => {
  const defaults = readConfig({});
  assert.equal(defaults.host, '127.0.0.1');
  assert.equal(defaults.port, 4173);
  assert.match(defaults.staticDir, /\/web\/$/);
  assert.deepEqual(readConfig({ HOST: '0.0.0.0', PORT: '8080', STATIC_DIR: './web' }), {
    host: '0.0.0.0', port: 8080, staticDir: resolve('./web'),
  });
  for (const port of ['', '0', '-1', '65536', '1.5', '8000x', ' 4173']) {
    assert.throws(() => readConfig({ PORT: port }), /PORT/);
  }
  for (const host of ['', 'http://localhost', 'localhost/preview', '127.0.0.1\n']) {
    assert.throws(() => readConfig({ HOST: host }), /HOST/);
  }
  assert.throws(() => readConfig({ STATIC_DIR: ' ' }), /STATIC_DIR/);
});
