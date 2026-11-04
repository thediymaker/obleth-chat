// The browser half of Chat's API. The session cookie is HttpOnly; the CSRF token
// from /api/session stays in memory and goes on every write.

export class ApiError extends Error {
  status: number;
  constructor(message: string, status = 0) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export interface RequestOptions {
  method?: string;
  /** Sent as JSON. */
  body?: unknown;
  /** Sent as is, with its own content type and time for a large upload. */
  raw?: BodyInit;
  contentType?: string;
  signal?: AbortSignal;
  /** Returns the Response of a text/event-stream instead of parsing JSON. */
  stream?: boolean;
}

async function responseError(response: Response): Promise<ApiError> {
  let message =
    response.status === 401
      ? "Your session has expired. Sign in again to continue."
      : `The request failed (${response.status}). Please try again.`;
  try {
    const data = await response.json();
    const detail = typeof data.error === "string" ? data.error : data.error?.message || data.message;
    if (typeof detail === "string") message = detail;
  } catch {
    // A proxy may return HTML rather than an API error.
  }
  return new ApiError(message, response.status);
}

export class ApiClient {
  csrfToken = "";
  onUnauthorized: (error: ApiError) => void = () => {};
  private sessionAbort = new AbortController();
  private fetcher: typeof fetch;

  constructor(fetcher: typeof fetch = (...args) => fetch(...args)) {
    this.fetcher = fetcher;
  }

  /** Drops the CSRF token and cancels every request of the old session. */
  clearSession() {
    this.csrfToken = "";
    this.sessionAbort.abort();
    this.sessionAbort = new AbortController();
  }

  request<T = any>(path: string, options: RequestOptions & { stream: true }): Promise<Response>;
  request<T = any>(path: string, options?: RequestOptions): Promise<T>;
  async request<T = any>(path: string, { method = "GET", body, raw, contentType, signal, stream = false }: RequestOptions = {}): Promise<T | Response> {
    if (!path.startsWith("/api/")) throw new Error("API requests must use this origin.");
    const headers: Record<string, string> = { Accept: stream ? "text/event-stream" : "application/json" };
    if (raw !== undefined) headers["Content-Type"] = contentType || "application/octet-stream";
    else if (body !== undefined) headers["Content-Type"] = "application/json";
    if (!["GET", "HEAD"].includes(method) && this.csrfToken) headers["X-CSRF-Token"] = this.csrfToken;
    const sessionSignal = this.sessionAbort.signal;
    const signals = [sessionSignal, ...(signal ? [signal] : [])];
    if (!stream) signals.push(AbortSignal.timeout(raw !== undefined ? 600_000 : 30_000));
    let response: Response;
    try {
      response = await this.fetcher(path, {
        method,
        headers,
        credentials: "same-origin",
        cache: "no-store",
        body: raw !== undefined ? raw : body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.any(signals),
      });
    } catch (error) {
      if ((error as Error).name === "AbortError") throw error;
      if ((error as Error).name === "TimeoutError") throw new ApiError("The request took too long. Please try again.");
      throw new ApiError("Connection lost. Your saved conversations will be here when the service returns.");
    }
    sessionSignal.throwIfAborted();
    if (!response.ok) {
      const error = await responseError(response);
      sessionSignal.throwIfAborted();
      if (response.status === 401 && path !== "/api/auth/login") this.onUnauthorized(error);
      throw error;
    }
    if (stream) {
      if (!response.headers.get("content-type")?.includes("text/event-stream")) {
        throw new ApiError("The service returned an invalid response stream.");
      }
      return response;
    }
    if (response.status === 204) return {} as T;
    let result: T;
    try {
      result = await response.json();
    } catch {
      throw new ApiError("The service returned an invalid response.");
    }
    sessionSignal.throwIfAborted();
    return result;
  }

  /** A POST that still goes out while the page closes, such as leaving a temporary chat. */
  send(path: string): Promise<void> {
    if (!path.startsWith("/api/")) throw new Error("API requests must use this origin.");
    const headers: Record<string, string> = this.csrfToken ? { "X-CSRF-Token": this.csrfToken } : {};
    return this.fetcher(path, { method: "POST", headers, credentials: "same-origin", cache: "no-store", keepalive: true }).then(
      () => {},
      () => {},
    );
  }
}

/** The one client the app uses. */
export const api = new ApiClient();

export interface StreamEvent<T = any> {
  event: string;
  data: T;
}

/** Reads SSE by lines, keeping UTF-8 and CRLF boundaries between chunks. */
export async function* readEvents(body: ReadableStream<Uint8Array> | null): AsyncGenerator<StreamEvent> {
  if (!body) throw new ApiError("The service returned an empty response stream.");
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let event = "message";
  let data: string[] = [];
  const parse = (): StreamEvent | null => {
    if (!data.length) {
      event = "message";
      return null;
    }
    let value: unknown;
    try {
      value = JSON.parse(data.join("\n"));
    } catch {
      throw new ApiError("The service returned an invalid stream event.");
    }
    const result = { event, data: value };
    event = "message";
    data = [];
    return result;
  };
  try {
    while (true) {
      const { value, done } = await reader.read();
      buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
      let index: number;
      while ((index = buffer.indexOf("\n")) !== -1) {
        const line = buffer.slice(0, index).replace(/\r$/, "");
        buffer = buffer.slice(index + 1);
        if (!line) {
          const item = parse();
          if (item) yield item;
        } else if (line.startsWith("event:")) event = line.slice(6).trimStart();
        else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
      }
      if (done) break;
    }
    if (buffer.startsWith("data:")) data.push(buffer.slice(5).replace(/^ /, ""));
    const item = parse();
    if (item) yield item;
  } finally {
    // Cancels upstream if the consumer stops after a terminal event.
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
