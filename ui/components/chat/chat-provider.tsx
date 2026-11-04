"use client";

// Holds the chat store for the signed-in session, above the pages, so a reply keeps
// streaming while the person is on another page. Feeds it the route, the catalog,
// preferences, presets and capabilities from the shared queries, and keeps the
// Recent list in step with what it does.

import { useQueryClient } from "@tanstack/react-query";
import { usePathname, useRouter } from "next/navigation";
import { createContext, useContext, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { api } from "@/lib/api";
import { ChatStore, routeOf } from "@/lib/chat/store";
import type { Conversation, ModelInfo, Preset } from "@/lib/chat/types";
import { keys, useCapabilities, useConversations, useModels, usePreferences, usePresets, type ConversationSummary } from "@/lib/queries";
import { onBeforeSignOut, useSession } from "@/lib/session";

const Context = createContext<ChatStore | null>(null);

/** The window event the sidebar and the shortcut send for a fresh new chat. */
export const NEW_CHAT_EVENT = "obleth:new-chat";
/** The window event that opens conversation search in the shell. */
export const OPEN_SEARCH_EVENT = "obleth:search";

const summary = (c: Conversation): ConversationSummary => {
  const { messages: _messages, ...rest } = c;
  return { ...rest, updatedAt: (c.updatedAt as string) || new Date().toISOString() } as ConversationSummary;
};

export function ChatProvider({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const queries = useQueryClient();
  const { isAdmin } = useSession();
  const routerRef = useRef(router);
  routerRef.current = router;
  const [store] = useState(
    () =>
      new ChatStore({
        api,
        toast: (message) => toast(message),
        go: (path, replace) => (replace ? routerRef.current.replace(path) : routerRef.current.push(path)),
        chats: {
          upsert: (c) =>
            queries.setQueryData<ConversationSummary[]>(keys.conversations, (list = []) =>
              [...(c.archived ? [] : [summary(c)]), ...list.filter((other) => other.id !== c.id)].sort((a, b) => Number(!!b.pinned) - Number(!!a.pinned)),
            ),
          remove: (id) => {
            queries.setQueryData<ConversationSummary[]>(keys.conversations, (list = []) => list.filter((c) => c.id !== id));
            queries.invalidateQueries({ queryKey: keys.conversations });
          },
          refresh: () => queries.invalidateQueries({ queryKey: keys.conversations }),
          find: (id) => queries.getQueryData<ConversationSummary[]>(keys.conversations)?.find((c) => c.id === id) as never,
        },
        queries,
        preferencesChanged: (r) => queries.setQueryData(keys.preferences, r),
      }),
  );
  store.isAdmin = isAdmin;

  useEffect(() => {
    store.start();
    // Closing the tab leaves a temporary chat too; so does signing out.
    const leave = () => {
      store.leaveOpenTemporary();
    };
    window.addEventListener("pagehide", leave);
    const unhook = onBeforeSignOut(() => store.leaveOpenTemporary());
    const fresh = () => store.newChat();
    window.addEventListener(NEW_CHAT_EVENT, fresh);
    return () => {
      window.removeEventListener("pagehide", leave);
      window.removeEventListener(NEW_CHAT_EVENT, fresh);
      unhook();
      store.dispose();
    };
  }, [store]);

  const path = usePathname();
  useEffect(() => {
    store.setRoute(routeOf(path));
  }, [store, path]);

  const capabilities = useCapabilities().data;
  useEffect(() => {
    if (capabilities) {
      store.capabilities = capabilities;
      store.emit();
    }
  }, [store, capabilities]);

  const models = useModels();
  useEffect(() => {
    if (models.data) store.setCatalog({ models: models.data as ModelInfo[] });
    else if (models.error) store.setCatalog({ error: (models.error as Error).message });
  }, [store, models.data, models.error]);

  const preferences = usePreferences().data as { preferences?: never; overrides?: Record<string, unknown> } | undefined;
  const preferencesSeen = useRef(false);
  useEffect(() => {
    if (!preferences) return;
    store.setPreferencesResponse(preferences, !preferencesSeen.current && !store.active);
    preferencesSeen.current = true;
  }, [store, preferences]);

  const presets = usePresets().data as { presets?: Preset[] } | undefined;
  useEffect(() => {
    if (presets) store.setPresets(presets.presets || []);
  }, [store, presets]);

  // Loaded once so the store can name chats in menus opened from the sidebar.
  useConversations();

  return <Context.Provider value={store}>{children}</Context.Provider>;
}

/** A given store, for tests of the components inside a chat. */
export function ChatStoreProvider({ store, children }: { store: ChatStore; children: React.ReactNode }) {
  return <Context.Provider value={store}>{children}</Context.Provider>;
}

/** The chat store, re-rendering the component whenever it changes. */
export function useChat(): ChatStore {
  const store = useContext(Context);
  if (!store) throw new Error("useChat needs a ChatProvider");
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  return store;
}

/** The store without subscribing, for event handlers. */
export function useChatStore(): ChatStore {
  const store = useContext(Context);
  if (!store) throw new Error("useChatStore needs a ChatProvider");
  return store;
}

/** Whether a ChatProvider is above, for shared components also used outside one. */
export function useOptionalChat(): ChatStore | null {
  const store = useContext(Context);
  useSyncExternalStore(store?.subscribe ?? noop, store?.getVersion ?? zero, store?.getVersion ?? zero);
  return store;
}
const noop = () => () => {};
const zero = () => 0;
