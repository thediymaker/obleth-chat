import { screen } from "@testing-library/react";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";
import { ApiError } from "@/lib/api";

const request = vi.fn();
vi.mock("@/lib/api", async (orig) => ({ ...(await orig<typeof import("@/lib/api")>()), api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false }), useUser: () => ({ id: "u1", role: "user" }), useSignedIn: () => true }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), usePathname: () => "/knowledge/k9", useSearchParams: () => new URLSearchParams() }));
vi.mock("@/components/app-shell/chrome", () => ({ usePageChrome: () => {} }));

import { CollectionPage } from "./collection-page";

polyfillLayout();

test("a collection that is not there shows the not-found block, with no Retry", async () => {
  request.mockImplementation(async (path: string) => {
    if (path.startsWith("/api/knowledge/collections/k9")) throw new ApiError("Not found", 404);
    if (path.startsWith("/api/knowledge/collections")) return { collections: [], usage: { used: 0, limit: 1 }, groups: [], maxFileBytes: 1 };
    return {};
  });
  renderWithQueries(<CollectionPage id="k9" />);
  expect(await screen.findByRole("heading", { name: "Collection not found" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /Back to all collections/ })).toHaveAttribute("href", "/knowledge");
  expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument();
});

test("any other failure keeps the error box and its Retry", { timeout: 15_000 }, async () => {
  request.mockImplementation(async (path: string) => {
    if (path.startsWith("/api/knowledge/collections/k9")) throw new ApiError("The server is busy", 503);
    return {};
  });
  renderWithQueries(<CollectionPage id="k9" />);
  // The query retries twice before giving up, so the box takes a few seconds.
  expect(await screen.findByRole("alert", {}, { timeout: 10_000 })).toHaveTextContent("The server is busy");
  expect(screen.getByRole("button", { name: /Retry/ })).toBeInTheDocument();
});
