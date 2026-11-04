import { expect, test } from "vitest";
import { createAttachments, describe as describeFile } from "./attachments";

test("an upload waiting on the limits after session reset cannot upload into the next account", async () => {
  let release!: (v: unknown) => void;
  const requests: unknown[] = [];
  const file = new File(["hello"], "private.txt", { type: "text/plain" });
  const attachments = createAttachments({ api: { request: (path, options) => { requests.push([path, options]); return new Promise((resolve) => { release = resolve; }); } }, toast() {}, streaming: () => false });
  const pending = attachments.add([file]);
  expect(requests.length).toBe(1);
  attachments.clear();
  release({ attachments: [], limits: { maxFileBytes: 1024 } });
  await pending;
  expect(requests.length).toBe(1);
  expect(attachments.current()).toEqual([]);
});

test("long pasted text is attached as a file whose chip can show it", async () => {
  const sent: [string, any][] = [], toasts: string[] = [];
  const attachments = createAttachments({
    api: { request: async (path, options) => { if (!options) return { attachments: [], limits: { maxFileBytes: 50 * 1024 * 1024 } }; sent.push([path, options]); return { attachment: { id: "a1", name: options.raw.name, size: 9000, kind: "text" } }; } },
    toast: (m) => toasts.push(m), streaming: () => false,
  });
  const text = `<b>line</b>\n${"x".repeat(9000)}`;
  await attachments.addText(text);
  expect(sent.length).toBe(1);
  const [path, options] = sent[0]!;
  const query = new URLSearchParams(path.split("?")[1]);
  expect(query.get("name")).toMatch(/^Pasted text \d\d\.\d\d\.\d\d\.txt$/);
  expect(query.get("type")).toBe("text/plain");
  expect(options.contentType).toBe("application/octet-stream");
  expect(await options.raw.text()).toBe(text);
  expect(attachments.pastedText("a1")).toBe(text);
  expect(attachments.current().map((a) => a.id)).toEqual(["a1"]);
  attachments.remove("a1");
  expect(attachments.current()).toEqual([]);
  expect(toasts).toEqual([]);
});

test("pasted text a conversation could not hold, or pasted mid-reply, is refused out loud", async () => {
  let calls = 0, streaming = false;
  const toasts: string[] = [];
  const attachments = createAttachments({ api: { request: async () => { calls++; } }, toast: (m) => toasts.push(m), streaming: () => streaming });
  await attachments.addText("é".repeat(60 * 1024));
  streaming = true;
  await attachments.addText("short but sent while a reply streams");
  expect(calls).toBe(0);
  expect(toasts).toEqual(["That text is 120.0 KB, and a conversation can hold 100.0 KB. Paste a shorter part.", "Paste it again once the reply has finished."]);
});

test("a document chip names its type and parts, and says when Chat searches it", async () => {
  const toasts: string[] = [];
  const attachments = createAttachments({
    api: { request: async (_path, options) => (options ? { id: "d1", name: options.raw.name, size: 3 * 1024 * 1024, kind: "document", parts: 120, searched: true } : { attachments: [], limits: { maxFileBytes: 20 * 1024 * 1024 } }) },
    toast: (m) => toasts.push(m), streaming: () => false,
  });
  const file = new File([new Uint8Array(8)], "Thesis.pdf", { type: "application/pdf" });
  Object.defineProperty(file, "size", { value: 3 * 1024 * 1024 });
  await attachments.add([file]);
  expect(toasts).toEqual([]);
  const d = describeFile(attachments.current()[0]!);
  expect(d.detail).toBe("PDF · 120 pages · 3.0 MB");
  expect(d.note).toBe("Large document: Chat searches it instead of reading it all.");
  expect(describeFile({ name: "deck.pptx", kind: "document", size: 2048, parts: 1 }).detail).toBe("PowerPoint · 1 slide · 2.0 KB");
  expect(describeFile({ name: "Budget.XLSX", kind: "document", size: 2048, parts: 3 }).glyph).toBe("grid");
  expect(describeFile({ name: "memo.docx", kind: "document", size: 512 }).detail).toBe("Word · 512 B");
  // Files over the server's limit are refused before they are sent.
  const big = new File(["x"], "Thesis.pdf", { type: "application/pdf" });
  Object.defineProperty(big, "size", { value: 21 * 1024 * 1024 });
  await attachments.add([big]);
  expect(toasts).toEqual(["Thesis.pdf is 21.0 MB, and files can be at most 20.0 MB."]);
});

test("any file can be attached, and an archive chip says how many files it holds", async () => {
  const attachments = createAttachments({
    api: { request: async (_path, options) => (options ? { id: "z1", name: "site.zip", size: 1.8 * 1024 * 1024, kind: "file", parts: 14 } : { attachments: [], limits: { maxFileBytes: 50 * 1024 * 1024 } }) },
    toast: (m) => { throw new Error(m); }, streaming: () => false,
  });
  await attachments.add([new File(["zip"], "site.zip", { type: "application/zip" })]);
  expect(describeFile(attachments.current()[0]!)).toEqual({ glyph: "archive", detail: "ZIP · 14 files · 1.8 MB", note: "" });
  expect(describeFile({ name: "code.tar.gz", kind: "file", size: 2048, parts: 1 }).detail).toBe("TAR.GZ · 1 file · 2.0 KB");
  expect(describeFile({ name: "code.tar.gz", kind: "file", size: 2048 }).glyph).toBe("archive");
  expect(describeFile({ name: "results.parquet", kind: "file", size: 4096 })).toEqual({ glyph: "file", detail: "PARQUET · 4.0 KB", note: "" });
});

test("at most four files go with one message", async () => {
  const toasts: string[] = [];
  const attachments = createAttachments({ api: { request: async () => ({}) }, toast: (m) => toasts.push(m), streaming: () => false });
  await attachments.add([1, 2, 3, 4, 5].map((n) => new File(["x"], `f${n}.txt`)));
  expect(toasts).toEqual(["Attach up to four files per message."]);
});
