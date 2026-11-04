"use client";

// What a new chat starts with before it exists: the project it was started from
// ("New chat in project") and the collections picked for it ("Ask in a new chat", the
// knowledge picker on the home composer). The chat page reads it with useChatDraft(),
// and once the chat is created calls applyChatDraft(id) to store both. Old
// projects.js chat.draft and knowledge.js chat.draft.

import { useSyncExternalStore } from "react";
import { api } from "@/lib/api";

export interface ChatDraft {
  /** The project the new chat goes into, or null for none. */
  project: string | null;
  /** The collections the new chat searches. */
  collections: string[];
}

let draft: ChatDraft = { project: null, collections: [] };
const listeners = new Set<() => void>();
const set = (next: Partial<ChatDraft>) => {
  draft = { ...draft, ...next };
  listeners.forEach((l) => l());
};

export const chatDraft = {
  get: () => draft,
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
  setProject: (project: string | null) => set({ project }),
  setCollections: (collections: string[]) => set({ collections }),
  /** A fresh new chat: nothing picked. */
  reset: () => set({ project: null, collections: [] }),
};

const SERVER: ChatDraft = { project: null, collections: [] };
/** The draft, re-rendering when it changes. */
export function useChatDraft(): ChatDraft {
  return useSyncExternalStore(chatDraft.subscribe, chatDraft.get, () => SERVER);
}

/**
 * A new chat was created: put it in the drafted project and give it the drafted
 * collections, then clear the draft. Failures come back as messages to show.
 */
export async function applyChatDraft(conversationId: string): Promise<string[]> {
  const { project, collections } = draft;
  set({ project: null, collections: [] });
  const path = `/api/conversations/${encodeURIComponent(conversationId)}`;
  const errors: string[] = [];
  await Promise.all([
    project && api.request(`${path}/project`, { method: "PUT", body: { projectId: project } }).catch((e: Error) => errors.push(e.message)),
    collections.length && api.request(`${path}/knowledge`, { method: "PUT", body: { collectionIds: collections } }).catch((e: Error) => errors.push(e.message)),
  ]);
  return errors;
}
