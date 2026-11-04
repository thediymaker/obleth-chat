import { beforeEach, describe, expect, it, vi } from "vitest";

const request = vi.fn();
vi.mock("@/lib/api", () => ({ api: { request: (...args: unknown[]) => request(...args) } }));

import { importChats, importFileError, importSummary, updateConversation } from "@/lib/chat/history";

describe("importSummary", () => {
  it("says what came over and what was left out, in plain words", () => {
    expect(importSummary({ source: "chatgpt", imported: 1204, alternativesLeftOut: 37 })).toEqual(["Imported 1,204 chats from ChatGPT.", "37 alternative replies or edits were left out."]);
    expect(importSummary({ source: "open-webui", imported: 1, alreadyImported: 2, overLimit: 1, messagesLeftOut: 30, empty: 1 })).toEqual([
      "Imported 1 chat from Open WebUI.",
      "2 chats were already here.",
      "1 older chat was left out because you can keep up to 1,000 chats. Delete some and import the file again to add them.",
      "The earliest 30 messages of very long chats were left out.",
      "1 empty chat was skipped.",
    ]);
    expect(importSummary({ source: "claude", imported: 0, alreadyImported: 5 })).toEqual(["Nothing new to import from Claude. All 5 chats are already here."]);
    expect(importSummary({ source: "obleth-chat", imported: 2 })).toEqual(["Imported 2 chats."]);
    expect(importSummary({ source: "chatgpt", imported: 100, failed: 20, error: "The service could not complete this request" })).toEqual([
      "Imported 100 chats from ChatGPT.",
      "20 chats could not be imported: The service could not complete this request. Import the same file again to add them.",
    ]);
  });
  it("says when one chat was already here, or nothing came over", () => {
    expect(importSummary({ source: "claude", alreadyImported: 1 })).toEqual(["Nothing new to import from Claude. That chat is already here."]);
    expect(importSummary({ failed: 1 })).toEqual(["No chats were imported.", "1 chat could not be imported: something went wrong. Import the same file again to add it."]);
  });
});

describe("importing", () => {
  beforeEach(() => {
    request.mockReset();
  });
  it("refuses an export over 200 MB before upload, with a hint for zips", () => {
    expect(importFileError({ name: "export.zip", size: 300 * 1024 * 1024 })).toMatch(/Unzip it and choose the conversations\.json/);
    expect(importFileError({ name: "chats.json", size: 300 * 1024 * 1024 })).toMatch(/200 MB or less/);
    expect(importFileError({ name: "chats.json", size: 1024 })).toBeNull();
  });
  it("sends the chosen export to Chat as it is", async () => {
    request.mockResolvedValue({ source: "chatgpt", imported: 2 });
    const file = new File(["{}"], "chatgpt-export.zip", { type: "application/zip" });
    expect(await importChats(file)).toEqual({ source: "chatgpt", imported: 2 });
    const [path, options] = request.mock.calls[0];
    expect(path).toBe("/api/conversations/import");
    expect(options.raw).toBe(file);
    expect(options.contentType).toBe("application/zip");
    expect(options.method).toBe("POST");
  });
  it("unwraps the conversation a change returns", async () => {
    request.mockResolvedValue({ conversation: { id: "c1", title: "T", pinned: true } });
    expect(await updateConversation("c1", { pinned: true })).toMatchObject({ id: "c1", pinned: true });
    expect(request).toHaveBeenCalledWith("/api/conversations/c1", { method: "PATCH", body: { pinned: true } });
  });
});
