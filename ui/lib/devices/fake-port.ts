// A Web Serial port for tests: what is written to it is recorded, and the test
// feeds what the "device" prints.

export class FakePort {
  readable: ReadableStream<Uint8Array> | null = null;
  writable: WritableStream<Uint8Array> | null = null;
  written: Uint8Array[] = [];
  signals: { dataTerminalReady?: boolean; requestToSend?: boolean }[] = [];
  opened: SerialOptions | null = null;
  private controller: ReadableStreamDefaultController<Uint8Array> | null = null;
  constructor(private info: { usbVendorId?: number; usbProductId?: number } = {}) {}
  getInfo() {
    return this.info;
  }
  async open(options: SerialOptions) {
    this.opened = options;
    this.readable = new ReadableStream<Uint8Array>({
      start: (controller) => {
        this.controller = controller;
      },
      cancel: () => {
        this.controller = null;
      },
    });
    this.writable = new WritableStream<Uint8Array>({
      write: (chunk) => {
        this.written.push(chunk);
      },
    });
  }
  async close() {
    this.readable = null;
    this.writable = null;
    this.controller = null;
  }
  async setSignals(signals: { dataTerminalReady?: boolean; requestToSend?: boolean }) {
    this.signals.push(signals);
  }
  /** The device prints this. */
  print(text: string) {
    this.controller?.enqueue(new TextEncoder().encode(text));
  }
  /** Everything written, as text. */
  sent() {
    return this.written.map((w) => new TextDecoder().decode(w)).join("");
  }
}

export const asPort = (p: FakePort) => p as unknown as SerialPort;
