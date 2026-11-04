import { hashPath } from "./hash-links";

test("old hash links land on the same page as a path", () => {
  expect(hashPath("#chat/1f2e")).toBe("/chat/1f2e");
  expect(hashPath("#chat")).toBe("/");
  expect(hashPath("#activate/abc.def")).toBe("/activate/abc.def");
  expect(hashPath("#projects/p1/app")).toBe("/projects/p1/app");
  expect(hashPath("#admin/users")).toBe("/settings/admin/users");
  expect(hashPath("#settings/admin/groups/g1")).toBe("/settings/admin/groups/g1");
  expect(hashPath("#settings/connections")).toBe("/settings/mcp");
  expect(hashPath("#settings/infrastructure")).toBe("/settings");
  expect(hashPath("#system")).toBe("/settings");
  expect(hashPath("#playground")).toBe("/");
  expect(hashPath("#classroom")).toBe("/groups");
  expect(hashPath("#compute")).toBe("/workspaces");
  expect(hashPath("#knowledge/c%201")).toBe("/knowledge/c%201");
  expect(hashPath("")).toBeNull();
  expect(hashPath("#")).toBeNull();
});
