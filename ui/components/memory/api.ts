"use client";

// Memory: what Chat learned about the person, linked to topics. Endpoints in
// api/src/memory.rs.

import { type QueryClient, useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { useSignedIn } from "@/lib/queries";

export type MemoryKind = "about" | "work" | "preference" | "goal";
export type TopicKind = "project" | "tool" | "field" | "course" | "topic";

export interface Memory {
  id: string;
  kind: MemoryKind;
  content: string;
  origin: "learned" | "added";
  pinned: boolean;
  uses: number;
  usedAt: string | null;
  createdAt: string;
  updatedAt: string;
  source: { conversationId: string; title: string | null } | null;
  topics: string[];
}
export interface Topic {
  id: string;
  name: string;
  kind: TopicKind;
  count: number;
}
export interface MemoryState {
  available: boolean;
  useMemory: boolean;
  learn: boolean;
  max: number;
  memories: Memory[];
  topics: Topic[];
}

export const MEMORY_KEY = ["memory"] as const;
export const memoryPath = (id: string) => `/api/memory/${encodeURIComponent(id)}`;
export const topicPath = (id: string) => `/api/memory/topics/${encodeURIComponent(id)}`;

export function useMemory() {
  return useQuery({ queryKey: MEMORY_KEY, queryFn: () => api.request<MemoryState>("/api/memory"), enabled: useSignedIn() });
}
export const refreshMemory = (client: QueryClient) => client.invalidateQueries({ queryKey: MEMORY_KEY });

/** [label, one-line meaning] for each kind of memory. */
export const MEMORY_KINDS: Record<MemoryKind, [string, string]> = {
  about: ["About you", "Who you are: your role, field and background"],
  work: ["Your work", "Projects, research, courses and the tools you use"],
  preference: ["Preference", "How you like answers"],
  goal: ["Goal", "What you want to achieve"],
};
/** [label, CSS color] for each kind of topic. */
export const TOPIC_KINDS: Record<TopicKind, [string, string]> = {
  project: ["Project", "var(--project-violet)"],
  tool: ["Tool", "var(--project-blue)"],
  field: ["Field", "var(--project-green)"],
  course: ["Course", "var(--project-gold)"],
  topic: ["Topic", "var(--project-rose)"],
};
export const topicColor = (kind: string | undefined) => TOPIC_KINDS[kind as TopicKind]?.[1] ?? "var(--project-gray)";

export interface MemoryInput {
  content?: string;
  kind?: MemoryKind;
  pinned?: boolean;
  topics?: string[];
}
export const addMemory = (body: MemoryInput) => api.request<{ memory: Memory }>("/api/memory", { method: "POST", body });
export const changeMemory = (id: string, body: MemoryInput) => api.request<{ memory: Memory }>(memoryPath(id), { method: "PATCH", body });
export const deleteMemory = (id: string) =>
  api.request(memoryPath(id), { method: "DELETE" }).catch((e: { status?: number }) => {
    if (e.status !== 404) throw e;
  });
export const saveChoices = (body: { useMemory: boolean; learn: boolean }) => api.request("/api/memory/settings", { method: "PUT", body });

/** "just now", "5 min ago", "3 h ago", "2 days ago", or a date. */
export function ago(at: string | null | undefined): string {
  if (!at) return "";
  const s = (Date.now() - new Date(at).getTime()) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 86400 * 14) return `${Math.floor(s / 86400)} day${s < 86400 * 2 ? "" : "s"} ago`;
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

/** Topic names typed as "a, b, c": trimmed, three at most, no repeats. */
export function topicNames(text: string): string[] {
  const out: string[] = [];
  for (const part of text.split(",")) {
    const name = part.trim().replace(/\s+/g, " ");
    if (name && !out.some((n) => n.toLowerCase() === name.toLowerCase())) out.push(name);
  }
  return out.slice(0, 3);
}
