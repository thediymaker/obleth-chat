import { screen } from "@testing-library/react";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";
import { ApiError } from "@/lib/api";

const request = vi.fn();
vi.mock("@/lib/api", async (orig) => ({ ...(await orig<typeof import("@/lib/api")>()), api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false }), useUser: () => ({ id: "u1", role: "user" }), useSignedIn: () => true }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), usePathname: () => "/projects/p9", useParams: () => ({ id: "p9" }), useSearchParams: () => new URLSearchParams() }));
vi.mock("@/components/app-shell/chrome", () => ({ usePageChrome: () => {} }));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));

import { ProjectFrame } from "./project-frame";

polyfillLayout();

test("a project that is not there shows the not-found block, with no Retry", async () => {
  request.mockImplementation(async (path: string) => {
    if (path.startsWith("/api/projects/p9")) throw new ApiError("Not found", 404);
    if (path === "/api/capabilities") return { application: "chat", mode: "connected", features: {} };
    return {};
  });
  renderWithQueries(
    <ProjectFrame>
      <p>inside</p>
    </ProjectFrame>,
  );
  expect(await screen.findByRole("heading", { name: "Project not found" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /Back to all projects/ })).toHaveAttribute("href", "/projects");
  expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument();
  expect(screen.queryByText("inside")).not.toBeInTheDocument();
});
