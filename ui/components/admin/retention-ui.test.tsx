import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { SystemTab } from "./system-tab";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

function tab() {
  const calls = mockApi((path, options) => (path === "/api/admin/retention" ? (options.method === "PATCH" ? options.body : { conversationDays: null, unusedFileDays: null, auditDays: null }) : {}));
  renderPage(<SystemTab />);
  return calls;
}
const patches = (calls: { method: string }[]) => calls.filter((c) => c.method === "PATCH");

test("automatic deletion cannot be enabled before explicit confirmation", async () => {
  const calls = tab();
  await userEvent.type(await screen.findByLabelText("Inactive conversations · days"), "365");
  await userEvent.type(screen.getByLabelText("Audit activity · days"), "90");
  await userEvent.click(screen.getByRole("button", { name: "Save retention" }));
  expect(await screen.findByText(/permanently deletes/)).toBeInTheDocument();
  expect(patches(calls)).toHaveLength(0);
  await userEvent.click(screen.getByRole("button", { name: "Apply deletion policy" }));
  await waitFor(() => expect(patches(calls)).toHaveLength(1));
  expect(patches(calls)[0]).toMatchObject({ body: { conversationDays: 365, unusedFileDays: null, auditDays: 90 } });
});

test("blank retention fields save keep-forever values without deletion confirmation", async () => {
  const calls = tab();
  await userEvent.click(await screen.findByRole("button", { name: "Save retention" }));
  await waitFor(() => expect(patches(calls)).toHaveLength(1));
  expect(screen.queryByText(/permanently deletes/)).toBeNull();
  expect(patches(calls)[0]).toMatchObject({ body: { conversationDays: null, unusedFileDays: null, auditDays: null } });
});

test("cancelling the confirmation saves nothing", async () => {
  const calls = tab();
  await userEvent.type(await screen.findByLabelText("Unused files · days"), "30");
  await userEvent.click(screen.getByRole("button", { name: "Save retention" }));
  await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(screen.queryByText(/permanently deletes/)).toBeNull());
  expect(patches(calls)).toHaveLength(0);
});
