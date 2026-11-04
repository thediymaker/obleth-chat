// Chats as a list: importing them from other apps, exporting them, and the small
// changes (pin, archive) made from lists and menus.

import { toast } from "sonner";
import { api } from "@/lib/api";
import type { Conversation } from "@/lib/chat/types";
import type { ConversationSummary } from "@/lib/queries";

/** The largest export file Chat takes (the server's import limit). */
export const MAX_IMPORT_BYTES = 200 * 1024 * 1024;
/** The apps whose exports Chat reads, by the source name the server reports. */
export const SOURCES: Record<string, string> = { "open-webui": "Open WebUI", chatgpt: "ChatGPT", claude: "Claude" };
/** Conversations per page of All chats. */
export const PAGE_SIZE = 50;

const count = (n: number, one: string, many = `${one}s`) => `${Number(n).toLocaleString("en-US")} ${n === 1 ? one : many}`;
const were = (n: number) => (n === 1 ? "was" : "were");

export interface ImportResult {
  source?: string;
  imported?: number;
  alreadyImported?: number;
  overLimit?: number;
  alternativesLeftOut?: number;
  messagesLeftOut?: number;
  empty?: number;
  failed?: number;
  error?: string;
  [key: string]: unknown;
}

/** What an import did, in plain sentences. */
export function importSummary(r: ImportResult): string[] {
  const from = r.source && SOURCES[r.source] ? ` from ${SOURCES[r.source]}` : "";
  const lines: string[] = [];
  if (r.imported) lines.push(`Imported ${count(r.imported, "chat")}${from}.`);
  else if (r.alreadyImported && !r.failed)
    lines.push(`Nothing new to import${from}. ${r.alreadyImported === 1 ? "That chat is" : `All ${count(r.alreadyImported, "chat")} are`} already here.`);
  else lines.push(`No chats were imported${from}.`);
  if (r.imported && r.alreadyImported) lines.push(`${count(r.alreadyImported, "chat")} ${were(r.alreadyImported)} already here.`);
  if (r.overLimit)
    lines.push(`${count(r.overLimit, "older chat")} ${were(r.overLimit)} left out because you can keep up to 1,000 chats. Delete some and import the file again to add them.`);
  if (r.alternativesLeftOut)
    lines.push(`${count(r.alternativesLeftOut, "alternative reply or edit", "alternative replies or edits")} ${were(r.alternativesLeftOut)} left out.`);
  if (r.messagesLeftOut) lines.push(`The earliest ${count(r.messagesLeftOut, "message")} of very long chats ${were(r.messagesLeftOut)} left out.`);
  if (r.empty) lines.push(`${count(r.empty, "empty chat")} ${were(r.empty)} skipped.`);
  if (r.failed)
    lines.push(
      `${count(r.failed, "chat")} could not be imported: ${String(r.error || "something went wrong").replace(/\.?$/, ".")} Import the same file again to add ${r.failed === 1 ? "it" : "them"}.`,
    );
  return lines;
}

/** Which chats a list shows: active ones, archived ones, or both. */
export type ArchivedFilter = "false" | "true" | "all";

export interface HistoryChat extends ConversationSummary {
  pinned?: boolean;
  archived?: boolean;
  createdAt?: string;
}
export interface HistoryResult {
  conversations: HistoryChat[];
  total: number;
  hasMore: boolean;
}

/** One page of the server's chat search: titles and message text. */
export async function fetchHistory(query: string, archived: ArchivedFilter, offset: number, limit = PAGE_SIZE, signal?: AbortSignal): Promise<HistoryResult> {
  const r = await api.request<Partial<HistoryResult>>(`/api/conversations?query=${encodeURIComponent(query)}&archived=${archived}&limit=${limit}&offset=${offset}`, { signal });
  const conversations = r.conversations || [];
  return { conversations, total: r.total ?? conversations.length, hasMore: !!r.hasMore };
}

/** Why an export file can't be imported before it is sent, or null when it can. */
export function importFileError(file: { name: string; size: number }): string | null {
  if (file.size <= MAX_IMPORT_BYTES) return null;
  return /\.zip$/i.test(file.name)
    ? "This export is larger than 200 MB. Unzip it and choose the conversations.json file inside."
    : "Choose an export of 200 MB or less.";
}

/** Sends an export file as it is, only to this Chat; the server reads its format. */
export function importChats(file: File, signal?: AbortSignal): Promise<ImportResult> {
  return api.request<ImportResult>("/api/conversations/import", { method: "POST", raw: file, contentType: file.type || "application/octet-stream", signal });
}

/** Changes a chat's title, pin or archive state; returns the saved chat. Callers invalidate. */
export async function updateConversation(id: string, body: Record<string, unknown>): Promise<Conversation> {
  const data = await api.request<{ conversation?: Conversation } & Conversation>(`/api/conversations/${encodeURIComponent(id)}`, { method: "PATCH", body });
  return (data.conversation || data) as Conversation;
}

/** Archives, restores or deletes several chats at once. */
export function bulkConversations(ids: string[], action: "archive" | "unarchive" | "delete") {
  return api.request("/api/conversations/bulk", { method: "POST", body: { ids, action } });
}

/** Downloads the chats as one JSON file, and says which files were too large to include. */
export async function exportChats(ids: string[]): Promise<void> {
  if (!ids.length) return;
  const archive = await api.request<{ leftOut?: string[]; [key: string]: unknown }>("/api/conversations/export", { method: "POST", body: { ids } });
  const url = URL.createObjectURL(new Blob([JSON.stringify(archive, null, 2)], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = `obleth-chat-${new Date().toISOString().slice(0, 10)}.json`;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  const left = archive.leftOut || [];
  if (left.length)
    toast(
      `Exported without ${left.length === 1 ? `${left[0]}, which is` : `${left.length} files that are`} too large to include. The export says where ${left.length === 1 ? "it was" : "they were"}.`,
    );
}
