import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { polyfillLayout } from "@/components/chat/test-queries";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { AddToNotebook } from "./add-to-notebook";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("sonner", () => ({ toast: vi.fn() }));

polyfillLayout();

beforeEach(() => {
  vi.mocked(toast).mockClear();
});

describe("Add this page to a notebook", () => {
  it("adds the tab's page to the notebook of the chat's project", async () => {
    const calls = mockApi(
      (path) => {
        if (path === "/api/conversations/c1/project") return { project: { id: "p1", name: "Batching paper" } };
        if (path === "/api/conversations/c1/browser/notebook") return { projectId: "p1", documentId: "d1", name: "Report.md" };
        return {};
      },
      { knowledge: true },
    );
    renderPage(<AddToNotebook conversationId="c1" url="https://example.org/report" />);
    fireEvent.click(await screen.findByRole("button", { name: "Add this page to a notebook" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/conversations/c1/browser/notebook", method: "POST", body: {} }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith("Added to the notebook of Batching paper. It is indexed in the background.", expect.anything()));
  });

  it("asks which project when the chat is in none", async () => {
    const calls = mockApi(
      (path) => {
        if (path === "/api/conversations/c1/project") return { project: null };
        if (path === "/api/projects") return { projects: [{ id: "p2", name: "Thesis", color: "blue", mine: true }] };
        if (path === "/api/conversations/c1/browser/notebook") return new Error("Only the project's owner can add a collection to it");
        return {};
      },
      { knowledge: true },
    );
    renderPage(<AddToNotebook conversationId="c1" url="https://example.org/report" />);
    await userEvent.click(await screen.findByRole("button", { name: "Add this page to a notebook" }));
    expect(screen.getByText("This chat is in no project. Add the page to:")).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("menuitem", { name: "Thesis" }));
    await waitFor(() => expect(calls).toContainEqual({ path: "/api/conversations/c1/browser/notebook", method: "POST", body: { projectId: "p2" } }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith("Only the project's owner can add a collection to it"));
  });

  it("is not offered for a tab without a web page, or without knowledge", async () => {
    // Shown for a web page once the chat's project is known, so each case waits for that.
    const settled = async (calls: { path: string }[]) => {
      await waitFor(() => expect(calls.some((c) => c.path === "/api/conversations/c1/project")).toBe(true));
      await new Promise((r) => setTimeout(r, 50));
    };
    let calls = mockApi((path) => (path === "/api/conversations/c1/project" ? { project: { id: "p1", name: "P" } } : {}), { knowledge: true });
    const web = renderPage(<AddToNotebook conversationId="c1" url="https://example.org/" />);
    expect(await screen.findByRole("button", { name: "Add this page to a notebook" })).toBeInTheDocument();
    web.unmount();
    calls = mockApi((path) => (path === "/api/conversations/c1/project" ? { project: { id: "p1", name: "P" } } : {}), { knowledge: true });
    const blank = renderPage(<AddToNotebook conversationId="c1" url="about:blank" />);
    await settled(calls);
    expect(screen.queryByRole("button", { name: "Add this page to a notebook" })).not.toBeInTheDocument();
    blank.unmount();
    calls = mockApi((path) => (path === "/api/conversations/c1/project" ? { project: { id: "p1", name: "P" } } : {}), { knowledge: false });
    renderPage(<AddToNotebook conversationId="c1" url="https://example.org/" />);
    await settled(calls);
    expect(screen.queryByRole("button", { name: "Add this page to a notebook" })).not.toBeInTheDocument();
  });
});
