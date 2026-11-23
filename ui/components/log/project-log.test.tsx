import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";
import { fakeStore } from "@/components/workspace/test-store";
import { ApiError } from "@/lib/api";
import { chatDraft } from "@/lib/chat/chat-draft";

const request = vi.fn();
const push = vi.fn();
let search = new URLSearchParams();
vi.mock("@/lib/api", async (orig) => ({ ...(await orig<typeof import("@/lib/api")>()), api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false }), useUser: () => ({ id: "u1", role: "user" }), useSignedIn: () => true }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }), usePathname: () => "/projects/p9/log", useParams: () => ({ id: "p9" }), useSearchParams: () => search }));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
vi.mock("sonner", () => ({ toast: vi.fn() }));

import { logPrompts, mondayOf, refHref } from "./api";
import { ProjectLog } from "./project-log";

polyfillLayout();

const SHA = "a41c9e2" + "0".repeat(33);
const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;

const day = (over: Record<string, unknown> = {}) => ({
  day: "2026-11-17",
  timeZone: zone,
  headline: "The sweep ran, and 128 is where each doubling starts to buy less",
  state: "written",
  failed: false,
  newer: false,
  writtenAt: "2026-11-17T19:00:00Z",
  model: "writer-1",
  lines: [
    { id: "l1", kind: "step", time: "09:12", at: null, text: "You asked whether a larger setting changes the conclusion.", refs: [{ kind: "chat", id: "c1", label: "Decode sweep" }, { kind: "commit", id: SHA, label: "a41c9e2" }], edited: false },
    { id: "l2", kind: "decision", time: "17:40", at: null, text: "Fig. 2 keeps its log axis.", refs: [{ kind: "run", id: "r1", label: "Run 3" }], edited: true },
  ],
  open: [{ id: "o1", kind: "open", time: null, at: null, text: "Run 4 at 256 is queued and has not started.", refs: [{ kind: "run", id: "r2", label: "Run 4" }], edited: false }],
  notes: [{ id: "n1", text: "Maya thinks the knee is memory-bound.", time: "18:05", at: "2026-11-17T17:05:00Z", sameDay: true }],
  ...over,
});

function serve(entry = day(), list: Record<string, unknown> = {}) {
  request.mockImplementation(async (path: string, options?: { method?: string; body?: unknown }) => {
    if (path.startsWith("/api/projects/p9/log?")) return { items: [{ day: "2026-11-17", headline: entry.headline, state: entry.state, failed: false, decisions: 1, notes: 1 }, { day: "2026-11-16", headline: "Outline and template", state: "written", failed: false, decisions: 0, notes: 0 }], more: false, enabled: true, timeZone: zone, ...list };
    if (path.startsWith("/api/projects/p9/log/week")) return { week: { from: "2026-11-16", to: "2026-11-22", timeZone: zone, daysWithWork: 2, chats: 3, runs: 7, decisions: 1, notes: 1 } };
    if (path === "/api/projects/p9/log/days/2026-11-17" && !options?.method) return { day: entry };
    if (options?.method) return { day: entry };
    return {};
  });
}

beforeEach(() => {
  request.mockReset();
  push.mockReset();
  search = new URLSearchParams();
  chatDraft.reset();
});

test("a log that is off on this server says so in the app's form", async () => {
  request.mockRejectedValue(new ApiError("The project log is turned off on this server", 503, "feature_off"));
  renderWithQueries(<ProjectLog projectId="p9" />);
  expect(await screen.findByRole("heading", { name: "The project log is turned off on this server" })).toBeInTheDocument();
});

test("a project with no days yet says how days get written", async () => {
  request.mockResolvedValue({ items: [], more: false, enabled: true, timeZone: zone });
  renderWithQueries(<ProjectLog projectId="p9" />);
  expect(await screen.findByRole("heading", { name: "No days in the log yet" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /Export the log/ })).toBeDisabled();
});

test("the newest day shows as a timeline whose references open where each line came from", async () => {
  serve();
  renderWithQueries(<ProjectLog projectId="p9" />);
  expect(await screen.findByRole("heading", { name: /The sweep ran, and 128/ })).toBeInTheDocument();
  const days = screen.getByRole("navigation", { name: "Days with work" });
  expect(within(days).getByRole("link", { name: /Tue 17/ })).toHaveAttribute("aria-current", "page");
  expect(within(days).getByRole("link", { name: /Outline and template/ })).toHaveAttribute("href", "/projects/p9/log?day=2026-11-16");
  expect(screen.getByText("Decision")).toBeInTheDocument();
  expect(screen.getByText("Edited by you")).toBeInTheDocument();
  expect(screen.getByText("Your note")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "chat: Decode sweep" })).toHaveAttribute("href", "/chat/c1");
  expect(screen.getByRole("link", { name: "a41c9e2" })).toHaveAttribute("href", `/projects/p9/code/history/${SHA}`);
  expect(screen.getByRole("link", { name: "run 3" })).toHaveAttribute("href", "/projects/p9/research?run=r1");
  expect(screen.getByText("Still open at the end of the day")).toBeInTheDocument();
  expect(screen.getByText("Run 4 at 256 is queued and has not started.")).toBeInTheDocument();
  expect(await screen.findByText("days with work")).toBeInTheDocument();
  expect(screen.getByText("7")).toBeInTheDocument();
  // Nothing is working, so nothing carries the accent.
  expect(document.querySelector(".bg-signal")).toBeNull();
  expect(screen.getByRole("button", { name: /Write this day again/ })).toBeEnabled();
});

test("a day being written shows as alive, and writing it again is asked for", async () => {
  serve(day({ state: "writing", headline: "" }));
  renderWithQueries(<ProjectLog projectId="p9" />);
  expect(await screen.findByRole("heading", { name: "Being written" })).toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent(/^Writing this day/);
  expect(document.querySelector(".signal-pulse.bg-signal")).not.toBeNull();
  expect(screen.queryByRole("button", { name: /Write this day again/ })).not.toBeInTheDocument();
});

test("writing a day again, adding a note and editing a line go to the API", async () => {
  serve();
  renderWithQueries(<ProjectLog projectId="p9" />);
  await screen.findByRole("heading", { name: /The sweep ran/ });
  await userEvent.click(screen.getByRole("button", { name: /Write this day again/ }));
  await waitFor(() => expect(request).toHaveBeenCalledWith("/api/projects/p9/log/days/2026-11-17/write", { method: "POST" }));

  await userEvent.type(screen.getByLabelText("Add your own note"), "Check the GPU memory logs.");
  await userEvent.click(screen.getByRole("button", { name: "Add" }));
  await waitFor(() => expect(request).toHaveBeenCalledWith("/api/projects/p9/log/days/2026-11-17/notes", { method: "POST", body: { text: "Check the GPU memory logs." } }));
  expect(screen.getByLabelText("Add your own note")).toHaveValue("");

  await userEvent.click(screen.getAllByRole("button", { name: "Edit this line" })[0]!);
  const box = screen.getByLabelText("Line");
  await userEvent.clear(box);
  await userEvent.type(box, "You asked about larger batches.");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(request).toHaveBeenCalledWith("/api/projects/p9/log/days/2026-11-17/lines/l1", { method: "PATCH", body: { text: "You asked about larger batches." } }));
});

test("writing from the log starts a chat in the project that reads the log", async () => {
  const store = fakeStore();
  serve();
  renderWithQueries(<ProjectLog projectId="p9" />);
  await userEvent.click(await screen.findByRole("button", { name: /Weekly update/ }));
  expect(push).toHaveBeenCalledWith("/");
  expect(chatDraft.get().project).toBe("p9");
  expect(store.starter).toHaveBeenCalledWith(expect.stringContaining("project_log"), false);
  expect(store.starter.mock.calls[0][0]).toContain("weekly update for my advisor");
});

test("the browser's time zone is saved when the log has another", async () => {
  serve(day(), { timeZone: null });
  renderWithQueries(<ProjectLog projectId="p9" />);
  await screen.findByRole("heading", { name: /The sweep ran/ });
  await waitFor(() => expect(request).toHaveBeenCalledWith("/api/log/settings", { method: "PUT", body: { timeZone: zone } }));
});

test("references and prompts are built from plain values", () => {
  expect(refHref("p 1", { kind: "chat", id: "c1", label: "x", gone: true })).toBeNull();
  expect(refHref("p 1", { kind: "card", id: "k1", label: "x" })).toBeNull();
  expect(mondayOf("2026-11-22")).toBe("2026-11-16");
  expect(mondayOf("2026-11-16")).toBe("2026-11-16");
  const p = logPrompts("2026-10-23", "2026-11-22", "2026-11-16");
  expect(p.weekly).toContain("from 2026-11-16 to 2026-11-22");
  expect(p.methods).toContain("from 2026-10-23 to 2026-11-22");
});
