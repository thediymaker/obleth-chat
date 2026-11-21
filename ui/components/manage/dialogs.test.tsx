import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { GroupsTab } from "@/components/admin/groups-tab";
import { ChatDialogs } from "@/components/chat/chat-dialogs";
import { SaveDialog } from "@/components/library/saved-dialogs";
import { mockApi, renderPage } from "@/components/manage/test-utils";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ usePathname: () => "/", useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));
const store = {
  dialog: { kind: "rename", id: "c1", title: "Old title" } as unknown,
  rename: vi.fn(async () => {
    throw new Error("That title is too long");
  }),
  openDialog: vi.fn(),
  attachments: { setLimits: vi.fn(), current: () => [] },
};
vi.mock("@/components/chat/chat-provider", () => ({ useChat: () => store }));

test("a chat dialog shows the server's message as a form error and stays open", async () => {
  mockApi(() => ({}));
  renderPage(<ChatDialogs />);
  await userEvent.click(screen.getByRole("button", { name: "Save title" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("That title is too long");
  expect(store.rename).toHaveBeenCalledWith("c1", "Old title");
  expect(screen.getByRole("dialog", { name: "Rename chat" })).toBeInTheDocument();
});

test("Cancel closes a Library dialog and an Administration dialog alike", async () => {
  mockApi(() => ({ groups: [], projects: [], tags: [] }));
  const onOpenChange = vi.fn();
  const { unmount } = renderPage(<SaveDialog open onOpenChange={onOpenChange} draft={{ kind: "note", title: "Notes" }} title="Save to Library" />);
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onOpenChange).toHaveBeenCalledWith(false);
  unmount();

  renderPage(<GroupsTab groupId="" />);
  await userEvent.click((await screen.findAllByRole("button", { name: /Create a group|New group/ }))[0]!);
  expect(screen.getByRole("dialog", { name: "Create group" })).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});
