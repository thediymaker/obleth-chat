import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { asPort, FakePort } from "@/lib/devices/fake-port";
import { DeviceCard } from "./device-card";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));

const request = { id: "m1", role: "assistant", content: "", status: "completed", activity: [{ kind: "device-request", reason: "To read the console.", name: "BeagleBone" }] };

beforeEach(() => {
  fakeStore();
});

describe("the device card", () => {
  it("attaches a device through the browser's chooser, names it and tells the model to go ahead", async () => {
    const store = fakeStore();
    const port = new FakePort({ usbVendorId: 0x1d6b, usbProductId: 0x0104 });
    vi.spyOn(store.devices, "supported").mockReturnValue(true);
    vi.spyOn(store.devices, "pick").mockResolvedValue({ port: asPort(port), vendorId: 0x1d6b, productId: 0x0104 });
    const calls = mockApi((path, o) => (path === "/api/connections" && o.method === "POST" ? { id: "d1", name: (o.body as any).name, kind: "device", tools: [] } : {}));
    renderPage(<DeviceCard m={request} index={0} isLast streaming={false} />);
    expect(screen.getByText("This needs a device at your computer")).toBeInTheDocument();
    expect(screen.getByText("To read the console.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Attach device" }));
    const name = await screen.findByLabelText("Name");
    expect(name).toHaveValue("BeagleBone");
    await userEvent.click(screen.getByRole("button", { name: "Add device" }));
    await waitFor(() => expect(store.send).toHaveBeenCalledWith("BeagleBone is attached. Go ahead."));
    expect(calls).toContainEqual({ path: "/api/connections", method: "POST", body: { kind: "device", name: "BeagleBone", url: "", confirmActions: true, options: { transport: "serial", vendorId: 0x1d6b, productId: 0x0104, label: "USB 1d6b:0104" } } });
    expect(store.devices.attached("d1")?.device.port).toBe(asPort(port));
    expect(await screen.findByText("Attached for your chats.")).toBeInTheDocument();
  });

  it("closes with Not now, says when the browser cannot do it, and shows nothing on older replies", () => {
    renderPage(<DeviceCard m={request} index={0} isLast streaming={false} />);
    expect(screen.getByText(/no Web Serial support/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Attach device" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.getByText(/Not attached. Add it under Compute/)).toBeInTheDocument();
    const { container } = renderPage(<DeviceCard m={{ role: "assistant", content: "hi" }} index={1} isLast streaming={false} />);
    expect(container).toBeEmptyDOMElement();
  });
});
