import { expect, test } from "vitest";
import { conversationBusy, mergeProgress, renderSignature, responseVersion, suggestionsPending } from "./conversation-state";
import type { Conversation } from "./types";

const chat = (messages: any[], extra: any = {}) => ({ id: "chat", title: "", model: "", messages, ...extra }) as Conversation;

test("pending checkpoints require recovery while follow-up generation does not block chat", () => {
  expect(conversationBusy(chat([], { activeGenerationId: "g" }))).toBe(true);
  expect(conversationBusy(chat([{ role: "assistant", status: "pending" }]))).toBe(true);
  const completed = chat([{ role: "assistant", status: "completed", followUpsStatus: "pending" }]);
  expect(conversationBusy(completed)).toBe(false);
  expect(suggestionsPending(completed)).toBe(true);
});

test("a delayed pending checkpoint cannot replace a terminal response with blank content", () => {
  const live = chat([{ id: "answer", role: "assistant", status: "completed", content: "Finished answer", reasoningContent: "Actual thinking" }]);
  const old = chat([{ id: "answer", role: "assistant", status: "pending", content: "", reasoningContent: "" }]);
  expect(mergeProgress(old, live).messages).toEqual(live.messages);
});

test("checkpoint merge preserves each streaming channel without importing another version", () => {
  const current = chat([{ id: "v1", role: "assistant", status: "streaming", content: "Answer in progress", reasoningContent: "First thought" }]);
  const saved = chat([{ id: "v1", role: "assistant", status: "pending", content: "Answer", reasoningContent: "First thought then another" }]);
  const result = mergeProgress(saved, current).messages[0]!;
  expect(result.content).toBe("Answer in progress");
  expect(result.reasoningContent).toBe("First thought then another");
  const different = chat([{ id: "v2", role: "assistant", status: "pending", content: "" }]);
  expect(mergeProgress(different, current)).toEqual(different);
});

test("inline version selection uses sibling IDs and stays within bounds", () => {
  const message = { id: "v2", role: "assistant", content: "", versionIndex: 2, versions: [{ id: "v1" }, { id: "v2" }, { id: "v3" }] };
  expect(responseVersion(message, -1)).toBe("v1");
  expect(responseVersion(message, 1)).toBe("v3");
  expect(responseVersion({ ...message, versionIndex: 1 }, -1)).toBe(null);
  expect(responseVersion({ ...message, versionIndex: 3 }, 1)).toBe(null);
});

test("render signature ignores follow-up suggestions but tracks transcript changes", () => {
  const base = chat([
    { id: "u", role: "user", content: "Hi", status: "completed" },
    { id: "a", role: "assistant", content: "Hello", status: "completed", followUpsStatus: "pending", followUps: [] },
  ], { title: "Chat" });
  const suggested = { ...base, messages: [base.messages[0]!, { ...base.messages[1]!, followUpsStatus: "completed", followUps: ["More?"] }] };
  expect(renderSignature(suggested)).toBe(renderSignature(base));
  expect(renderSignature({ ...base, title: "Renamed" })).not.toBe(renderSignature(base));
  expect(renderSignature({ ...base, messages: [base.messages[0]!, { ...base.messages[1]!, content: "Hello there" }] })).not.toBe(renderSignature(base));
  expect(renderSignature({ ...base, activeGenerationId: "g" })).not.toBe(renderSignature(base));
});
