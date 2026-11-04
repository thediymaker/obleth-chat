"use client";

// Knowledge: collections of documents a chat can search (api/src/knowledge/mod.rs).

import { type QueryClient, useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";

export const ACCEPT =
  ".pdf,.docx,.pptx,.xlsx,.html,.htm,.md,.markdown,.txt,.rst,.tex,.csv,.tsv,.json,.jsonl,.yaml,.yml,.toml,.xml,.ipynb,.py,.rs,.go,.java,.c,.h,.cc,.cpp,.hpp,.js,.mjs,.ts,.tsx,.jsx,.sh,.sql,.r,.jl,.m,.f90,.cu,.log";
export const MAX_PER_CHAT = 8;
export const DEFAULT_MAX_FILE = 50 * 1048576;

export type Scope = "personal" | "group" | "everyone";
export interface Collection {
  id: string;
  name: string;
  description: string;
  scope: Scope;
  groupId: string | null;
  groupName: string | null;
  mine: boolean;
  ownerName: string;
  updatedAt?: string;
  documents: number;
  processing: number;
  problems: number;
  passages: number;
  bytes: number;
}
export interface CollectionList {
  collections: Collection[];
  usage?: { used: number; limit: number };
  groups: { id: string; name: string }[];
  maxFileBytes?: number;
}
export type DocumentStatus = "queued" | "processing" | "ready" | "needs_ocr" | "failed";
export interface KnowledgeDocument {
  id: string;
  name: string;
  mimeType: string;
  size: number;
  status: DocumentStatus;
  stage: string;
  done: number;
  total: number;
  pages: number | null;
  passages: number;
  error: string | null;
  createdAt: string;
}
export interface CollectionDetail {
  collection: Omit<Collection, "documents" | "processing" | "problems" | "passages" | "bytes">;
  documents: KnowledgeDocument[];
}
export interface SearchResult {
  n: number;
  documentId: string;
  collectionId: string;
  name: string;
  page: number | null;
  section: string | null;
  excerpt: string;
  matched: string;
  saved?: boolean;
}

export const size = (n: number) =>
  n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(0)} KB` : n < 1073741824 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1073741824).toFixed(2)} GB`;
export const fileUrl = (id: string, page?: number | null) => `/api/knowledge/documents/${encodeURIComponent(id)}/file${page ? `#page=${page}` : ""}`;
export const busy = (d: Pick<KnowledgeDocument, "status">) => d.status === "queued" || d.status === "processing";
export const scopeLabel = (c: Pick<Collection, "scope" | "groupName">) => (c.scope === "everyone" ? "Everyone" : c.scope === "group" ? c.groupName || "Group" : "Personal");
export const collectionPath = (id: string) => `/api/knowledge/collections/${encodeURIComponent(id)}`;

export const knowledgeKeys = {
  list: ["knowledge", "collections"] as const,
  detail: (id: string) => ["knowledge", "collection", id] as const,
  chat: (conversationId: string) => ["conversation-knowledge", conversationId] as const,
};

/** Every collection the person can search. Polls while any is indexing when `poll` is set. */
export function useCollections(enabled = true, poll = false) {
  return useQuery({
    queryKey: knowledgeKeys.list,
    queryFn: () => api.request<CollectionList>("/api/knowledge/collections"),
    enabled,
    refetchInterval: (q) => (poll && q.state.data?.collections?.some((c) => c.processing > 0) ? 2500 : false),
  });
}
export function useCollection(id: string) {
  return useQuery({
    queryKey: knowledgeKeys.detail(id),
    queryFn: () => api.request<CollectionDetail>(collectionPath(id)),
    enabled: !!id,
    retry: (n, e) => (e as { status?: number }).status !== 404 && n < 2,
    refetchInterval: (q) => (q.state.data?.documents?.some(busy) ? 2500 : false),
  });
}
export function refreshKnowledge(client: QueryClient, id?: string) {
  client.invalidateQueries({ queryKey: knowledgeKeys.list });
  if (id) client.invalidateQueries({ queryKey: knowledgeKeys.detail(id) });
}
