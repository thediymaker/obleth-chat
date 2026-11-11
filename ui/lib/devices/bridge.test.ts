import { vi } from "vitest";
import { createDevicesBridge, execute, type DevicesBridge } from "./bridge";
import { asPort, FakePort } from "./fake-port";
import { SerialDevice } from "./serial";

/** A Web Serial API whose chooser hands out the given ports. */
function fakeSerial(ports: FakePort[]) {
  return {
    getPorts: async () => ports.map(asPort),
    requestPort: async () => asPort(ports[0]),
  } as unknown as Serial;
}

function bridge(serial: Serial | null, requests: { path: string; body?: any }[] = [], connections: any[] = []): DevicesBridge {
  return createDevicesBridge({
    api: {
      request: async (path: string, options: any = {}) => {
        requests.push({ path, body: options.body });
        if (path === "/api/connections") return { connections };
        return {};
      },
    },
    enabled: () => true,
    changed: () => {},
    serial,
    fetchBytes: async (path) => new TextEncoder().encode(`bytes of ${path}`),
  });
}

const step = (id: string, request: any, connectionId = "dev1") => ({ id, service: "device", status: "running", connectionId, connection: "Board", request });

test("a device step is carried out on the attached port and its result posted", async () => {
  const port = new FakePort({ usbVendorId: 0x303a, usbProductId: 0x1001 });
  const requests: { path: string; body?: any }[] = [];
  const b = bridge(fakeSerial([port]), requests);
  b.adopt({ id: "dev1", name: "Board" }, asPort(port));
  b.observe({ event: "tool", data: step("call_open", { tool: "open", arguments: { baudRate: 115200 } }) }, "chat1");
  await vi.waitFor(() => expect(requests.find((r) => r.path.endsWith("/call_open"))).toBeTruthy());
  expect(requests.at(-1)).toEqual({ path: "/api/conversations/chat1/device-calls/call_open", body: { ok: true, output: "Opened at 115200 baud.", detail: undefined } });
  port.print("hello\n");
  b.sync("chat1", [step("call_read", { tool: "read", arguments: { waitMs: 50 } })]);
  await vi.waitFor(() => expect(requests.find((r) => r.path.endsWith("/call_read"))).toBeTruthy());
  expect(requests.at(-1)!.body).toMatchObject({ ok: true, output: "hello\n" });
  // The same step seen again (a reconcile) is not run twice.
  b.sync("chat1", [step("call_read", { tool: "read", arguments: { waitMs: 50 } })]);
  b.observe({ event: "tool", data: step("call_write", { tool: "write", arguments: { text: "AT" } }) }, "chat1");
  await vi.waitFor(() => expect(requests.find((r) => r.path.endsWith("/call_write"))).toBeTruthy());
  expect(port.sent()).toBe("AT\n");
  expect(requests.filter((r) => r.path.endsWith("/call_read"))).toHaveLength(1);
  // A finished step, or a step of another service, is ignored.
  b.observe({ event: "tool", data: { ...step("call_done", { tool: "read", arguments: {} }), status: "completed" } }, "chat1");
  b.observe({ event: "tool", data: { ...step("call_mcp", { tool: "read", arguments: {} }), service: "mcp" } }, "chat1");
  await new Promise((r) => setTimeout(r, 20));
  expect(requests.some((r) => r.path.endsWith("/call_done") || r.path.endsWith("/call_mcp"))).toBe(false);
  // A tool this browser does not know fails cleanly.
  b.observe({ event: "tool", data: step("call_odd", { tool: "levitate", arguments: {} }) }, "chat1");
  await vi.waitFor(() => expect(requests.find((r) => r.path.endsWith("/call_odd"))).toBeTruthy());
  expect(requests.at(-1)!.body).toMatchObject({ ok: false, output: "This browser has no device tool named levitate" });
});

test("a step for a device not attached here waits, then runs once the person attaches it", async () => {
  const port = new FakePort({ usbVendorId: 0x1a86, usbProductId: 0x7523 });
  const requests: { path: string; body?: any }[] = [];
  const b = bridge(fakeSerial([]), requests);
  b.observe({ event: "tool", data: step("call_status", { tool: "status", arguments: {} }) }, "chat1");
  await vi.waitFor(() => expect(b.waiting("call_status")).toBeTruthy());
  expect(b.waiting("call_status")).toMatchObject({ connectionId: "dev1", connection: "Board" });
  b.adopt({ id: "dev1", name: "Board" }, asPort(port));
  await vi.waitFor(() => expect(requests.find((r) => r.path.endsWith("/call_status"))).toBeTruthy());
  expect(b.waiting("call_status")).toBeUndefined();
  expect(requests.at(-1)!.body.output).toContain("Attached in the browser: yes (USB 0x1a86:0x7523). Port closed.");
});

test("granted ports are tied back to their devices by USB ids", async () => {
  const esp = new FakePort({ usbVendorId: 0x303a, usbProductId: 0x1001 });
  const ftdi = new FakePort({ usbVendorId: 0x0403, usbProductId: 0x6001 });
  const requests: { path: string; body?: any }[] = [];
  const b = bridge(fakeSerial([ftdi, esp]), requests, [
    { id: "a", kind: "device", name: "ESP", vendorId: 0x303a, productId: 0x1001 },
    { id: "b", kind: "device", name: "Probe", vendorId: 0x0403, productId: 0x6001 },
    { id: "m", kind: "mcp", name: "Docs" },
  ]);
  // A step arrives before anything is attached: the bridge looks the ports up itself.
  b.observe({ event: "tool", data: step("call_s", { tool: "status", arguments: {} }, "a") }, "chat1");
  await vi.waitFor(() => expect(requests.find((r) => r.path.endsWith("/call_s"))).toBeTruthy());
  expect(b.attached("a")?.device.port).toBe(asPort(esp));
  expect(b.attached("b")?.device.port).toBe(asPort(ftdi));
  expect(requests[0].path).toBe("/api/connections");
});

test("a browser without Web Serial answers every device step with why", async () => {
  const requests: { path: string; body?: any }[] = [];
  const b = bridge(null, requests);
  expect(b.supported()).toBe(false);
  b.observe({ event: "tool", data: step("call_x", { tool: "status", arguments: {} }) }, "chat1");
  await vi.waitFor(() => expect(requests.find((r) => r.path.endsWith("/call_x"))).toBeTruthy());
  expect(requests.at(-1)!.body).toMatchObject({ ok: false, output: expect.stringContaining("no Web Serial support") });
  await expect(b.attach({ id: "dev1" })).rejects.toThrow("Chrome or Edge");
});

test("flash reads each image from the workspace and refuses bad parts before touching the port", async () => {
  const port = new FakePort();
  const a = { id: "dev1", device: new SerialDevice(asPort(port)), vendorId: 0x303a };
  const fetched: string[] = [];
  const fetchBytes = async (path: string) => {
    fetched.push(path);
    return new Uint8Array([1, 2, 3]);
  };
  await expect(execute(a, "chat1", { tool: "flash", arguments: { parts: [] } }, () => {}, fetchBytes)).rejects.toThrow("1 to 16 images");
  await expect(execute(a, "chat1", { tool: "flash", arguments: { parts: [{ offset: "app", path: "a.bin" }] } }, () => {}, fetchBytes)).rejects.toThrow("offset such as 0x10000");
  expect(fetched).toEqual([]);
  // Images are read before the port is touched; a workspace that is off ends the call there.
  const progress: string[] = [];
  const failing = async (path: string) => {
    fetched.push(path);
    throw new Error("The workspace is not running");
  };
  await expect(execute(a, "chat1", { tool: "flash", arguments: { parts: [{ offset: "0x10000", path: "build/app.bin" }] } }, (t) => progress.push(t), failing)).rejects.toThrow("The workspace is not running");
  expect(fetched).toEqual(["/api/conversations/chat1/workspace/raw?path=build%2Fapp.bin"]);
  expect(progress).toEqual(["Reading build/app.bin from the workspace…"]);
  expect(port.opened).toBeNull();
});
