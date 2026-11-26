"use client";

// What a new chat starts with before it exists: the project it was started from
// ("New chat in project"), the board card it works ("Start with the assistant") and
// the collections picked for it ("Ask in a new chat", the knowledge picker on the
// home composer) and the Home tab it opens on (the scratch pad's Research this). The chat page reads it with useChatDraft(), and once the chat is
// created calls applyChatDraft(id) to store them. Old projects.js chat.draft and
// knowledge.js chat.draft.

import { useSyncExternalStore } from "react";
import { api } from "@/lib/api";
import type { Intent } from "@/lib/chat/home";

export interface ChatDraft {
  /** The project the new chat goes into, or null for none. */
  project: string | null;
  /** The collections the new chat searches. */
  collections: string[];
  /** The card of the project's board the new chat works, or null. */
  card: { id: string; title: string } | null;
  /** The Home tab to choose once Home shows, or null to leave it as it is. */
  intent: Intent | null;
}

let draft: ChatDraft = { project: null, collections: [], card: null, intent: null };
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
  /** Another project (or none) drops the card, which belongs to the old one. */
  setProject: (project: string | null) => set(project === draft.project ? { project } : { project, card: null }),
  /** The card the new chat works; set after its project. */
  setCard: (card: { id: string; title: string } | null) => set({ card }),
  setCollections: (collections: string[]) => set({ collections }),
  /** Home chooses this tab when it next shows, then clears it. */
  setIntent: (intent: Intent | null) => set({ intent }),
  /** A fresh new chat: nothing picked. */
  reset: () => set({ project: null, collections: [], card: null, intent: null }),
};

const SERVER: ChatDraft = { project: null, collections: [], card: null, intent: null };
/** The draft, re-rendering when it changes. */
export function useChatDraft(): ChatDraft {
  return useSyncExternalStore(chatDraft.subscribe, chatDraft.get, () => SERVER);
}

/**
 * A new chat was created: put it in the drafted project, link it to the drafted card
 * (which puts the assistant on it) and give it the drafted collections, then clear
 * the draft. Failures come back as messages to show.
 */
export async function applyChatDraft(conversationId: string): Promise<string[]> {
  const { project, collections, card } = draft;
  set({ project: null, collections: [], card: null, intent: null });
  const path = `/api/conversations/${encodeURIComponent(conversationId)}`;
  const errors: string[] = [];
  const intoProject = async () => {
    if (!project) return;
    await api.request(`${path}/project`, { method: "PUT", body: { projectId: project } });
    // The card is linked once the chat is in its project, before the first reply.
    if (card) await api.request(`/api/projects/${encodeURIComponent(project)}/cards/${encodeURIComponent(card.id)}`, { method: "PATCH", body: { chatId: conversationId } });
  };
  await Promise.all([
    intoProject().catch((e: Error) => errors.push(e.message)),
    collections.length && api.request(`${path}/knowledge`, { method: "PUT", body: { collectionIds: collections } }).catch((e: Error) => errors.push(e.message)),
  ]);
  return errors;
}
