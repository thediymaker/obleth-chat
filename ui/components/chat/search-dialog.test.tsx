import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";

const request = vi.fn();
vi.mock("@/lib/api", () => ({ api: { request: (...args: unknown[]) => request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready" }) }));
const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }) }));

import { SearchDialog } from "@/components/chat/search-dialog";

polyfillLayout();

describe("Search chats", () => {
  beforeEach(() => {
    push.mockReset();
    request.mockReset();
    request.mockImplementation(async (path: string) => {
      if (path === "/api/models") return { models: [{ id: "m1", name: "Model One" }] };
      if (path === "/api/conversations") return { conversations: [{ id: "r1", title: "Recent chat", model: "m1", updatedAt: new Date().toISOString() }] };
      if (path.startsWith("/api/conversations?"))
        return { conversations: [{ id: "f1", title: "GPU job notes", model: "m1", archived: true, updatedAt: new Date().toISOString() }], total: 1, hasMore: false };
      return {};
    });
  });

  it("lists recent chats before anything is typed", async () => {
    renderWithQueries(<SearchDialog open onOpenChange={() => {}} />);
    expect(await screen.findByText("Recent chat")).toBeInTheDocument();
  });

  it("asks the server, which searches titles and messages, and opens a result", async () => {
    const onOpenChange = vi.fn();
    renderWithQueries(<SearchDialog open onOpenChange={onOpenChange} />);
    await userEvent.type(screen.getByPlaceholderText("Search titles and messages…"), "gpu");
    expect(await screen.findByText("GPU job notes")).toBeInTheDocument();
    expect(request).toHaveBeenCalledWith("/api/conversations?query=gpu&archived=all&limit=20&offset=0", expect.anything());
    expect(request.mock.calls.filter(([path]) => String(path).includes("query=")).length).toBe(1);
    expect(screen.getByText("Archived")).toBeInTheDocument();
    await userEvent.click(screen.getByText("GPU job notes"));
    expect(push).toHaveBeenCalledWith("/chat/f1");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("offers every result on the All chats page", async () => {
    renderWithQueries(<SearchDialog open onOpenChange={() => {}} />);
    await userEvent.type(screen.getByPlaceholderText("Search titles and messages…"), "gpu");
    await userEvent.click(await screen.findByText(/See all results/));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/history?query=gpu&archived=all"));
  });

  it("offers the scratch pad, which opens once the dialog has closed", async () => {
    const opened = vi.fn();
    window.addEventListener("obleth:scratch", opened);
    function Harness() {
      const [open, setOpen] = useState(true);
      return <SearchDialog open={open} onOpenChange={setOpen} />;
    }
    renderWithQueries(<Harness />);
    expect(await screen.findByRole("option", { name: /Open the scratch pad/ })).toBeInTheDocument();
    await userEvent.type(screen.getByPlaceholderText("Search titles and messages…"), "gpu");
    expect(screen.queryByRole("option", { name: /Open the scratch pad/ })).not.toBeInTheDocument();
    await userEvent.clear(screen.getByPlaceholderText("Search titles and messages…"));
    await userEvent.type(screen.getByPlaceholderText("Search titles and messages…"), "scr");
    await userEvent.click(await screen.findByRole("option", { name: /Open the scratch pad/ }));
    await waitFor(() => expect(opened).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    window.removeEventListener("obleth:scratch", opened);
  });
});
