import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { AboutCard, SystemTab, bannerBody, workspaceStatus } from "./system-tab";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

test("the system tab says where workspaces run, or why they are off", () => {
  expect(workspaceStatus({ workspaces: { enabled: true, namespace: "obleth-chat", browser: true, off: null } })).toBe("On, in namespace obleth-chat, with the browser");
  expect(workspaceStatus({ workspaces: { enabled: false, namespace: null, browser: false, off: "Turned off with OBLETH_CHAT_WORKSPACES=off" } })).toBe("Off: Turned off with OBLETH_CHAT_WORKSPACES=off");
});

test("the About card says which Chat this is, with its source and release notes", () => {
  const about = {
    version: "0.16.0",
    repository: "https://github.com/thediymaker/obleth-chat",
    migration: { version: 60, description: "workspace sessions" },
    buildMigration: 60,
    replica: "obleth-chat-7d9f",
    startedAt: "2026-11-02T12:00:00Z",
    images: { workspace: "ghcr.io/thediymaker/obleth-chat/workspace:v0.16.0@sha256:0123456789abcdef", browser: null },
  };
  const { container, unmount } = render(<AboutCard health={{ about }} />);
  expect(screen.getByText("v0.16.0").tagName).toBe("STRONG");
  expect(screen.getByRole("link", { name: /Release notes/ })).toHaveAttribute("href", "https://github.com/thediymaker/obleth-chat/releases/tag/v0.16.0");
  expect(screen.getByRole("link", { name: /github.com\/thediymaker\/obleth-chat/ })).toBeInTheDocument();
  expect(container.textContent).toContain("Migration 060, workspace sessions");
  expect(screen.getByText("workspace:v0.16.0").tagName).toBe("CODE");
  expect(screen.getByText("0123456789ab")).toBeInTheDocument();
  expect(container.textContent).not.toContain("Browser image");
  expect(container.textContent).not.toContain("newer than this replica");
  unmount();
  render(<AboutCard health={{ about: { version: "0.15.0", repository: "https://x", migration: { version: 60, description: "w" }, buildMigration: 57 } }} />);
  expect(screen.getByText(/newer than this replica/)).toBeInTheDocument();
});

test("the banner card shows the saved banner and saves what the form says", async () => {
  const calls = mockApi((path, options) => {
    if (path === "/api/admin/banner" && options.method === "PATCH") return { ...(options.body as object), updatedAt: "2026-10-29T12:00:00Z" };
    if (path === "/api/admin/banner") return { text: "In development", link: null, tone: "warning", signedOut: true, dismissible: false, updatedAt: "2026-10-29T00:00:00Z" };
    if (path === "/api/admin/retention") return { conversationDays: null, unusedFileDays: null, auditDays: null };
    return {};
  });
  const { session } = renderPage(<SystemTab />);
  const text = await screen.findByLabelText("Text");
  expect(text).toHaveValue("In development");
  expect(screen.getByLabelText("Style")).toHaveValue("warning");
  expect(screen.getByRole("checkbox", { name: "Show it on the sign-in page" })).toBeChecked();
  await userEvent.clear(text);
  await userEvent.type(text, "Maintenance Friday");
  await userEvent.type(screen.getByLabelText("Link text"), "Details");
  await userEvent.type(screen.getByLabelText("Link address"), "https://status.example.edu");
  await userEvent.selectOptions(screen.getByLabelText("Style"), "info");
  await userEvent.click(screen.getByRole("checkbox", { name: "Show it on the sign-in page" }));
  await userEvent.click(screen.getByRole("checkbox", { name: "Let people close it" }));
  await userEvent.click(screen.getByRole("button", { name: "Save banner" }));
  await waitFor(() =>
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ text: "Maintenance Friday", link: { label: "Details", url: "https://status.example.edu" }, tone: "info", signedOut: false, dismissible: true }),
  );
  await waitFor(() => expect(session.setBanner).toHaveBeenCalledWith(expect.objectContaining({ text: "Maintenance Friday" })));
});

test("a banner link needs both its text and its address", () => {
  const form = new FormData();
  form.set("banner-text", "Hi");
  form.set("banner-link-label", "Details");
  expect(() => bannerBody(form)).toThrow("A link needs both its text and its address.");
});

test("knowledge search is asked for only where knowledge is on, and a failed part shows on its own card", async () => {
  const calls = mockApi((path) => {
    if (path === "/api/admin/retention") return { conversationDays: null, unusedFileDays: null, auditDays: null };
    if (path === "/api/admin/decisions") return new Error("Decisions are unavailable");
    return {};
  });
  renderPage(<SystemTab />);
  expect(await screen.findByText("Decisions are unavailable")).toBeInTheDocument();
  expect(calls.some((c) => c.path === "/api/admin/knowledge/embedding")).toBe(false);
  expect(screen.getByRole("button", { name: "Save retention" })).toBeInTheDocument();
});
