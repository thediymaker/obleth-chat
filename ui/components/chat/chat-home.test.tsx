// Home's tabs taken from the chat draft: the scratch pad's Research this opens Home
// on Research with its text in the box.

import { screen, waitFor } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat", display_name: "Pat Lee" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), useSearchParams: () => new URLSearchParams(), usePathname: () => "/" }));
import { chatDraft } from "@/lib/chat/chat-draft";
import { ChatStore } from "@/lib/chat/store";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ChatStoreProvider } from "./chat-provider";
import { ChatHome } from "./chat-view";
import { polyfillLayout, renderWithQueries } from "./test-queries";

polyfillLayout();

test("a drafted Research tab is chosen once, with the text in the box", async () => {
  apiRequest.mockImplementation(async (path: string) => {
    if (path === "/api/capabilities") return { application: "chat", mode: "connected", features: {} };
    if (path === "/api/conversations") return { conversations: [] };
    throw new Error("offline");
  });
  const store = new ChatStore({ api: { request: apiRequest, send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.route = { page: "home" } as never;
  store.newChat();
  chatDraft.setIntent("research");
  store.starter("Prefix caching", false);
  renderWithQueries(
    <TooltipProvider>
      <ChatStoreProvider store={store}>
        <ChatHome />
      </ChatStoreProvider>
    </TooltipProvider>,
  );
  await waitFor(() => expect(screen.getByRole("radio", { name: "Research" })).toHaveAttribute("aria-checked", "true"));
  expect(screen.getByRole("combobox", { name: "Message" })).toHaveValue("Prefix caching");
  expect(chatDraft.get().intent).toBeNull();
});
