// Flashers: one per chip family, each knowing how to identify a chip over its
// port and write images to its flash. A device's flasher is picked by the USB
// vendor id the browser read off the port, or by trying each in turn. Adding a
// family means adding one module here; the tools the model sees stay the same.

export interface FlashPart {
  offset: number;
  path: string;
  data: Uint8Array;
}

export interface FlashOptions {
  eraseAll?: boolean;
  baudRate?: number;
}

export interface Identity {
  family: string;
  chip: string;
  mac?: string;
  flashSize?: string;
  features?: string[];
  crystal?: string;
}

export interface Flasher {
  id: string;
  name: string;
  /** USB vendor ids whose devices this flasher handles first. */
  vendorIds: number[];
  identify(port: SerialPort, log: (line: string) => void): Promise<Identity>;
  flash(port: SerialPort, parts: FlashPart[], options: FlashOptions, progress: (text: string) => void, log: (line: string) => void): Promise<string>;
  erase(port: SerialPort, log: (line: string) => void): Promise<string>;
}

/** The flashers in the order they are tried. Each loads only when first used. */
const LOADERS: { id: string; vendorIds: number[]; load: () => Promise<Flasher> }[] = [{ id: "espressif", vendorIds: [0x303a], load: () => import("./espressif").then((m) => m.espressif) }];

export const FAMILIES = LOADERS.map((l) => l.id);

/** The flashers to try for a port: the vendor's own first, then the rest. */
export async function flashersFor(vendorId?: number): Promise<Flasher[]> {
  const ordered = [...LOADERS].sort((a, b) => Number(b.vendorIds.includes(vendorId ?? -1)) - Number(a.vendorIds.includes(vendorId ?? -1)));
  return Promise.all(ordered.map((l) => l.load()));
}

/** Runs `work` with the first flasher that recognises the chip. */
export async function withFlasher<T>(vendorId: number | undefined, work: (flasher: Flasher) => Promise<T>): Promise<T> {
  const flashers = await flashersFor(vendorId);
  const errors: string[] = [];
  for (const flasher of flashers) {
    try {
      return await work(flasher);
    } catch (error) {
      errors.push(`${flasher.name}: ${(error as Error).message || String(error)}`);
    }
  }
  throw new Error(`No supported chip answered. ${errors.join(" · ")}`);
}

export function formatBytes(n: number) {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(2)} MB`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${n} bytes`;
}
