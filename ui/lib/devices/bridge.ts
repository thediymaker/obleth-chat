// Devices at the person's computer, reached through this browser tab. A device
// connection's tool calls arrive as steps of a reply (service "device" with a
// request); this bridge carries each one out over Web Serial and posts the result,
// which the server waits for. The pages and cards read its state through the chat
// store (store.devices).

import type { StreamEvent } from "@/lib/api";
import { withFlasher, formatBytes, type FlashPart } from "./flashers";
import { hexBytes, portInfo, SerialDevice, type OpenOptions } from "./serial";

export interface DeviceRequest {
  tool: string;
  arguments: Record<string, any>;
}

export interface DeviceConnection {
  id: string;
  name?: string;
  vendorId?: number;
  productId?: number;
  /** The console sign-in saved for it: the user name and the secret holding the password. */
  user?: string;
  secret?: string;
}

/** A port tied to a device connection in this tab. */
export interface Attached {
  id: string;
  device: SerialDevice;
  vendorId?: number;
  productId?: number;
  user?: string;
  secret?: string;
}

/** A call that waits for its device to be attached in this tab. */
export interface Waiting {
  chatId: string;
  callId: string;
  connectionId: string;
  connection: string;
  request: DeviceRequest;
}

export interface DevicesBridge {
  /** The browser has Web Serial (Chrome and Edge do). */
  supported(): boolean;
  /** Devices are offered on this server (capabilities.features.devices). */
  enabled(): boolean;
  attached(connectionId: string): Attached | undefined;
  list(): Attached[];
  /** Asks the browser for a port and ties it to the connection; calls waiting for it then run. */
  attach(connection: DeviceConnection): Promise<Attached>;
  /** Ties a port the person already picked (for a device just added) to its connection. */
  adopt(connection: DeviceConnection, port: SerialPort): Attached;
  /** Ties ports the person granted earlier to their connections by USB ids. */
  restore(connections?: DeviceConnection[]): Promise<void>;
  /** A connection's saved sign-in changed. */
  update(connection: DeviceConnection): void;
  detach(connectionId: string): Promise<void>;
  /** The port the person picks for a new device, with its USB ids and a suggested name. */
  pick(): Promise<{ port: SerialPort; vendorId?: number; productId?: number }>;
  /** A stream event of a chat: a device step with a request is carried out. */
  observe(event: StreamEvent, chatId: string): void;
  /** A loaded chat: device steps still running are carried out (a reloaded page). */
  sync(chatId: string, activity: { id?: string; service?: string; status?: string; connectionId?: string; connection?: string; request?: unknown }[]): void;
  /** What a running call is doing right now, for its card. */
  progress(callId: string): string;
  waiting(callId: string): Waiting | undefined;
  /** A device-request card's answer: being attached, attached, or Not now. */
  offer(key: string): Offer | undefined;
  setOffer(key: string, value: Offer | null): void;
  subscribe(listener: () => void): () => void;
  reset(): void;
}

interface Context {
  api: { request: (path: string, options?: any) => Promise<any> };
  enabled: () => boolean;
  changed: () => void;
  /** Reads a workspace file whole; the default fetches with the session cookie. */
  fetchBytes?: (path: string) => Promise<Uint8Array>;
  /** Opens one of the person's secrets by name; the default asks the API. */
  secretValue?: (name: string) => Promise<string>;
  /** The Web Serial API; the default is the browser's. */
  serial?: Serial | null;
}

export type Offer = "busy" | "done" | "dismissed";

export interface Outcome {
  ok: boolean;
  output: string;
  detail?: string;
}

const resultPath = (chatId: string, callId: string) => `/api/conversations/${encodeURIComponent(chatId)}/device-calls/${encodeURIComponent(callId)}`;
export const rawPath = (chatId: string, path: string) => `/api/conversations/${encodeURIComponent(chatId)}/workspace/raw?path=${encodeURIComponent(path)}`;

async function fetchBytesDefault(path: string): Promise<Uint8Array> {
  const response = await fetch(path, { credentials: "same-origin", cache: "no-store" });
  if (!response.ok) {
    let message = `The file could not be read (${response.status})`;
    try {
      const data = await response.json();
      if (typeof data.error === "string") message = data.error;
      else if (typeof data.error?.message === "string") message = data.error.message;
    } catch {
      // Not JSON.
    }
    throw new Error(message);
  }
  return new Uint8Array(await response.arrayBuffer());
}

function parseOffset(value: unknown): number | null {
  if (typeof value === "number" && Number.isInteger(value) && value >= 0) return value;
  if (typeof value !== "string") return null;
  const text = value.trim();
  const n = /^0x/i.test(text) ? parseInt(text.slice(2), 16) : /^\d+$/.test(text) ? parseInt(text, 10) : NaN;
  return Number.isFinite(n) && n >= 0 && n < 2 ** 32 ? n : null;
}

const hex = (n: number | undefined) => (n == null ? "?" : `0x${n.toString(16).padStart(4, "0")}`);

export const secretPath = (name: string) => `/api/secrets/${encodeURIComponent(name)}/value`;

/** Signs in at a console: wakes it, answers the login and password prompts, reads what follows. */
async function login(a: Attached, user: string, secretValue: () => Promise<string>, progress: (text: string) => void): Promise<Outcome> {
  const { device } = a;
  if (!device.isOpen) await device.open({});
  // Clear whatever waited, wake the console, and see where it stands.
  await device.read({ waitMs: 0, maxChars: 100 });
  await device.writeText("", true);
  let seen = (await device.read({ waitMs: 3000, until: "ogin:", maxChars: 2000 })).text;
  const prompt = (text: string) => /[$#>] ?$/.test(text.trimEnd());
  if (!/ogin:/.test(seen)) {
    if (prompt(seen)) return { ok: true, output: `Already signed in. The prompt is: ${seen.trimEnd().split("\n").pop()}` };
    if (!seen.trim()) throw new Error("The console printed nothing. Is the port right, and the board on?");
    throw new Error(`No login prompt appeared. The console printed:\n${seen.slice(-500)}`);
  }
  progress(`Signing in as ${user}…`);
  await device.writeText(user, true);
  seen = (await device.read({ waitMs: 5000, until: "assword", maxChars: 2000 })).text;
  if (!/assword/.test(seen)) throw new Error(`No password prompt appeared after the user name. The console printed:\n${seen.slice(-500)}`);
  const password = await secretValue();
  await device.writeText(password, true);
  seen = (await device.read({ waitMs: 6000, until: "$ ", maxChars: 4000 })).text;
  if (/incorrect|denied|failed/i.test(seen)) throw new Error("The console said the sign-in was incorrect. Check the user name and the saved secret.");
  const line = seen.trimEnd().split("\n").pop() || "";
  return { ok: true, output: `Signed in as ${user}. The prompt is: ${line}`, detail: seen.slice(-1500).replace(password, "••••") };
}

/** Carries one request out against an attached device. Exported for tests. */
export async function execute(a: Attached, chatId: string, request: DeviceRequest, progress: (text: string) => void, fetchBytes: (path: string) => Promise<Uint8Array>, secretValue: (name: string) => Promise<string> = () => Promise.reject(new Error("No secrets here"))): Promise<Outcome> {
  const { device } = a;
  const args = request.arguments || {};
  const log: string[] = [];
  const logLine = (line: string) => {
    if (line) log.push(line);
    if (log.length > 40) log.shift();
  };
  switch (request.tool) {
    case "status": {
      const s = device.status();
      return {
        ok: true,
        output: [`Attached in the browser: yes (USB ${hex(a.vendorId)}:${hex(a.productId)}).`, s.open ? `Port open at ${s.baudRate} baud.` : "Port closed.", `Unread output: ${s.unread} characters (${s.total} received since opening).`, device.lastError ? `Last error: ${device.lastError}` : ""]
          .filter(Boolean)
          .join(" "),
      };
    }
    case "open": {
      const options: OpenOptions = {};
      if (typeof args.baudRate === "number") options.baudRate = args.baudRate;
      if (args.dataBits === 7 || args.dataBits === 8) options.dataBits = args.dataBits;
      if (args.stopBits === 1 || args.stopBits === 2) options.stopBits = args.stopBits;
      if (["none", "even", "odd"].includes(args.parity)) options.parity = args.parity;
      if (["none", "hardware"].includes(args.flowControl)) options.flowControl = args.flowControl;
      await device.open(options);
      return { ok: true, output: `Opened at ${device.status().baudRate} baud.` };
    }
    case "close":
      await device.close();
      return { ok: true, output: "Closed." };
    case "read": {
      const { text, dropped, matched } = await device.read({ waitMs: typeof args.waitMs === "number" ? args.waitMs : undefined, until: typeof args.until === "string" && args.until ? args.until : undefined, maxChars: typeof args.maxChars === "number" ? args.maxChars : undefined });
      if (!text) return { ok: true, output: `(no output within ${typeof args.waitMs === "number" ? args.waitMs : 2000} ms)`, detail: "" };
      const notes = [dropped ? `(${dropped} earlier characters dropped)` : "", matched === false ? `(“${args.until}” did not arrive in time)` : ""].filter(Boolean);
      return { ok: true, output: notes.length ? `${notes.join(" ")}\n${text}` : text, detail: text.slice(-4000) };
    }
    case "write": {
      if (typeof args.text !== "string" || !args.text) throw new Error("text is required");
      const bytes = args.hex ? hexBytes(args.text) : new TextEncoder().encode(args.newline === false ? args.text : `${args.text}\n`);
      await device.write(bytes);
      return { ok: true, output: `Sent ${bytes.length} bytes.` };
    }
    case "signals": {
      if (args.pulse === "reset" || args.pulse === "bootloader") {
        await device.pulse(args.pulse);
        return { ok: true, output: args.pulse === "reset" ? "Reset the board." : "Reset the board into its bootloader." };
      }
      const signals: { dataTerminalReady?: boolean; requestToSend?: boolean } = {};
      if (typeof args.dtr === "boolean") signals.dataTerminalReady = args.dtr;
      if (typeof args.rts === "boolean") signals.requestToSend = args.rts;
      if (!Object.keys(signals).length) throw new Error("Name a pulse, or dtr or rts");
      await device.setSignals(signals);
      return { ok: true, output: `Set ${Object.entries(signals).map(([k, v]) => `${k === "dataTerminalReady" ? "DTR" : "RTS"} ${v ? "high" : "low"}`).join(", ")}.` };
    }
    case "login": {
      const user = typeof args.user === "string" && args.user.trim() ? args.user.trim() : a.user;
      const secret = typeof args.secret === "string" && args.secret.trim() ? args.secret.trim() : a.secret;
      if (!user || !secret) throw new Error("No console sign-in is saved for this device. The person can set a user name and a secret for it under Compute › Devices, or name them in the call.");
      return login(a, user, () => secretValue(secret), progress);
    }
    case "identify": {
      progress("Talking to the chip's bootloader…");
      const identity = await device.lend((port) => withFlasher(a.vendorId, (f) => f.identify(port, logLine)));
      const lines = [`${identity.family} ${identity.chip}`, identity.mac ? `MAC ${identity.mac}` : "", identity.flashSize ? `Flash ${identity.flashSize}` : "", identity.crystal ? `Crystal ${identity.crystal}` : "", identity.features?.length ? `Features: ${identity.features.join(", ")}` : ""].filter(Boolean);
      return { ok: true, output: lines.join("\n"), detail: lines.join("\n") };
    }
    case "flash": {
      const given: any[] = Array.isArray(args.parts) ? args.parts : [];
      if (!given.length || given.length > 16) throw new Error("parts must list 1 to 16 images");
      const parts: FlashPart[] = [];
      for (const part of given) {
        const offset = parseOffset(part?.offset);
        const path = typeof part?.path === "string" ? part.path.trim() : "";
        if (offset == null || !path) throw new Error("Each part needs an offset such as 0x10000 and a path in the workspace");
        progress(`Reading ${path} from the workspace…`);
        const data = await fetchBytes(rawPath(chatId, path));
        if (!data.length) throw new Error(`${path} is empty`);
        parts.push({ offset, path, data });
      }
      const total = parts.reduce((n, p) => n + p.data.length, 0);
      progress(`Connecting to the chip (${formatBytes(total)} to write)…`);
      const summary = await device.lend((port) => withFlasher(a.vendorId, (f) => f.flash(port, parts, { eraseAll: !!args.eraseAll, baudRate: typeof args.baudRate === "number" ? args.baudRate : undefined }, progress, logLine)));
      return { ok: true, output: summary, detail: [summary, ...log.slice(-12)].join("\n") };
    }
    case "erase": {
      progress("Erasing the flash…");
      const summary = await device.lend((port) => withFlasher(a.vendorId, (f) => f.erase(port, logLine)));
      return { ok: true, output: summary, detail: [summary, ...log.slice(-12)].join("\n") };
    }
    default:
      throw new Error(`This browser has no device tool named ${request.tool}`);
  }
}

export function createDevicesBridge(ctx: Context): DevicesBridge {
  const serial = ctx.serial === undefined ? (typeof navigator !== "undefined" ? (navigator as Navigator & { serial?: Serial }).serial ?? null : null) : ctx.serial;
  const fetchBytes = ctx.fetchBytes ?? fetchBytesDefault;
  const secretValue = ctx.secretValue ?? (async (name: string) => String((await ctx.api.request(secretPath(name)))?.value ?? ""));
  const attached = new Map<string, Attached>();
  const handled = new Set<string>();
  const waiting = new Map<string, Waiting>();
  const progress = new Map<string, string>();
  const offers = new Map<string, Offer>();
  const listeners = new Set<() => void>();
  let known: DeviceConnection[] = [];
  let restoredAt = 0;
  const changed = () => {
    for (const l of listeners) l();
    ctx.changed();
  };
  const key = (chatId: string, callId: string) => `${chatId}:${callId}`;

  function tie(connection: DeviceConnection, port: SerialPort): Attached {
    const old = attached.get(connection.id);
    if (old?.device.port === port) return old;
    if (old) old.device.close().catch(() => {});
    const info = portInfo(port);
    const a: Attached = { id: connection.id, device: new SerialDevice(port), ...info, user: connection.user, secret: connection.secret };
    a.device.subscribe(changed);
    attached.set(connection.id, a);
    changed();
    for (const w of [...waiting.values()]) if (w.connectionId === connection.id) void run(w);
    return a;
  }

  async function post(chatId: string, callId: string, outcome: Outcome) {
    try {
      await ctx.api.request(resultPath(chatId, callId), { method: "POST", body: { ok: outcome.ok, output: outcome.output, detail: outcome.detail } });
    } catch {
      // The server stopped waiting (the reply ended, or 10 minutes passed).
    }
  }

  async function run(w: Waiting) {
    const k = key(w.chatId, w.callId);
    const a = attached.get(w.connectionId);
    if (!a) {
      if (Date.now() - restoredAt > 5000) await restore().catch(() => {});
      if (!attached.get(w.connectionId)) {
        waiting.set(k, w);
        changed();
        return;
      }
      return run(w);
    }
    waiting.delete(k);
    progress.set(w.callId, "Working…");
    changed();
    let outcome: Outcome;
    try {
      outcome = await execute(a, w.chatId, w.request, (text) => {
        progress.set(w.callId, text);
        changed();
      }, fetchBytes, secretValue);
    } catch (error) {
      outcome = { ok: false, output: (error as Error).message || String(error) };
    }
    progress.delete(w.callId);
    changed();
    await post(w.chatId, w.callId, outcome);
  }

  function consider(chatId: string, step: { id?: string; service?: string; status?: string; connectionId?: string; connection?: string; request?: unknown }) {
    if (!ctx.enabled() || step.service !== "device" || step.status !== "running" || !step.id || !step.connectionId) return;
    const request = step.request as DeviceRequest | undefined;
    if (!request || typeof request !== "object" || typeof request.tool !== "string") return;
    const k = key(chatId, step.id);
    if (handled.has(k)) return;
    handled.add(k);
    if (!serial) {
      void post(chatId, step.id, { ok: false, output: "This browser cannot reach devices: it has no Web Serial support. Chrome and Edge do." });
      return;
    }
    void run({ chatId, callId: step.id, connectionId: step.connectionId, connection: step.connection || "the device", request: { tool: request.tool, arguments: request.arguments && typeof request.arguments === "object" ? request.arguments : {} } });
  }

  async function restore(connections?: DeviceConnection[]) {
    if (!serial) return;
    restoredAt = Date.now();
    if (connections) known = connections;
    else {
      const r = await ctx.api.request("/api/connections");
      known = ((r?.connections ?? []) as any[]).filter((c) => c.kind === "device").map((c) => ({ id: c.id, name: c.name, vendorId: c.vendorId ?? undefined, productId: c.productId ?? undefined, user: c.user || undefined, secret: c.secret || undefined }));
    }
    const ports = await serial.getPorts();
    const used = new Set([...attached.values()].map((a) => a.device.port));
    for (const c of known) {
      if (attached.has(c.id)) continue;
      const port = ports.find((p) => !used.has(p) && (c.vendorId == null || portInfo(p).vendorId === c.vendorId) && (c.productId == null || portInfo(p).productId === c.productId));
      if (!port) continue;
      used.add(port);
      tie(c, port);
    }
  }

  return {
    supported: () => !!serial,
    enabled: ctx.enabled,
    attached: (id) => attached.get(id),
    list: () => [...attached.values()],
    async attach(connection) {
      if (!serial) throw new Error("This browser cannot reach devices. Use Chrome or Edge.");
      const filters = connection.vendorId != null ? [{ usbVendorId: connection.vendorId, ...(connection.productId != null ? { usbProductId: connection.productId } : {}) }] : [];
      const port = await serial.requestPort({ filters });
      return tie(connection, port);
    },
    adopt: tie,
    restore,
    /** The device's saved sign-in changed on its page. */
    update(connection) {
      const a = attached.get(connection.id);
      if (a) Object.assign(a, { user: connection.user, secret: connection.secret });
      known = known.map((c) => (c.id === connection.id ? { ...c, ...connection } : c));
    },
    async detach(id) {
      const a = attached.get(id);
      if (!a) return;
      attached.delete(id);
      await a.device.close().catch(() => {});
      changed();
    },
    async pick() {
      if (!serial) throw new Error("This browser cannot reach devices. Use Chrome or Edge.");
      const port = await serial.requestPort();
      return { port, ...portInfo(port) };
    },
    observe(event, chatId) {
      if (event.event === "tool" && event.data && typeof event.data === "object") consider(chatId, event.data);
    },
    sync(chatId, activity) {
      for (const step of activity) consider(chatId, step);
    },
    progress: (callId) => progress.get(callId) || "",
    offer: (key) => offers.get(key),
    setOffer(key, value) {
      if (value) offers.set(key, value);
      else offers.delete(key);
      changed();
    },
    waiting: (callId) => [...waiting.values()].find((w) => w.callId === callId),
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    reset() {
      handled.clear();
      waiting.clear();
      progress.clear();
      changed();
    },
  };
}
