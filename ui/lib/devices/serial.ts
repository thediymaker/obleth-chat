// One serial device at the person's computer, reached with the browser's Web Serial
// API: open at a baud rate, collect what it prints, send to it, drive its DTR and
// RTS lines, and hand the port over (and back) when a flasher needs it whole.

export interface OpenOptions {
  baudRate?: number;
  dataBits?: 7 | 8;
  stopBits?: 1 | 2;
  parity?: "none" | "even" | "odd";
  flowControl?: "none" | "hardware";
}

export interface SerialStatus {
  open: boolean;
  baudRate: number | null;
  /** Characters collected and not yet read. */
  unread: number;
  /** Characters collected since the port was opened. */
  total: number;
}

export const DEFAULT_BAUD = 115200;
/** How much output is kept for reading; older output is dropped from the front. */
const KEEP = 256 * 1024;
/** How much the monitor keeps for showing. */
const MONITOR_KEEP = 64 * 1024;

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** The id reported for a port, from the USB ids the browser knows. */
export function portInfo(port: SerialPort): { vendorId?: number; productId?: number } {
  const info = port.getInfo();
  return { vendorId: info.usbVendorId, productId: info.usbProductId };
}

export class SerialDevice {
  readonly port: SerialPort;
  private reader: ReadableStreamDefaultReader<Uint8Array> | null = null;
  private readLoop: Promise<void> | null = null;
  private decoder = new TextDecoder();
  /** Output not yet read by a tool. */
  private unread = "";
  /** Output kept for the monitor. */
  private monitor = "";
  private total = 0;
  private baud: number | null = null;
  private options: OpenOptions = {};
  private waiters = new Set<() => void>();
  private listeners = new Set<() => void>();
  /** Set while a flasher owns the port. */
  busy = false;
  lastError = "";

  constructor(port: SerialPort) {
    this.port = port;
  }

  get isOpen() {
    return this.port.readable != null && !this.busy;
  }

  status(): SerialStatus {
    return { open: this.isOpen, baudRate: this.isOpen ? this.baud : null, unread: this.unread.length, total: this.total };
  }

  /** The monitor's view of recent output. */
  recent() {
    return this.monitor;
  }

  /** Listens for new output and state changes. */
  subscribe(listener: () => void) {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
  private changed() {
    for (const l of this.listeners) l();
  }

  async open(options: OpenOptions = {}) {
    if (this.busy) throw new Error("The port is busy with a flasher");
    const baudRate = options.baudRate ?? this.baud ?? DEFAULT_BAUD;
    if (this.port.readable) await this.close();
    await this.port.open({ baudRate, dataBits: options.dataBits, stopBits: options.stopBits, parity: options.parity, flowControl: options.flowControl });
    this.baud = baudRate;
    this.options = { ...options, baudRate };
    this.unread = "";
    this.total = 0;
    this.lastError = "";
    this.startReading();
    this.changed();
  }

  private startReading() {
    if (!this.port.readable) return;
    const reader = this.port.readable.getReader();
    this.reader = reader;
    this.readLoop = (async () => {
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          if (value) this.receive(value);
        }
      } catch (error) {
        // The device was unplugged, or the port closed under us.
        this.lastError = (error as Error).message || "The port stopped";
      } finally {
        reader.releaseLock();
        if (this.reader === reader) this.reader = null;
        this.changed();
      }
    })();
  }

  private receive(bytes: Uint8Array) {
    const text = this.decoder.decode(bytes, { stream: true });
    if (!text) return;
    this.total += text.length;
    this.unread = (this.unread + text).slice(-KEEP);
    this.monitor = (this.monitor + text).slice(-MONITOR_KEEP);
    for (const w of this.waiters) w();
    this.changed();
  }

  async close() {
    const reader = this.reader;
    if (reader) {
      await reader.cancel().catch(() => {});
      await this.readLoop?.catch(() => {});
    }
    if (this.port.readable || this.port.writable) await this.port.close().catch(() => {});
    this.changed();
  }

  /** What arrived since the last read, waiting up to `waitMs` for more or for `until`. */
  async read({ waitMs = 2000, until, maxChars = 6000 }: { waitMs?: number; until?: string; maxChars?: number } = {}) {
    if (!this.isOpen) throw new Error("The port is not open. Open it first.");
    const deadline = Date.now() + Math.min(Math.max(waitMs, 0), 60000);
    const satisfied = () => (until ? this.unread.includes(until) : this.unread.length > 0);
    while (!satisfied() && Date.now() < deadline) {
      await new Promise<void>((resolve) => {
        const waiter = () => {
          this.waiters.delete(waiter);
          resolve();
        };
        this.waiters.add(waiter);
        setTimeout(waiter, Math.max(0, Math.min(250, deadline - Date.now())));
      });
    }
    let text = this.unread;
    this.unread = "";
    const dropped = Math.max(0, text.length - maxChars);
    if (dropped) text = text.slice(-maxChars);
    return { text, dropped, matched: until ? text.includes(until) : undefined };
  }

  async write(bytes: Uint8Array) {
    if (!this.isOpen || !this.port.writable) throw new Error("The port is not open. Open it first.");
    const writer = this.port.writable.getWriter();
    try {
      await writer.write(bytes);
    } finally {
      writer.releaseLock();
    }
    this.changed();
  }

  async writeText(text: string, newline = true) {
    await this.write(new TextEncoder().encode(newline ? `${text}\n` : text));
  }

  async setSignals(signals: { dataTerminalReady?: boolean; requestToSend?: boolean }) {
    if (!this.port.readable) throw new Error("The port is not open. Open it first.");
    await this.port.setSignals(signals);
  }

  /** Restarts the board the way USB serial adapters wire it: RTS to reset (EN), DTR to the boot pin. */
  async pulse(mode: "reset" | "bootloader") {
    if (mode === "bootloader") {
      await this.setSignals({ dataTerminalReady: false, requestToSend: true });
      await sleep(100);
      await this.setSignals({ dataTerminalReady: true, requestToSend: false });
      await sleep(50);
      await this.setSignals({ dataTerminalReady: false, requestToSend: false });
    } else {
      await this.setSignals({ dataTerminalReady: false, requestToSend: true });
      await sleep(100);
      await this.setSignals({ dataTerminalReady: false, requestToSend: false });
    }
  }

  /**
   * Hands the closed port to a flasher, and reopens it as it was afterwards. The
   * flasher's own output goes through `log`.
   */
  async lend<T>(work: (port: SerialPort) => Promise<T>): Promise<T> {
    if (this.busy) throw new Error("The port is busy with a flasher");
    const wasOpen = this.isOpen;
    const options = this.options;
    await this.close();
    this.busy = true;
    this.changed();
    try {
      return await work(this.port);
    } finally {
      this.busy = false;
      if (this.port.readable) await this.port.close().catch(() => {});
      if (wasOpen) await this.open(options).catch((error) => (this.lastError = (error as Error).message));
      this.changed();
    }
  }
}

/** Bytes from a hex string such as "A5 01 ff" or "a501ff". */
export function hexBytes(text: string): Uint8Array {
  const clean = text.replace(/0x/gi, "").replace(/[^0-9a-f]/gi, "");
  if (!clean || clean.length % 2) throw new Error("hex needs an even number of hex digits");
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return out;
}
