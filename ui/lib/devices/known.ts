// USB vendors seen on development boards, for naming a device the person just
// picked. Anything else is named by its ids.

const VENDORS: Record<number, string> = {
  0x303a: "Espressif",
  0x10c4: "Silicon Labs CP210x",
  0x1a86: "WCH CH340",
  0x0403: "FTDI",
  0x067b: "Prolific",
  0x2341: "Arduino",
  0x2a03: "Arduino",
  0x1b4f: "SparkFun",
  0x239a: "Adafruit",
  0x2e8a: "Raspberry Pi Pico",
  0x0483: "STMicroelectronics",
  0x1366: "SEGGER J-Link",
  0x0d28: "DAPLink",
  0x16c0: "Teensy",
  0x1209: "Open hardware",
  0x1915: "Nordic Semiconductor",
  0x04d8: "Microchip",
};

export const vendorName = (vendorId?: number) => (vendorId != null ? VENDORS[vendorId] : undefined);

/** A name for a new device, from its USB ids: "Espressif board", "FTDI serial". */
export function suggestName(vendorId?: number, productId?: number) {
  const vendor = vendorName(vendorId);
  if (vendor) return vendor === "Espressif" || vendor === "Arduino" || vendor === "Raspberry Pi Pico" || vendor === "Adafruit" || vendor === "SparkFun" || vendor === "Teensy" ? `${vendor} board` : `${vendor} serial`;
  if (vendorId != null) return `USB ${vendorId.toString(16).padStart(4, "0")}:${(productId ?? 0).toString(16).padStart(4, "0")}`;
  return "Serial device";
}

/** What the browser can say about the port, kept as the connection's label. */
export function portLabel(vendorId?: number, productId?: number) {
  const vendor = vendorName(vendorId);
  const ids = vendorId != null ? `${vendorId.toString(16).padStart(4, "0")}:${(productId ?? 0).toString(16).padStart(4, "0")}` : "";
  return [vendor, ids ? `USB ${ids}` : ""].filter(Boolean).join(" · ") || "serial port";
}
