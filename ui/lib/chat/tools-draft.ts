// A new chat's tool choices before it exists, and storing them once it does: which
// connections are switched off and whether Auto mode is on. The chat page reads the
// draft through useChatTools (components/connections/chat-tools.tsx).

import type { QueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";

export interface ToolsDraft {
  off: Set<string>;
  auto: boolean;
}

let draft: ToolsDraft = { off: new Set(), auto: false };
const listeners = new Set<() => void>();

export const toolsDraft = () => draft;
export function subscribeToolsDraft(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
export function setToolsDraft(next: Partial<ToolsDraft>) {
  draft = { ...draft, ...next };
  listeners.forEach((l) => l());
}

export const chatToolsPath = (id: string) => `/api/conversations/${encodeURIComponent(id)}/connections`;
export const chatToolsKey = (id: string | null) => ["chat-tools", id ?? ""] as const;

/** Stores what was chosen before a new chat existed. */
export async function applyToolsDraft(id: string, queries?: QueryClient) {
  const off = [...draft.off];
  const auto = draft.auto;
  setToolsDraft({ off: new Set(), auto: false });
  if (auto) await api.request(chatToolsPath(id), { method: "PATCH", body: { auto: true } }).catch(() => {});
  await Promise.all(off.map((connectionId) => api.request(chatToolsPath(id), { method: "PATCH", body: { connectionId, on: false } }).catch(() => {})));
  await queries?.invalidateQueries({ queryKey: chatToolsKey(id) });
}

/** Research turns Auto mode on for its chat, where the server allows it. */
export async function enableAuto(id: string, autoAllowed: boolean, queries?: QueryClient) {
  if (!autoAllowed) return;
  await api.request(chatToolsPath(id), { method: "PATCH", body: { auto: true } }).catch(() => {});
  await queries?.invalidateQueries({ queryKey: chatToolsKey(id) });
}

/** A new chat starts from every active connection again, with Auto off. */
export const resetToolsDraft = () => setToolsDraft({ off: new Set(), auto: false });
/** A new chat that should start in Auto mode, such as research from a project. */
export const startAutoDraft = () => setToolsDraft({ auto: true });
