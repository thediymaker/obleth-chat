import { screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { DevicesPage } from "./devices-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
vi.mock("next/navigation", () => ({ usePathname: () => "/devices", useRouter: () => ({ push: vi.fn() }) }));

beforeEach(() => {
  fakeStore();
});

test("with devices off, the page keeps the Compute heading and tabs and says devices are off", async () => {
  mockApi(() => ({ connections: [] }), { workspaces: true, clusters: true, devices: false });
  renderPage(<DevicesPage />);
  expect(await screen.findByRole("heading", { name: "Devices are turned off on this server" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "Compute" })).toBeInTheDocument();
  const tabs = screen.getByRole("navigation", { name: "Compute" });
  expect(within(tabs).getByRole("link", { name: "Workspaces" })).toHaveAttribute("href", "/workspaces");
});

test("with devices on, the heading is Compute and the action is Add device", async () => {
  mockApi(() => ({ connections: [] }), { workspaces: true, connections: true, devices: true });
  renderPage(<DevicesPage />);
  expect(await screen.findByText("No devices yet")).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "Compute" })).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Add device" }).length).toBe(2);
});
