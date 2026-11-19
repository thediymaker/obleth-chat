// The composer's bottom row: the tool buttons never wrap, so on a phone the model
// button shrinks instead of pushing Workspace and Browser onto a second line.

import { screen } from "@testing-library/react";
import { vi } from "vitest";
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat", display_name: "Pat Lee" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
import { ChatStore } from "@/lib/chat/store";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ChatStoreProvider } from "./chat-provider";
import { Composer } from "./composer";
import { polyfillLayout, renderWithQueries } from "./test-queries";

polyfillLayout();

test("the tool buttons stay on one line", () => {
  const store = new ChatStore({ api: { request: vi.fn().mockRejectedValue(new Error("offline")), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.route = { page: "home" } as never;
  renderWithQueries(
    <TooltipProvider>
      <ChatStoreProvider store={store}>
        <Composer home />
      </ChatStoreProvider>
    </TooltipProvider>,
  );
  const tools = screen.getByRole("group", { name: "What the model can use" });
  expect(tools.className).toContain("flex-nowrap");
  expect(tools.className).toContain("shrink-0");
});
