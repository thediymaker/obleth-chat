import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { AdminPage, adminLocation } from "@/components/admin/admin-page";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { ApiError } from "@/lib/api";
import { hashPath } from "@/lib/hash-links";
import { TokensCard } from "./account-cards";
import { settingsTabs } from "./settings-frame";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ usePathname: () => "/settings", useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

test("old administration links land on the same tab, and groups keep their id", () => {
  expect(hashPath("#admin/models")).toBe("/settings/admin/models");
  expect(hashPath("#settings/admin/groups/group-123")).toBe("/settings/admin/groups/group-123");
  expect(adminLocation(["groups", "group-123"])).toEqual({ section: "groups", groupId: "group-123" });
  expect(adminLocation([])).toEqual({ section: "users", groupId: "" });
  expect(adminLocation(["nope"])).toEqual({ section: "users", groupId: "" });
  expect(adminLocation(["models", "x"])).toEqual({ section: "models", groupId: "" });
});

test("ordinary Settings navigation omits Administration, and MCP servers needs connections", () => {
  expect(settingsTabs(false, false).map((t) => t.label)).toEqual(["Personal"]);
  expect(settingsTabs(true, false).map((t) => t.href)).toEqual(["/settings", "/settings/admin"]);
  expect(settingsTabs(false, true).map((t) => t.label)).toEqual(["Personal", "MCP servers", "Secrets"]);
});

test("administration deep links stay denied without asking for administrator data", async () => {
  const calls = mockApi(() => ({}));
  renderPage(<AdminPage segments={["models"]} />, { user: { role: "user" } });
  expect(screen.getByText("Administrator access required")).toBeInTheDocument();
  await new Promise((r) => setTimeout(r, 20));
  expect(calls.filter((c) => c.path.startsWith("/api/admin"))).toEqual([]);
});

test("a token refused for an old sign-in offers to sign in again", async () => {
  let answer: () => unknown = () => new ApiError("Sign in again to create a token", 403);
  const calls = mockApi((_path, options) => (options.method === "POST" ? answer() : {}));
  const { session } = renderPage(<TokensCard tokens={[]} onChange={() => {}} />);
  const create = async () => {
    await userEvent.click(screen.getByRole("button", { name: "New token" }));
    await userEvent.type(screen.getByLabelText("Name"), "Laptop");
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
  };
  await create();
  expect(await screen.findByText("Sign in again", { selector: "h2" })).toBeInTheDocument();
  expect(screen.getByText(/Sign in again to create a token\. For your security/)).toBeInTheDocument();
  expect(calls.at(-1)).toEqual({ path: "/api/auth/tokens", method: "POST", body: { name: "Laptop", days: 7, readOnly: false } });
  await userEvent.click(screen.getByRole("button", { name: "Sign out to sign in again" }));
  expect(session.signOut).toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Not now" }));

  // Other refusals are not about the sign-in's age and are shown as they are.
  answer = () => new ApiError("Change your password before continuing", 403);
  await create();
  expect(await screen.findByText("Change your password before continuing")).toBeInTheDocument();
  expect(screen.queryByText("Sign in again", { selector: "h2" })).toBeNull();
  await userEvent.keyboard("{Escape}");

  answer = () => ({ token: "obc_secret" });
  await create();
  expect(await screen.findByText("Copy your token now")).toBeInTheDocument();
  expect(screen.getByLabelText("Token")).toHaveValue("obc_secret");
  await waitFor(() => expect((screen.getByLabelText("Add it to Claude Code") as HTMLTextAreaElement).value).toMatch(/claude mcp add --transport http obleth-chat .*\/mcp --header "Authorization: Bearer obc_secret"/));
});
