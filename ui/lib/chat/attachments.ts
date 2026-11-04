// Files attached to the next message: picked, pasted or dropped, uploaded as they
// are, at most four per message. A small store the composer subscribes to.

import type { Attachment } from "./types";

export const sizeLabel = (n: number) =>
  n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : n < 1024 ** 3 ? `${(n / 1024 / 1024).toFixed(1)} MB` : `${(n / 1024 ** 3).toFixed(1)} GB`;
// Text a conversation can hold in all (store::MAX_CONTEXT_BYTES); pasted text over it can't be read whole.
export const CONTEXT_BYTES = 100 * 1024;
export const MAX_FILES = 4;
const stamp = () => new Date().toTimeString().slice(0, 8).replaceAll(":", ".");
// Documents Chat reads the text of: what each is called, its icon and what its parts are.
const DOCUMENTS: Record<string, [string, string, string]> = { pdf: ["PDF", "file", "page"], docx: ["Word", "file", ""], pptx: ["PowerPoint", "monitor", "slide"], xlsx: ["Excel", "grid", "sheet"] };
const extension = (name: string) => (name.match(/\.([^.]+)$/)?.[1] || "").toLowerCase();
// Archives, by what their names end with, and what each is called.
const ARCHIVES: [string, string][] = [[".tar.gz", "TAR.GZ"], [".tgz", "TGZ"], [".tar.bz2", "TAR.BZ2"], [".tar.xz", "TAR.XZ"], [".tar", "TAR"], [".zip", "ZIP"], [".7z", "7Z"], [".rar", "RAR"], [".gz", "GZ"], [".xz", "XZ"], [".bz2", "BZ2"]];
const archive = (name: string) => ARCHIVES.find(([end]) => name.toLowerCase().endsWith(end))?.[1];

/** The icon and the line under a file's name: "PDF · 12 pages · 1.2 MB", "ZIP · 14 files · 1.8 MB". */
export function describe(a: Pick<Attachment, "name" | "kind" | "size" | "parts" | "searched">): { glyph: string; detail: string; note: string } {
  if (a.kind === "file") {
    const type = archive(a.name);
    const entries = type && a.parts ? `${a.parts.toLocaleString()} file${a.parts === 1 ? "" : "s"}` : "";
    return { glyph: type ? "archive" : "file", detail: [type || extension(a.name).toUpperCase(), entries, sizeLabel(a.size)].filter(Boolean).join(" · "), note: "" };
  }
  const doc = a.kind === "document" && DOCUMENTS[extension(a.name)];
  if (!doc) return { glyph: a.kind === "image" ? "image" : "file", detail: sizeLabel(a.size), note: "" };
  const [type, glyph, unit] = doc;
  const parts = unit && a.parts ? `${a.parts.toLocaleString()} ${unit}${a.parts === 1 ? "" : "s"}` : "";
  return { glyph, detail: [type, parts, sizeLabel(a.size)].filter(Boolean).join(" · "), note: a.searched ? "Large document: Chat searches it instead of reading it all." : "" };
}
export const attachmentUrl = (a: Pick<Attachment, "id" | "url">) => (a.url as string) || `/api/attachments/${encodeURIComponent(a.id)}`;

interface Api {
  request: (path: string, options?: any) => Promise<any>;
}
export interface AttachmentsOptions {
  api: Api;
  toast: (message: string) => void;
  /** Whether a reply is being written now; nothing is added meanwhile. */
  streaming: () => boolean;
}

export function createAttachments({ api, toast, streaming }: AttachmentsOptions) {
  // The server's limits (GET /api/attachments), read once per session.
  let selected: Attachment[] = [],
    uploading = false,
    sequence = 0,
    limits: { maxFileBytes?: number } | null = null;
  // Pasted text by attachment id, so its chip can show it.
  const pasted = new Map<string, string>();
  const listeners = new Set<() => void>();
  let snapshot = { selected, uploading, version: 0 };
  const changed = () => {
    snapshot = { selected: [...selected], uploading, version: snapshot.version + 1 };
    listeners.forEach((l) => l());
  };

  /** Files from the picker, a paste or a drop. Pasted screenshots arrive as image.png. */
  async function add(input: File[], text?: string) {
    const files = input.map((file, i) =>
      file.name === "image.png" || !file.name
        ? new File([file], `Pasted image ${stamp()}${input.length > 1 ? `-${i + 1}` : ""}.${file.type.split("/")[1] || "png"}`, { type: file.type })
        : file,
    );
    if (!files.length || streaming() || uploading) return;
    if (selected.length + files.length > MAX_FILES) {
      toast("Attach up to four files per message.");
      return;
    }
    const epoch = sequence;
    uploading = true;
    changed();
    try {
      if (!limits) {
        try {
          limits = (await api.request("/api/attachments"))?.limits || null;
        } catch (e) {
          if ((e as Error).name === "AbortError") throw e;
        }
      }
      if (epoch !== sequence) return;
      const most = limits?.maxFileBytes;
      for (const file of files) {
        if (most && file.size > most) throw new Error(`${file.name} is ${sizeLabel(file.size)}, and files can be at most ${sizeLabel(most)}.`);
        // The file's own bytes, unread here and not re-encoded.
        const query = new URLSearchParams({ name: file.name, type: file.type || "" });
        const r = await api.request(`/api/attachments?${query}`, { method: "POST", raw: file, contentType: "application/octet-stream" });
        if (epoch !== sequence) return;
        const attachment: Attachment = r.attachment || r;
        selected.push(attachment);
        if (text !== undefined) pasted.set(attachment.id, text);
      }
    } catch (e) {
      if (epoch === sequence && (e as Error).name !== "AbortError") toast((e as Error).message);
    } finally {
      if (epoch === sequence) {
        uploading = false;
        changed();
      }
    }
  }
  /** Text too long for the message box, attached as a file instead; nothing is cut off. */
  async function addText(text: string) {
    const bytes = new TextEncoder().encode(text).length;
    if (bytes > CONTEXT_BYTES) {
      toast(`That text is ${sizeLabel(bytes)}, and a conversation can hold ${sizeLabel(CONTEXT_BYTES)}. Paste a shorter part.`);
      return;
    }
    if (streaming() || uploading) {
      toast(streaming() ? "Paste it again once the reply has finished." : "Paste it again once the files have uploaded.");
      return;
    }
    await add([new File([text], `Pasted text ${stamp()}.txt`, { type: "text/plain" })], text);
  }
  return {
    add,
    addText,
    /** Picked from the person's stored files. */
    attachExisting(a: Attachment) {
      if (streaming() || selected.length >= MAX_FILES || selected.some((s) => s.id === a.id)) return false;
      selected.push(a);
      changed();
      return true;
    },
    remove(id: string) {
      selected = selected.filter((a) => a.id !== id);
      pasted.delete(id);
      changed();
    },
    pastedText: (id: string) => pasted.get(id),
    /** Puts back files a failed send took, after what is there now, up to four. */
    restore(files: Attachment[]) {
      selected = [...selected, ...files.filter((a) => !selected.some((b) => a.id === b.id))].slice(0, MAX_FILES);
      changed();
    },
    /** Forgets everything, and makes uploads still running land nowhere. */
    clear() {
      sequence++;
      selected = [];
      uploading = false;
      limits = null;
      pasted.clear();
      changed();
    },
    current: () => [...selected],
    consume() {
      selected = [];
      changed();
    },
    busy: () => uploading,
    full: () => uploading || selected.length >= MAX_FILES,
    setLimits(value: { maxFileBytes?: number } | null) {
      limits = value || limits;
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    snapshot: () => snapshot,
  };
}
export type Attachments = ReturnType<typeof createAttachments>;
