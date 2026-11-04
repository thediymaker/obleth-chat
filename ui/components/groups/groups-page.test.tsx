import { screen } from "@testing-library/react";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { GroupsPage, groupLine } from "./groups-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const detail = {
  group: { id: "g1", name: "Physics lab", description: "", members: 2 },
  projects: [{ id: "p1", name: "Thesis", color: "blue", mine: false, ownerName: "Bo", saved: 3 }],
  chats: [{ id: "c1", title: "Shared chat", mine: true }],
  collections: [{ id: "k1", name: "Papers", documents: 1 }],
  members: [
    { id: "u1", username: "ana", displayName: "Ana" },
    { id: "u2", username: "bo", displayName: "Bo" },
  ],
  canManage: false,
};

test("a group shows what is shared with it and who is in it", async () => {
  mockApi((path) => (path === "/api/groups" ? { groups: [{ id: "g1", name: "Physics lab", members: 2, projects: 1, member: true }], canManage: false } : detail));
  renderPage(<GroupsPage />, { user: { role: "user" } });
  expect(await screen.findByText("project · Bo · 3 saved items")).toBeInTheDocument();
  expect(screen.getByText("chat · yours")).toBeInTheDocument();
  expect(screen.getByText("collection · 1 doc")).toBeInTheDocument();
  expect(screen.getByText("2 people in this group.")).toBeInTheDocument();
  expect(screen.getByText("You")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /Thesis/ })).toHaveAttribute("href", "/projects/p1");
  expect(screen.getByText("An administrator adds and removes people.")).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: /Manage groups/ })).toBeNull();
});

test("with no groups, people are told how they join one", async () => {
  mockApi(() => ({ groups: [], canManage: false }));
  renderPage(<GroupsPage />, { user: { role: "user" } });
  expect(await screen.findByText("You are not in any groups")).toBeInTheDocument();
  expect(groupLine({ id: "g", name: "x", members: 1, collections: 2, member: false })).toBe("Not a member · 1 person · 2 collections");
});
