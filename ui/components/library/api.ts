"use client";

// Library: things saved while working (functions, notes, links, excerpts) and the
// images models generated. Endpoints in api/src/knowledge/saved.rs and api/src/images.rs.

import { Bookmark, Code, File, Link as LinkIcon, type LucideIcon } from "lucide-react";
import { type QueryClient, useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { keys } from "@/lib/queries";

export type SavedKind = "code" | "note" | "link" | "excerpt";
export interface SavedItem {
  id: string;
  ownerId: string;
  ownerName: string | null;
  projectId: string | null;
  pinned: boolean;
  kind: SavedKind;
  title: string;
  body: string;
  language: string;
  url: string | null;
  tags: string[];
  searchable: boolean;
  indexed: boolean;
  source: { conversationId: string; messageId: string | null; title: string | null } | null;
  createdAt: string;
  updatedAt: string;
}
export interface SavedList {
  items: SavedItem[];
  more: boolean;
  tags: { tag: string; count: number }[];
  kinds: { kind: SavedKind; count: number }[];
}
export interface SavedFilters {
  q?: string;
  kind?: string;
  tag?: string;
  project?: string;
}

/** [icon, plural label, one] for each kind. */
export const KINDS: Record<SavedKind, [LucideIcon, string, string]> = {
  code: [Code, "Code", "Function"],
  note: [File, "Notes", "Note"],
  link: [LinkIcon, "Links", "Link"],
  excerpt: [Bookmark, "Excerpts", "Excerpt"],
};
export const kindOf = (kind: string) => KINDS[kind as SavedKind] || KINDS.note;
/** "Function · python", "Note". */
export const kindLabel = (item: Pick<SavedItem, "kind" | "language">) => {
  const label = kindOf(item.kind)[2];
  return item.kind === "code" && item.language ? `${label} · ${item.language}` : label;
};
export const imageSize = (n: number) => (n < 1048576 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / 1048576).toFixed(1)} MB`);
export const host = (url: string) => {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
};
/** Tags from what someone typed: "#a, b #A" → ["a", "b"], at most 12. */
export const parseTags = (text: string) =>
  [
    ...new Set(
      String(text || "")
        .split(/[\s,]+/)
        .map((t) => t.replace(/^#/, "").toLowerCase())
        .filter(Boolean),
    ),
  ].slice(0, 12);
/** Tags worth offering: the code's language and the person's own tags that the text mentions. */
export function suggestTags(item: { title?: string; body?: string; language?: string; tags?: string[] }, known: { tag: string }[]): string[] {
  const text = `${item.title || ""} ${item.body || ""}`.toLowerCase();
  const out: string[] = [];
  if (item.language) out.push(item.language.toLowerCase());
  for (const t of known) if (out.length < 5 && t.tag.length > 2 && text.includes(t.tag.replace(/-/g, " ")) && !out.includes(t.tag)) out.push(t.tag);
  return out.filter((t) => !(item.tags || []).includes(t));
}

export const libraryKey = (filters: SavedFilters = {}) => ["library", "saved", filters] as const;
export const savedPath = (id: string) => `/api/saved/${encodeURIComponent(id)}`;
export const savedQuery = (filters: SavedFilters) => {
  const q = new URLSearchParams(Object.entries(filters).filter(([, v]) => v) as [string, string][]);
  return `/api/saved${q.size ? `?${q}` : ""}`;
};
export function useSaved(filters: SavedFilters = {}, enabled = true) {
  return useQuery({ queryKey: libraryKey(filters), queryFn: () => api.request<SavedList>(savedQuery(filters)), enabled });
}
export const getSaved = async (id: string) => (await api.request<{ item: SavedItem }>(savedPath(id))).item;

/** After a saved item changes: Library, project pages (their saved lists) and project counts. */
export function refreshSaved(client: QueryClient) {
  client.invalidateQueries({ queryKey: ["library"] });
  client.invalidateQueries({ queryKey: ["project"] });
  client.invalidateQueries({ queryKey: keys.projects });
}

export interface GeneratedImage {
  id: string;
  url: string;
  conversationId: string;
  conversationTitle: string;
  size: number;
  createdAt: string;
}
export function useGeneratedImages(enabled = true) {
  return useQuery({
    queryKey: ["library", "images"],
    queryFn: () => api.request<{ images?: GeneratedImage[]; usage?: { used: number; limit: number } }>("/api/images"),
    enabled,
  });
}
