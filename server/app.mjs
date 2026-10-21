import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { realpath, stat } from 'node:fs/promises';
import { extname, isAbsolute, relative, resolve, sep } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { defaultStaticDir } from './config.mjs';
import { createMetrics } from './metrics.mjs';

export { readConfig } from './config.mjs';

const contentTypes = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.txt': 'text/plain; charset=utf-8',
};

export const capabilities = Object.freeze({
  application: 'obleth-chat',
  mode: 'preview',
  liveExecution: false,
  integrations: Object.freeze(Object.fromEntries(
    ['gateway', 'oidc', 'rag', 'kubernetes', 'slurm'].map(name => [name, Object.freeze({ configured: false })]),
  )),
  features: Object.freeze({ staticPreview: true, metrics: true }),
  notice: 'Interface preview only. Authentication, inference, retrieval, and workload execution are not connected.',
});

function pathFromRequest(target) {
  if (!target?.startsWith('/')) throw new Error('Invalid path');
  const path = decodeURIComponent(target.split('?')[0]);
  if (path.includes('\\') || path.includes('\0') || path.split('/').some(part => part === '.' || part === '..')) {
    throw new Error('Invalid path');
  }
  return path;
}

function headers(response) {
  response.setHeader('Cache-Control', 'no-store');
  response.setHeader('X-Content-Type-Options', 'nosniff');
  response.setHeader('X-Frame-Options', 'DENY');
  response.setHeader('Referrer-Policy', 'no-referrer');
  response.setHeader('Permissions-Policy', 'camera=(), microphone=(), geolocation=()');
  response.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'");
}

function send(request, response, status, body, contentType = 'application/json; charset=utf-8') {
  response.statusCode = status;
  response.setHeader('Content-Type', contentType);
  response.setHeader('Content-Length', Buffer.byteLength(body));
  response.end(request.method === 'HEAD' ? undefined : body);
}

function json(request, response, status, body) {
  send(request, response, status, JSON.stringify(body));
}

async function staticFile(staticDir, pathname) {
  if (pathname.split('/').some(part => part.startsWith('.'))) return null;
  try {
    const root = await realpath(staticDir);
    const file = await realpath(resolve(root, `.${pathname === '/' ? '/index.html' : pathname}`));
    const within = relative(root, file);
    if (!within || within === '..' || within.startsWith(`..${sep}`) || isAbsolute(within)) return null;
    const info = await stat(file);
    return info.isFile() ? { file, info } : null;
  } catch (error) {
    if (['ENOENT', 'ENOTDIR', 'EACCES', 'EPERM', 'ELOOP'].includes(error.code)) return null;
    throw error;
  }
}

/** Returns an unbound http.Server. Each app instance owns its metrics. */
export function createApp({ staticDir = defaultStaticDir } = {}) {
  const metrics = createMetrics();
  const server = createServer((request, response) => {
    headers(response);
    const method = ['GET', 'HEAD'].includes(request.method) ? request.method : 'OTHER';
    let route = 'invalid';
    const finish = metrics.begin();
    response.once('finish', () => finish(method, route, response.statusCode));
    response.once('close', () => finish(method, route, response.writableFinished ? response.statusCode : 499));

    async function handle() {
      let pathname;
      try {
        pathname = pathFromRequest(request.url);
      } catch {
        json(request, response, 400, { error: 'Invalid request path' });
        return;
      }

      route = ['/healthz', '/readyz', '/metrics', '/api/capabilities'].includes(pathname) ? pathname : 'not_found';
      if (!['GET', 'HEAD'].includes(request.method)) {
        request.resume();
        response.setHeader('Allow', 'GET, HEAD');
        json(request, response, 405, { error: 'Method not allowed' });
        return;
      }

      if (pathname === '/healthz') {
        json(request, response, 200, { status: 'ok', mode: 'preview' });
        return;
      }
      if (pathname === '/readyz') {
        const ready = Boolean(await staticFile(staticDir, '/index.html'));
        json(request, response, ready ? 200 : 503, { status: ready ? 'ready' : 'not_ready', mode: 'preview' });
        return;
      }
      if (pathname === '/api/capabilities') {
        json(request, response, 200, capabilities);
        return;
      }
      if (pathname === '/metrics') {
        send(request, response, 200, metrics.render(), 'text/plain; version=0.0.4; charset=utf-8');
        return;
      }
      // Reserved API paths never fall through to static content.
      const asset = pathname.startsWith('/api/') ? null : await staticFile(staticDir, pathname);
      if (!asset) {
        json(request, response, 404, { error: 'Not found' });
        return;
      }
      route = 'static';
      response.statusCode = 200;
      response.setHeader('Content-Type', contentTypes[extname(asset.file).toLowerCase()] ?? 'application/octet-stream');
      response.setHeader('Content-Length', asset.info.size);
      if (request.method === 'HEAD') {
        response.end();
        return;
      }
      await pipeline(createReadStream(asset.file), response);
    }

    handle().catch(() => {
      if (!response.headersSent && !response.destroyed) {
        json(request, response, 500, { error: 'Internal server error' });
      } else if (!response.destroyed) {
        response.destroy();
      }
    });
  });
  server.requestTimeout = 15_000;
  server.headersTimeout = 10_000;
  server.keepAliveTimeout = 5_000;
  server.maxHeadersCount = 100;
  return server;
}
