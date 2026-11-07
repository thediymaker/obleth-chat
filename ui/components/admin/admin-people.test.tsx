import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { choose, mockApi, renderPage } from "@/components/manage/test-utils";
import { PeopleTab } from "./people-tab";
import { forgetKept } from "./state";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const users = [
  { id: "u1", username: "ana", display_name: "Ana", role: "admin", enabled: true, last_sign_in_at: "2026-11-02T18:30:00Z" },
  { id: "u2", username: "bo", display_name: "Bo", role: "user", enabled: true, last_sign_in_at: null },
];

test("the people tab shows when each person last signed in", async () => {
  forgetKept();
  mockApi((path) => (path.startsWith("/api/admin/users") ? { users, hasMore: false } : {}));
  const { container } = renderPage(<PeopleTab />);
  expect(await screen.findByRole("columnheader", { name: "Last sign-in" })).toBeInTheDocument();
  expect(container.querySelector('time[datetime="2026-11-02T18:30:00Z"]')).not.toBeNull();
  expect(screen.getByText("Never")).toBeInTheDocument();
  expect(screen.getByText("You")).toBeInTheDocument();
});

test("searching asks the server, and changing a role is confirmed first", async () => {
  forgetKept();
  const calls = mockApi((path, options) => (options.method === "PATCH" ? {} : { users, hasMore: path.includes("q=") }));
  renderPage(<PeopleTab />);
  await userEvent.type(await screen.findByLabelText("Search accounts"), "bo{Enter}");
  expect(await screen.findByText(/Showing the first 1,000 matches/)).toBeInTheDocument();
  expect(calls.some((c) => c.path === "/api/admin/users?q=bo")).toBe(true);
  const row = screen.getByText("bo").closest("tr")!;
  await userEvent.click(within(row).getByRole("button", { name: "Manage" }));
  await choose(screen.getByLabelText("Role"), "Administrator");
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(await screen.findByText("Change this account’s role?")).toBeInTheDocument();
  expect(calls.some((c) => c.method === "PATCH")).toBe(false);
  await userEvent.click(screen.getByRole("button", { name: "Change role" }));
  await waitFor(() => expect(calls.find((c) => c.method === "PATCH")).toEqual({ path: "/api/admin/users/u2", method: "PATCH", body: { displayName: "Bo", role: "admin" } }));
});

test("an invitation shows its one-time link", async () => {
  forgetKept();
  const calls = mockApi((path) => (path === "/api/admin/invitations" ? { kind: "invite", url: "https://chat.example/#activate/abc", expiresAt: "2026-11-10T00:00:00Z" } : { users }));
  renderPage(<PeopleTab />);
  await userEvent.click(await screen.findByRole("button", { name: "Invite person" }));
  await userEvent.type(screen.getByLabelText("Username"), "cy");
  await userEvent.type(screen.getByLabelText("Display name"), "Cy");
  await userEvent.click(screen.getByRole("button", { name: "Create invitation" }));
  expect(await screen.findByText("Invitation ready")).toBeInTheDocument();
  expect(screen.getByLabelText("One-time link")).toHaveValue("https://chat.example/#activate/abc");
  expect(calls.find((c) => c.path === "/api/admin/invitations")?.body).toEqual({ username: "cy", displayName: "Cy", role: "user" });
});

test("without passwords there is no invitation, password reset or initial password", async () => {
  forgetKept();
  mockApi(() => ({ users }));
  renderPage(<PeopleTab />, { passwords: false, oidc: true });
  await screen.findByText("Bo");
  expect(screen.queryByRole("button", { name: "Invite person" })).toBeNull();
  await userEvent.click(screen.getAllByRole("button", { name: "Manage" })[1]!);
  expect(screen.queryByRole("button", { name: "Reset password" })).toBeNull();
  expect(screen.getByRole("button", { name: "Link single sign-on" })).toBeInTheDocument();
});
