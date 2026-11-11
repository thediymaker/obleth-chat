// Test helpers for the workspace, browser and code parts of a chat: a small chat
// store with the real bridges over a mocked API, standing in for the ChatProvider.
//
//   vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
//   const store = fakeStore();
//   renderWithQueries(<WorkspacePanel conversationId="c1" tab="files" />);

import { useSyncExternalStore } from "react";
import { vi } from "vitest";
import { createBrowserBridge } from "@/components/browser/bridge";
import { createCodeBridge } from "@/components/code/bridge";
import { createDevicesBridge } from "@/lib/devices/bridge";
import { createWorkspaceBridge } from "@/components/workspace/bridge";
import { api } from "@/lib/api";

export type FakeStore = ReturnType<typeof makeStore>;

function makeStore() {
  const listeners = new Set<() => void>();
  let version = 0;
  const panes = new Map<string, { open: boolean; tab: string }>();
  const store = {
    version: 0,
    emit() {
      version++;
      listeners.forEach((l) => l());
    },
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    getVersion: () => version,
    route: { page: "chat", id: "c1" },
    active: { id: "c1", messages: [] as any[] } as any,
    streaming: false,
    streamChatId: null as string | null,
    capabilities: { features: {} as Record<string, boolean> },
    workspaceDraft: false,
    pane: (id: string) => panes.get(id) || { open: false, tab: "details" },
    showTab: vi.fn((id: string, tab: string) => {
      panes.set(id, { open: true, tab });
      store.emit();
    }),
    closePane: vi.fn((id: string) => {
      panes.set(id, { ...(panes.get(id) || { tab: "details" }), open: false });
      store.emit();
    }),
    showing: (id: string, tab: string) => !!panes.get(id)?.open && panes.get(id)?.tab === tab,
    send: vi.fn(async (_text: string) => {}),
    run: async (action: () => unknown) => {
      try {
        await action();
      } catch {
        // The real store shows a toast.
      }
    },
    newChat: vi.fn(),
    starter: vi.fn(),
    toggleBrowser: vi.fn(),
    toggleWorkspace: vi.fn(),
    openDialog: vi.fn(),
    feature: (name: string) => !!store.capabilities.features[name],
    workspace: null as unknown as ReturnType<typeof createWorkspaceBridge>,
    browser: null as unknown as ReturnType<typeof createBrowserBridge>,
    code: null as unknown as ReturnType<typeof createCodeBridge>,
    devices: null as unknown as ReturnType<typeof createDevicesBridge>,
  };
  const context = { api, enabled: () => true, changed: () => store.emit(), showTab: (id: string, tab: string) => store.showTab(id, tab) };
  store.workspace = createWorkspaceBridge({ ...context, viewing: () => false, toast: () => {} });
  store.browser = createBrowserBridge(context);
  store.code = createCodeBridge({ api, toast: () => {} });
  store.devices = createDevicesBridge({ api, enabled: () => true, changed: () => store.emit(), serial: null });
  return store;
}

let current = makeStore();
/** A fresh store for the next render. */
export function fakeStore() {
  current = makeStore();
  return current;
}

export function useChat() {
  useSyncExternalStore(current.subscribe, current.getVersion, current.getVersion);
  return current as any;
}
export function useChatStore() {
  return current as any;
}
export function useOptionalChat() {
  return useChat();
}
export function ChatProvider({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}
export const NEW_CHAT_EVENT = "obleth:new-chat";
