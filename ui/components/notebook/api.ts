"use client";

// A project's notebook (api/src/notebook): its sources with the person's ticks and
// their guides, the chat that answers from them, the Notebook model and the meter.

import { useQuery, type QueryClient } from "@tanstack/react-query";
import type { KnowledgeDocument } from "@/components/knowledge/api";
import { api } from "@/lib/api";

export interface Guide {
  status: "running" | "ready" | "failed";
  summary?: string;
  questions?: string[];
}
export interface Source extends KnowledgeDocument {
  ticked: boolean;
  guide: Guide | null;
}
export interface SourceGroup {
  id: string;
  name: string;
  mine: boolean;
  /** The newest documents, up to 300; `total` counts them all. */
  documents: Source[];
  total: number;
}
export interface NotebookModel {
  set: boolean;
  available: boolean;
  name?: string;
  contextWindow?: number | null;
}
export interface Meter {
  chars: number;
  tokens: number;
  capacity: number;
  percent: number;
  fits: boolean;
}
export interface Notebook {
  chatId: string | null;
  onlySources: boolean;
  collections: SourceGroup[];
  ticked: number;
  total: number;
  model: NotebookModel;
  /** With a Notebook model whose window the gateway reports. */
  meter: Meter | null;
  /** The project's collections of this person's, where Add puts files. */
  addTo: string[];
  /** The project's owner can have a collection made for added sources. */
  canMakeCollection: boolean;
}

export const notebookPath = (projectId: string) => `/api/projects/${encodeURIComponent(projectId)}/notebook`;
export const notebookKey = (projectId: string) => ["notebook", projectId] as const;
export const notebookHref = (projectId: string, chatId?: string | null) => `/projects/${encodeURIComponent(projectId)}/notebook${chatId ? `/${encodeURIComponent(chatId)}` : ""}`;

const working = (n: Notebook) => n.collections.some((g) => g.documents.some((d) => d.status === "queued" || d.status === "processing" || d.guide?.status === "running"));

/** The notebook, asked again every few seconds while a source is indexed or its guide written. */
export function useNotebook(projectId: string) {
  return useQuery({
    queryKey: notebookKey(projectId),
    queryFn: () => api.request<Notebook>(notebookPath(projectId)),
    enabled: !!projectId,
    refetchInterval: (q) => (q.state.data && working(q.state.data) ? 3000 : false),
  });
}

/** Ticks or unticks sources; the answer is the notebook as it now is. */
export async function setTicked(client: QueryClient, projectId: string, documentIds: string[], ticked: boolean) {
  const key = notebookKey(projectId);
  const before = client.getQueryData<Notebook>(key);
  if (before) {
    const ids = new Set(documentIds);
    client.setQueryData<Notebook>(key, {
      ...before,
      collections: before.collections.map((g) => ({ ...g, documents: g.documents.map((d) => (ids.has(d.id) ? { ...d, ticked } : d)) })),
    });
  }
  try {
    client.setQueryData(key, await api.request<Notebook>(`${notebookPath(projectId)}/sources`, { method: "PUT", body: { documentIds, ticked } }));
  } catch (e) {
    if (before) client.setQueryData(key, before);
    throw e;
  }
}

/** Starts a chat in the notebook with the person's model. */
export const startChat = (projectId: string, model: string) => api.request<{ chatId: string; onlySources: boolean }>(`${notebookPath(projectId)}/chat`, { method: "POST", body: { model } });

/** "Only from the sources" for a notebook chat, from its next message. */
export const setOnlySources = (chatId: string, onlySources: boolean) => api.request<{ onlySources: boolean }>(`/api/conversations/${encodeURIComponent(chatId)}/notebook`, { method: "PUT", body: { onlySources } });

/** A collection for added sources, made and attached for the project's owner when none of the project's is theirs. */
export const makeCollection = (projectId: string) => api.request<{ collectionId: string }>(`${notebookPath(projectId)}/collection`, { method: "POST" });

/** The ticked sources' suggested questions, a few from each source in turn. */
export function suggestions(n: Notebook | undefined, most = 4): string[] {
  const lists = (n?.collections ?? []).flatMap((g) => g.documents.filter((d) => d.ticked && d.guide?.status === "ready").map((d) => d.guide?.questions ?? []));
  const out: string[] = [];
  for (let i = 0; out.length < most && lists.some((l) => l.length > i); i++) {
    for (const list of lists) {
      const q = list[i];
      if (q && !out.includes(q) && out.length < most) out.push(q);
    }
  }
  return out;
}

/** The meter's sentence. */
export const meterText = (m: Meter) => (m.fits ? `Ticked sources use ${m.percent}% of what the notebook can read` : `Ticked sources are more than the notebook can read (${m.percent}%)`);

/** A source's line under its name: its kind and length, or how indexing goes. */
export function sourceMeta(d: Source): string {
  const kind = d.mimeType.includes("pdf") ? "PDF" : d.mimeType.includes("word") ? "Word" : d.mimeType.includes("presentation") ? "Slides" : d.mimeType.includes("sheet") ? "Spreadsheet" : d.mimeType.includes("markdown") ? "Text" : d.mimeType.includes("html") ? "Web page" : "Text";
  if (d.pages) return `${kind} · ${d.pages} ${d.pages === 1 ? "page" : "pages"}`;
  return `${kind} · ${d.passages} ${d.passages === 1 ? "passage" : "passages"}`;
}

/** The text of an answer to save as a note, with the sources its [n] cite listed under it. */
export function noteBody(content: string, items: { n: number; name: string; page?: number | null }[]): string {
  const cited = new Set([...content.matchAll(/\[(\d{1,2}(?:\s*,\s*\d{1,2})*)\]/g)].flatMap((m) => m[1]!.split(",").map((n) => Number(n.trim()))));
  const lines = items.filter((s) => cited.has(s.n)).map((s) => `[${s.n}] ${s.name}${s.page ? `, page ${s.page}` : ""}`);
  return lines.length ? `${content.trim()}\n\nSources\n${lines.join("\n")}` : content.trim();
}
