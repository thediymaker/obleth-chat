// The Library: older saved items show on request.

import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";

const request = vi.fn();
vi.mock("@/lib/api", () => ({ api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready" }), useSignedIn: () => true, useUser: () => ({ id: "u1", role: "user" }) }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), usePathname: () => "/library", useSearchParams: () => new URLSearchParams() }));

import { LibraryPage } from "./library-page";
import { SAVED_PAGE } from "./api";

polyfillLayout();

const item = (n: number) => ({ id: `s${n}`, kind: "note", title: `Note ${n}`, body: "", tags: [], pinned: false, searchable: false, createdAt: "2026-11-01T10:00:00Z", updatedAt: "2026-11-01T10:00:00Z" });

test("Show more asks for a larger page", async () => {
  request.mockImplementation(async (path: string) => {
    if (path.startsWith("/api/saved")) {
      const limit = Number(new URLSearchParams(path.split("?")[1] ?? "").get("limit") || SAVED_PAGE);
      const items = Array.from({ length: Math.min(limit, 70) }, (_, i) => item(i + 1));
      return { items, more: items.length < 70, tags: [], kinds: [] };
    }
    if (path.startsWith("/api/projects")) return { projects: [] };
    return {};
  });
  renderWithQueries(<LibraryPage />);
  expect(await screen.findByText("Note 1")).toBeInTheDocument();
  expect(screen.queryByText("Note 70")).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Show more" }));
  expect(await screen.findByText("Note 70")).toBeInTheDocument();
  expect(request.mock.calls.some(([path]) => path === `/api/saved?limit=${2 * SAVED_PAGE}`)).toBe(true);
  expect(screen.queryByRole("button", { name: "Show more" })).not.toBeInTheDocument();
});
