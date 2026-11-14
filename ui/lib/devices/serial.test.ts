import { asPort, FakePort } from "./fake-port";
import { hexBytes, SerialDevice } from "./serial";

test("a device collects what it prints and reads it once, waiting for a marker", async () => {
  const port = new FakePort({ usbVendorId: 0x303a, usbProductId: 0x1001 });
  const device = new SerialDevice(asPort(port));
  expect(device.status()).toEqual({ open: false, baudRate: null, unread: 0, total: 0 });
  await device.open({ baudRate: 9600 });
  expect(port.opened).toMatchObject({ baudRate: 9600 });
  expect(device.status().open).toBe(true);
  port.print("boot\r\n");
  setTimeout(() => port.print("ready>"), 20);
  const first = await device.read({ until: "ready>", waitMs: 2000 });
  expect(first).toEqual({ text: "boot\nready>", dropped: 0, matched: true });
  // Read once: the next read sees only what came after.
  const empty = await device.read({ waitMs: 10 });
  expect(empty).toEqual({ text: "", dropped: 0, matched: undefined });
  port.print("x".repeat(50));
  const cut = await device.read({ waitMs: 10, maxChars: 20 });
  expect(cut.text).toBe("x".repeat(20));
  expect(cut.dropped).toBe(30);
  expect(device.recent()).toContain("boot");
  // Colour codes are cleaned out, and one split across two reads doesn't leak its tail.
  port.print("\x1b[01;32mdebian@bb\x1b[00m:~$ \x1b[3");
  expect((await device.read({ until: "bb:~$ ", waitMs: 500 })).text).toBe("debian@bb:~$ ");
  port.print("3mok\x1b[0m");
  expect((await device.read({ waitMs: 500 })).text).toBe("ok");
  await device.writeText("AT");
  await device.write(hexBytes("A5 ff"));
  expect(port.written.map((w) => [...w])).toEqual([[65, 84, 10], [0xa5, 0xff]]);
  await device.pulse("bootloader");
  expect(port.signals).toEqual([
    { dataTerminalReady: false, requestToSend: true },
    { dataTerminalReady: true, requestToSend: false },
    { dataTerminalReady: false, requestToSend: false },
  ]);
  await device.close();
  expect(device.status().open).toBe(false);
  await expect(device.read()).rejects.toThrow("not open");
});

test("lending the port to a flasher closes it and reopens it the same way", async () => {
  const port = new FakePort();
  const device = new SerialDevice(asPort(port));
  await device.open({ baudRate: 74880 });
  const seen = await device.lend(async (p) => {
    expect(p.readable).toBeNull();
    expect(device.busy).toBe(true);
    return "flashed";
  });
  expect(seen).toBe("flashed");
  expect(device.busy).toBe(false);
  expect(device.status()).toMatchObject({ open: true, baudRate: 74880 });
  expect(() => hexBytes("abc")).toThrow("even number");
});
