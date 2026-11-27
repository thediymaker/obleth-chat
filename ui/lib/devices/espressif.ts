// The Espressif flasher: ESP32, S2, S3, C3, C6, H2 and the rest, through
// esptool-js (Espressif's own flasher for browsers). Loaded only when a device
// tool first needs it.

import { ESPLoader, Transport, type IEspLoaderTerminal } from "esptool-js";
import { type FlashOptions, type FlashPart, type Flasher, type Identity } from "./flashers";
import { bytes } from "@/lib/format";

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
      // One at a time: the bootloader answers one command on the one serial line.
      const features = await rom.getChipFeatures(loader).catch(() => [] as string[]);
      const mac = await rom.readMac(loader).catch(() => undefined);
      const crystal = await rom.getCrystalFreq(loader).catch(() => undefined);
      const flashSize = await loader.detectFlashSize().catch(() => undefined);
      return { family: "Espressif", chip, mac, flashSize, features, crystal: crystal ? `${crystal} MHz` : undefined };
    });
  },

  async flash(port, parts: FlashPart[], options: FlashOptions, progress, log) {
    const baudrate = options.baudRate ?? 460800;
    const started = Date.now();
    const total = parts.reduce((n, p) => n + p.data.length, 0);
    return withLoader(port, baudrate, log, async (loader, chip) => {
      progress(`Connected to ${chip}. Writing ${parts.length} image${parts.length === 1 ? "" : "s"}, ${bytes(total)}…`);
      await loader.writeFlash({
        fileArray: parts.map((p) => ({ data: p.data, address: p.offset })),
        flashSize: "keep",
        flashMode: "keep",
        flashFreq: "keep",
        eraseAll: !!options.eraseAll,
        compress: true,
        reportProgress: (index, written, size) => {
          const part = parts[index];
          progress(`Writing ${part.path} at 0x${part.offset.toString(16)}: ${size > 0 ? Math.round((written / size) * 100) : 100}%`);
        },
      });
      const seconds = ((Date.now() - started) / 1000).toFixed(1);
      return `Wrote ${parts.length} image${parts.length === 1 ? "" : "s"} (${bytes(total)}) to ${chip} in ${seconds} s, then restarted the board.`;
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
