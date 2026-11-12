// The Espressif flasher: ESP32, S2, S3, C3, C6, H2 and the rest, through
// esptool-js (Espressif's own flasher for browsers). Loaded only when a device
// tool first needs it.

import { ESPLoader, Transport, type IEspLoaderTerminal } from "esptool-js";
import { formatBytes, type FlashOptions, type FlashPart, type Flasher, type Identity } from "./flashers";

const ROM_BAUD = 115200;

function terminal(log: (line: string) => void): IEspLoaderTerminal {
  let partial = "";
  return {
    clean: () => {},
    write: (data: string) => {
      partial += data;
    },
    writeLine: (data: string) => {
      log((partial + data).trim());
      partial = "";
    },
  };
}

/** Connects to the ROM bootloader and runs `work`, then restarts the board and frees the port. */
async function withLoader<T>(port: SerialPort, baudrate: number, log: (line: string) => void, work: (loader: ESPLoader, chip: string) => Promise<T>): Promise<T> {
  const transport = new Transport(port, false);
  const loader = new ESPLoader({ transport, baudrate, romBaudrate: ROM_BAUD, terminal: terminal(log) });
  let connected = false;
  try {
    const chip = await loader.main();
    connected = true;
    return await work(loader, chip);
  } finally {
    if (connected) await loader.after("hard_reset").catch(() => {});
    await transport.disconnect().catch(() => {});
  }
}

export const espressif: Flasher = {
  id: "espressif",
  name: "Espressif",
  vendorIds: [0x303a],

  async identify(port, log): Promise<Identity> {
    return withLoader(port, ROM_BAUD, log, async (loader, chip) => {
      const rom = loader.chip;
      const [features, mac, crystal, flashSize] = await Promise.all([
        rom.getChipFeatures(loader).catch(() => [] as string[]),
        rom.readMac(loader).catch(() => undefined),
        rom.getCrystalFreq(loader).catch(() => undefined),
        loader.detectFlashSize().catch(() => undefined),
      ]);
      return { family: "Espressif", chip, mac, flashSize, features, crystal: crystal ? `${crystal} MHz` : undefined };
    });
  },

  async flash(port, parts: FlashPart[], options: FlashOptions, progress, log) {
    const baudrate = options.baudRate ?? 460800;
    const started = Date.now();
    const total = parts.reduce((n, p) => n + p.data.length, 0);
    return withLoader(port, baudrate, log, async (loader, chip) => {
      progress(`Connected to ${chip}. Writing ${parts.length} image${parts.length === 1 ? "" : "s"}, ${formatBytes(total)}…`);
      await loader.writeFlash({
        fileArray: parts.map((p) => ({ data: p.data, address: p.offset })),
        flashSize: "keep",
        flashMode: "keep",
        flashFreq: "keep",
        eraseAll: !!options.eraseAll,
        compress: true,
        reportProgress: (index, written, size) => {
          const part = parts[index];
          progress(`Writing ${part.path} at 0x${part.offset.toString(16)}: ${Math.round((written / size) * 100)}%`);
        },
      });
      const seconds = ((Date.now() - started) / 1000).toFixed(1);
      return `Wrote ${parts.length} image${parts.length === 1 ? "" : "s"} (${formatBytes(total)}) to ${chip} in ${seconds} s, then restarted the board.`;
    });
  },

  async erase(port, log) {
    return withLoader(port, ROM_BAUD, log, async (loader, chip) => {
      const started = Date.now();
      await loader.eraseFlash();
      return `Erased the flash of ${chip} in ${((Date.now() - started) / 1000).toFixed(1)} s, then restarted the board.`;
    });
  },
};
