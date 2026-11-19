// The Batches page: older batches load on request, and only running ones are polled.

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";

const request = vi.fn();
vi.mock("@/lib/api", () => ({ api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready" }), useSignedIn: () => true }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), usePathname: () => "/batches", useSearchParams: () => new URLSearchParams() }));

import { BatchesPage } from "./batches-page";

polyfillLayout();

const batch = (id: string, status = "completed") => ({ id, name: `Batch ${id}`, model: "m1", status, total: 10, completed: 10, failed: 0, createdAt: "2026-11-01T10:00:00Z", inProgressAt: null, finishedAt: null, deadlineAt: null, lastProgressAt: null, waiting: false, rows: null, fromChat: false, tags: [] });

test("older batches load on request, without repeating any", async () => {
  request.mockImplementation(async (path: string) => {
    if (path === "/api/models") return { models: [{ id: "m1", name: "Model One" }] };
    if (path === "/api/batches") return { available: true, batches: [batch("b1"), batch("b2")], more: true, nextAfter: "b2" };
    if (path === "/api/batches?after=b2") return { available: true, batches: [batch("b2"), batch("b3")], more: false, nextAfter: null };
    return {};
  });
  renderWithQueries(<BatchesPage />);
  expect(await screen.findByText("Batch b1")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Load more batches" }));
  expect(await screen.findByText("Batch b3")).toBeInTheDocument();
  expect(screen.getAllByText("Batch b2")).toHaveLength(1);
  expect(screen.queryByRole("button", { name: "Load more batches" })).not.toBeInTheDocument();
  expect(request.mock.calls.some(([path]) => path === "/api/batches?status=active")).toBe(false);
});

test("while a batch runs, its progress alone is polled", async () => {
  request.mockImplementation(async (path: string) => {
    if (path === "/api/models") return { models: [{ id: "m1", name: "Model One" }] };
    if (path === "/api/batches") return { available: true, batches: [{ ...batch("b1", "in_progress"), completed: 2 }], more: false, nextAfter: null };
    if (path === "/api/batches?status=active") return { available: true, batches: [{ ...batch("b1", "in_progress"), completed: 7 }], more: false };
    return {};
  });
  renderWithQueries(<BatchesPage />);
  expect(await screen.findByText("Batch b1")).toBeInTheDocument();
  await waitFor(() => expect(request.mock.calls.some(([path]) => path === "/api/batches?status=active")).toBe(true));
  await waitFor(() => expect(screen.getAllByText(/\b7\b/).length).toBeGreaterThan(0));
});
