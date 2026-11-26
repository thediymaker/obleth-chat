"use client";

// A project's notebook (api/src/notebook): its sources with the person's ticks and
// their guides, the chat that answers from them, the Notebook model and the meter.

import { useQuery, type QueryClient } from "@tanstack/react-query";
import type { KnowledgeDocument } from "@/components/knowledge/api";
import { api } from "@/lib/api";
import { ago } from "@/lib/format";

export interface Guide {
  status: "running" | "ready" | "failed";
  summary?: string;
  questions?: string[];
}
export interface Source extends KnowledgeDocument {
  ticked: boolean;
  guide: Guide | null;
  /** A web page: the address it was read from, and when. */
  sourceUrl?: string | null;
  readAt?: string | null;
  /** The page being read again, or why that last failed; only for whoever asked. */
  reading?: { id: string; status: "reading" | "failed"; error: string | null } | null;
}
/** A web page someone asked for that is not a source yet: being read in their browser, or failed. */
export interface PageRead {
  id: string;
  url: string;
  status: "reading" | "failed";
  error: string | null;
  requestedAt: string;
}
export interface SourceGroup {
  id: string;
  name: string;
  mine: boolean;
  /** The newest documents, up to 300; `total` counts them all. */
  documents: Source[];
  total: number;
  /** The person's pages on their way into this collection. */
  reading?: PageRead[];
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
  /** What the person made from the sources here, newest first. */
  items: NotebookItem[];
  /** Web pages can be added: the browser is on for this server and this person. */
  webPages?: boolean;
}

export const notebookPath = (projectId: string) => `/api/projects/${encodeURIComponent(projectId)}/notebook`;
export const notebookKey = (projectId: string) => ["notebook", projectId] as const;
export const notebookHref = (projectId: string, chatId?: string | null) => `/projects/${encodeURIComponent(projectId)}/notebook${chatId ? `/${encodeURIComponent(chatId)}` : ""}`;

const working = (n: Notebook) =>
  n.items?.some((i) => i.status === "running") ||
  n.collections.some((g) => g.reading?.some((r) => r.status === "reading") || g.documents.some((d) => d.status === "queued" || d.status === "processing" || d.guide?.status === "running" || d.reading?.status === "reading"));

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

/** Web pages one Add may give, as the server allows. */
export const PAGES_AT_ONCE = 10;

/** The addresses in what the person typed: one per line, blank lines left out. */
export const addressesOf = (text: string) =>
  text
    .split(/\s+/)
    .map((a) => a.trim())
    .filter(Boolean);

/** Reads pages into the notebook, one after another in the person's browser; the answer is the notebook as it now is. */
export const addPages = (projectId: string, addresses: string[]) => api.request<Notebook>(`${notebookPath(projectId)}/pages`, { method: "POST", body: { addresses } });
/** Reads a web page source again, keeping it and its tick. */
export const readAgain = (projectId: string, documentId: string) => api.request<Notebook>(`${notebookPath(projectId)}/sources/${encodeURIComponent(documentId)}/read`, { method: "POST" });
export const retryRead = (id: string) => api.request<Notebook>(`/api/notebook/reads/${encodeURIComponent(id)}/retry`, { method: "POST" });
export const removeRead = (id: string) => api.request(`/api/notebook/reads/${encodeURIComponent(id)}`, { method: "DELETE" });
/** Adds the page a chat's browser tab shows now to a notebook: the chat's project's, or the one given. */
export const addTabPage = (conversationId: string, projectId?: string) =>
  api.request<{ projectId: string; documentId: string; name: string }>(`/api/conversations/${encodeURIComponent(conversationId)}/browser/notebook`, { method: "POST", body: projectId ? { projectId } : {} });

/** An address's site, without www. */
export function siteOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return "";
  }
}
/** A link to an address, only when it is a web one. */
export const webHref = (url: string | null | undefined) => (url && /^https?:\/\//i.test(url) ? url : undefined);
/** What a source is called: a web page by its title, without the .md it is kept as. */
export const sourceName = (d: Pick<Source, "name" | "sourceUrl">) => (d.sourceUrl ? d.name.replace(/\.md$/i, "") : d.name);

/** A source's line under its name: its kind and length, or how indexing goes. */
export function sourceMeta(d: Source): string {
  if (d.sourceUrl) return [siteOf(d.sourceUrl), `${d.mimeType.includes("pdf") ? "PDF" : "Web page"} · read ${ago(d.readAt ?? d.createdAt)}`].filter(Boolean).join(" · ");
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

/** Something made from the sources, as the notebook lists it; opened, with its text and citations. */
export interface NotebookItem {
  id: string;
  kind: string;
  title: string;
  status: "running" | "ready" | "failed";
  sources: { id: string; name: string }[];
  /** Verified quotations, shown as [n]. */
  cited: number;
  /** Quotations not found in their source, never shown as citations. */
  unverified: number;
  error: string | null;
  createdAt: string;
  finishedAt: string | null;
  body?: string;
  citations?: Citation[];
}
export interface Citation {
  n: number;
  documentId: string;
  name: string;
  page: number | null;
  section: string | null;
  quote: string;
}

/** What can be made: kind, name, and a few words on it. */
export const KINDS: [string, string, string][] = [
  ["briefing", "Briefing", "Two pages"],
  ["faq", "FAQ", "What people ask"],
  ["study_guide", "Study guide", "With questions"],
  ["timeline", "Timeline", "Who found what, when"],
];

/** Starts making `kind` from the ticked sources. */
export const startItem = (projectId: string, kind: string) => api.request<{ item: NotebookItem }>(`${notebookPath(projectId)}/items`, { method: "POST", body: { kind } });
export const removeItem = (id: string) => api.request(`/api/notebook/items/${encodeURIComponent(id)}`, { method: "DELETE" });
export function useNotebookItem(id: string | null) {
  return useQuery({
    queryKey: ["notebook-item", id],
    queryFn: async () => (await api.request<{ item: NotebookItem }>(`/api/notebook/items/${encodeURIComponent(id!)}`)).item,
    enabled: !!id,
  });
}
/** An item's citations as the chat's sources, so [n] link and quote the same way. */
export const citationItems = (item: NotebookItem) => (item.citations ?? []).map((c) => ({ n: c.n, documentId: c.documentId, name: c.name, page: c.page, section: c.section, excerpt: c.quote }));
/** Why nothing can be made while the ticked sources do not fit. */
export const meterRefusal = (m: Meter) => `The ticked sources are too long for the Notebook model to read at once: they come to about ${m.percent}% of what it can read. Untick some sources to make something from them.`;
