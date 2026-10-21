// Session secrets stay in memory. The browser sends the HttpOnly session cookie.
export class ApiError extends Error {
  constructor(message, status = 0) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

export async function capabilities(fetcher = fetch) {
  const response = await fetcher('./api/capabilities', { credentials: 'same-origin', headers: { Accept: 'application/json' }, cache: 'no-store', signal: AbortSignal.timeout(15000) });
  // Static Sites hosting may return its HTML entry point for unknown routes.
  if (response.status === 404 || (response.ok && response.headers.get('content-type')?.includes('text/html'))) return { mode: 'preview' };
  if (!response.ok) throw new ApiError('The service could not be reached. Please try again.', response.status);
  const result = await response.json();
  if (!['preview', 'connected'].includes(result.mode)) throw new ApiError('The service returned an unsupported configuration.');
  return result;
}

async function responseError(response) {
  let message = response.status === 401 ? 'Your session has expired. Sign in again to continue.' : `The request failed (${response.status}). Please try again.`;
  try {
    const data = await response.json();
    const detail = typeof data.error === 'string' ? data.error : data.error?.message || data.message;
    if (typeof detail === 'string') message = detail;
  } catch { /* A proxy may return HTML rather than an API error. */ }
  return new ApiError(message, response.status);
}

export class ApiClient {
  constructor({ fetcher = fetch, onUnauthorized = () => {} } = {}) {
    this.fetcher = fetcher.bind(globalThis);
    this.onUnauthorized = onUnauthorized;
    this.csrfToken = '';
    this.sessionAbort = new AbortController();
  }
  clearSession() {
    this.csrfToken = '';
    this.sessionAbort.abort();
    this.sessionAbort = new AbortController();
  }
  // `raw` sends a file as the body with its own content type, with time for a large upload.
  async request(path, { method = 'GET', body, raw, contentType, signal, stream = false } = {}) {
    if (!path.startsWith('/api/')) throw new Error('API requests must use this origin.');
    const headers = { Accept: stream ? 'text/event-stream' : 'application/json' };
    if (raw !== undefined) headers['Content-Type'] = contentType || 'application/octet-stream';
    else if (body !== undefined) headers['Content-Type'] = 'application/json';
    if (!['GET', 'HEAD'].includes(method) && this.csrfToken) headers['X-CSRF-Token'] = this.csrfToken;
    const sessionSignal = this.sessionAbort.signal;
    const requestSignal = AbortSignal.any([sessionSignal, ...(signal ? [signal] : []), ...(!stream ? [AbortSignal.timeout(raw !== undefined ? 600000 : 30000)] : [])]);
    let response;
    try {
      response = await this.fetcher(path, { method, headers, credentials: 'same-origin', cache: 'no-store', body: raw !== undefined ? raw : body === undefined ? undefined : JSON.stringify(body), signal: requestSignal });
    } catch (error) {
      if (error.name === 'AbortError') throw error;
      throw new ApiError('Connection lost. Your saved conversations will be here when the service returns.');
    }
    sessionSignal.throwIfAborted();
    if (!response.ok) {
      const error = await responseError(response);
      sessionSignal.throwIfAborted();
      if (response.status === 401 && path !== '/api/auth/login') this.onUnauthorized(error);
      throw error;
    }
    if (stream) {
      if (!response.headers.get('content-type')?.includes('text/event-stream')) throw new ApiError('The service returned an invalid response stream.');
      return response;
    }
    if (response.status === 204) { sessionSignal.throwIfAborted(); return {}; }
    let result;
    try { result = await response.json(); }
    catch { throw new ApiError('The service returned an invalid response.'); }
    sessionSignal.throwIfAborted();
    return result;
  }
}

// Read SSE by lines, preserving UTF-8 and CRLF boundaries between chunks.
export async function* readEvents(body) {
  if (!body) throw new ApiError('The service returned an empty response stream.');
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = '', event = 'message', data = [];
  const parse = () => {
    if (!data.length) { event = 'message'; return null; }
    let value;
    try { value = JSON.parse(data.join('\n')); }
    catch { throw new ApiError('The service returned an invalid stream event.'); }
    const result = { event, data: value };
    event = 'message'; data = [];
    return result;
  };
  try {
    while (true) {
      const { value, done } = await reader.read();
      buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
      let index;
      while ((index = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, index).replace(/\r$/, '');
        buffer = buffer.slice(index + 1);
        if (!line) { const item = parse(); if (item) yield item; }
        else if (line.startsWith('event:')) event = line.slice(6).trimStart();
        else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
      }
      if (done) break;
    }
    if (buffer.startsWith('data:')) data.push(buffer.slice(5).replace(/^ /, ''));
    const item = parse();
    if (item) yield item;
  } finally {
    // Cancels upstream if the consumer stops after a terminal event.
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
