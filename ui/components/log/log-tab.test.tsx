import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";

const request = vi.fn();
vi.mock("@/lib/api", async (orig) => ({ ...(await orig<typeof import("@/lib/api")>()), api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false }), useUser: () => ({ id: "u1", role: "user" }), useSignedIn: () => true }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), usePathname: () => "/projects/p9", useParams: () => ({ id: "p9" }), useSearchParams: () => new URLSearchParams() }));
vi.mock("@/components/app-shell/chrome", () => ({ usePageChrome: () => {} }));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
vi.mock("sonner", () => ({ toast: vi.fn() }));

import { ProjectFrame } from "@/components/projects/project-frame";
import { LogSwitchCard } from "./log-switch";

polyfillLayout();

const project = { id: "p9", name: "Batching paper", description: "", instructions: "", color: "blue", scope: "personal", groupId: null, groupName: null, mine: true, ownerName: "Ana", chats: 2, saved: 0, updatedAt: "2026-11-17T10:00:00Z" };

function serve(features: Record<string, boolean>) {
  request.mockImplementation(async (path: string) => {
    if (path === "/api/capabilities") return { application: "chat", mode: "connected", features };
    if (path === "/api/projects/p9") return { project, collections: [], chats: [], saved: [], app: null };
    return { available: true, enabled: false, timeZone: null };
  });
}

beforeEach(() => request.mockReset());

test("the Log tab shows while the log is on for this server and the person", async () => {
  serve({ log: true, myLog: true, code: true });
  renderWithQueries(
    <ProjectFrame>
      <p>inside</p>
    </ProjectFrame>,
  );
  const tab = await screen.findByRole("link", { name: "Log" });
  expect(tab).toHaveAttribute("href", "/projects/p9/log");
  // Right after Overview.
  const names = screen.getAllByRole("link").map((l) => l.textContent);
  expect(names.indexOf("Log")).toBe(names.indexOf("Overview") + 1);
});

test("the Log tab is hidden when the person turned their log off", async () => {
  serve({ log: true, myLog: false, code: true });
  renderWithQueries(
    <ProjectFrame>
      <p>inside</p>
    </ProjectFrame>,
  );
  expect(await screen.findByRole("link", { name: "Code" })).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Log" })).not.toBeInTheDocument();
});

test("a person turns their own log off in Settings, only while the server has it", async () => {
  serve({ log: true, myLog: true });
  renderWithQueries(<LogSwitchCard />);
  const toggle = await screen.findByRole("switch", { name: "Write my project log" });
  expect(toggle).toBeChecked();
  await userEvent.click(toggle);
  await waitFor(() => expect(request).toHaveBeenCalledWith("/api/log/settings", { method: "PUT", body: { enabled: false } }));
  expect(toggle).not.toBeChecked();
});

test("there is no switch when the log is off on this server", async () => {
  serve({ log: false, myLog: false });
  const { container } = renderWithQueries(<LogSwitchCard />);
  await waitFor(() => expect(request).toHaveBeenCalledWith("/api/capabilities"));
  expect(container).toBeEmptyDOMElement();
});
