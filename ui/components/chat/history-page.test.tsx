import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";

const request = vi.fn();
vi.mock("@/lib/api", () => ({ api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready" }) }));
const replace = vi.fn();
let search = "";
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace }),
  usePathname: () => "/history",
  useSearchParams: () => new URLSearchParams(search),
}));

import { HistoryPage, ImportDialog } from "@/components/chat/history-page";

polyfillLayout();

const chats = (from: number, n: number) =>
  Array.from({ length: n }, (_, i) => ({ id: `c${from + i}`, title: `Chat ${from + i}`, model: "m1", updatedAt: "2026-11-01T10:00:00Z", pinned: i === 0, archived: false }));

function serve(total = 120) {
  request.mockImplementation(async (path: string) => {
    if (path === "/api/models") return { models: [{ id: "m1", name: "Model One" }] };
    if (path.startsWith("/api/conversations?")) {
      const offset = Number(new URLSearchParams(path.split("?")[1]).get("offset"));
      const n = Math.max(0, Math.min(50, total - offset));
      return { conversations: chats(offset, n), total, hasMore: offset + n < total };
    }
    return {};
  });
}

describe("All chats", () => {
  beforeEach(() => {
    request.mockReset();
    replace.mockReset();
    search = "";
  });

  it("lists a page of chats with their model, and pages on", async () => {
    serve();
    renderWithQueries(<HistoryPage />);
    expect(await screen.findByText("Chat 0")).toBeInTheDocument();
    expect(screen.getAllByText("Model One")).toHaveLength(50);
    expect(screen.getByText("1–50 of 120")).toBeInTheDocument();
    expect(screen.getAllByRole("link")[3]).toHaveAttribute("href", "/chat/c3");
    expect(screen.getByRole("button", { name: "Previous" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Chat 50")).toBeInTheDocument();
    expect(request).toHaveBeenCalledWith("/api/conversations?query=&archived=false&limit=50&offset=50", expect.anything());
    expect(screen.getByText("51–100 of 120")).toBeInTheDocument();
  });

  it("selecting the page turns on the bulk actions, and delete asks first", async () => {
    serve(3);
    renderWithQueries(<HistoryPage />);
    await screen.findByText("Chat 0");
    const exportButton = screen.getByRole("button", { name: "Export" });
    expect(exportButton).toBeDisabled();
    await userEvent.click(screen.getByRole("checkbox", { name: "Select page" }));
    expect(screen.getByText("3 selected")).toBeInTheDocument();
    expect(exportButton).toBeEnabled();
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("Delete 3 conversations?")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Delete conversations" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/conversations/bulk", { method: "POST", body: { ids: ["c0", "c1", "c2"], action: "delete" } }));
  });

  it("pins from the row and searches what was asked in the URL", async () => {
    search = "query=gpu&archived=all";
    serve(1);
    renderWithQueries(<HistoryPage />);
    await screen.findByText("Chat 0");
    expect(request).toHaveBeenCalledWith("/api/conversations?query=gpu&archived=all&limit=50&offset=0", expect.anything());
    expect(screen.getByRole("textbox", { name: "Search conversations" })).toHaveValue("gpu");
    await userEvent.click(screen.getByRole("button", { name: "Unpin" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/conversations/c0", { method: "PATCH", body: { pinned: false } }));
  });

  it("says when a search finds nothing", async () => {
    search = "query=nothing";
    request.mockImplementation(async (path: string) => (path === "/api/models" ? { models: [] } : { conversations: [], total: 0, hasMore: false }));
    renderWithQueries(<HistoryPage />);
    expect(await screen.findByText("No conversations found")).toBeInTheDocument();
    expect(screen.getByText("Try another search or include archived conversations.")).toBeInTheDocument();
  });
});

describe("Import conversations", () => {
  beforeEach(() => {
    request.mockReset();
  });
  const choose = (file: File) => userEvent.upload(screen.getByLabelText("Export file"), file);

  it("sends the file unread and says what came over", async () => {
    request.mockResolvedValue({ source: "chatgpt", imported: 3 });
    const onImported = vi.fn();
    renderWithQueries(<ImportDialog open onOpenChange={() => {}} onImported={onImported} />);
    expect(screen.getByText(/Open WebUI, ChatGPT or Claude/)).toBeInTheDocument();
    expect(screen.getByLabelText("Export file")).toHaveAttribute("accept", ".json,.zip,application/json,application/zip");
    const file = new File(["{}"], "conversations.json", { type: "application/json" });
    await choose(file);
    await userEvent.click(screen.getByRole("button", { name: "Import conversations" }));
    expect(await screen.findByText("Import finished")).toBeInTheDocument();
    expect(screen.getByText("Imported 3 chats from ChatGPT.")).toBeInTheDocument();
    expect(request.mock.calls[0][1].raw).toBe(file);
    expect(onImported).toHaveBeenCalled();
  });

  it("refuses an export over 200 MB before sending it", async () => {
    renderWithQueries(<ImportDialog open onOpenChange={() => {}} onImported={() => {}} />);
    const file = new File(["x"], "export.zip", { type: "application/zip" });
    Object.defineProperty(file, "size", { value: 300 * 1024 * 1024 });
    await choose(file);
    await userEvent.click(screen.getByRole("button", { name: "Import conversations" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Unzip it and choose the conversations.json file inside.");
    expect(request).not.toHaveBeenCalled();
  });

  it("a reply that arrives after the page closed shows nothing", async () => {
    let release: (value: unknown) => void = () => {};
    request.mockImplementation(() => new Promise((resolve) => (release = resolve)));
    const onImported = vi.fn();
    const view = renderWithQueries(<ImportDialog open onOpenChange={() => {}} onImported={onImported} />);
    const file = new File(["{}"], "conversations.json", { type: "application/json" });
    await choose(file);
    await userEvent.click(screen.getByRole("button", { name: "Import conversations" }));
    expect(screen.getByText("Importing… A large export can take a few minutes.")).toBeInTheDocument();
    expect(request.mock.calls[0][1].raw).toBe(file);
    expect(request.mock.calls[0][1].signal.aborted).toBe(false);
    view.unmount();
    expect(request.mock.calls[0][1].signal.aborted).toBe(true);
    release({ source: "chatgpt", imported: 3 });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(onImported).not.toHaveBeenCalled();
    expect(screen.queryByText("Import finished")).not.toBeInTheDocument();
  });
});
