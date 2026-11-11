// The chat store's send and operation bookkeeping against a fake server: the
// stream sequences api/src/lib.rs writes (meta, reasoning, delta, tool, done,
// error), stopping, reconnecting after a dropped stream, regenerate and edit, and
// leaving a temporary chat.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { StreamEvent } from "@/lib/api";
import { ChatStore, routeOf } from "./store";
import type { Conversation, Message } from "./types";

type Call = { path: string; method: string; body?: any };
type Handler = (call: Call) => unknown;

class HttpError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}

/** A stream the test feeds event by event, or ends with an error. */
function channel() {
  const queue: (StreamEvent | Error | null)[] = [];
  let wake: (() => void) | null = null;
  const push = (item: StreamEvent | Error | null) => {
    queue.push(item);
    wake?.();
  };
  async function* events(): AsyncGenerator<StreamEvent> {
    for (;;) {
      if (!queue.length) await new Promise<void>((r) => (wake = r));
      wake = null;
      const item = queue.shift()!;
      if (item === null) return;
      if (item instanceof Error) throw item;
      yield item;
    }
  }
  return { send: (event: string, data: any = {}) => push({ event, data } as StreamEvent), end: () => push(null), fail: (e: Error) => push(e), events };
}

function setup(handlers: Record<string, Handler>, stream = channel()) {
  const calls: Call[] = [];
  const sent: string[] = [];
  const api = {
    request: vi.fn(async (path: string, opts: any = {}) => {
      const call = { path, method: opts.method || "GET", body: opts.body };
      calls.push(call);
      const handler = handlers[`${call.method} ${path}`];
      if (!handler) throw new HttpError(`no handler for ${call.method} ${path}`, 500);
      return handler(call);
    }),
    send: vi.fn(async (path: string) => {
      sent.push(path);
    }),
  };
  const go = vi.fn();
  const toast = vi.fn();
  const chats = { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() };
  const store = new ChatStore({ api: api as never, readEvents: (() => stream.events()) as never, toast, go, chats });
  store.setCatalog({ models: [{ id: "m1", name: "Model One" } as never] });
  store.setRoute(routeOf("/"));
  stores.push(store);
  return { store, api, calls, sent, go, toast, chats, stream };
}
const stores: ChatStore[] = [];
afterEach(() => {
  stores.splice(0).forEach((s) => s.dispose());
  vi.useRealTimers();
});

const conversation = (over: Partial<Conversation> = {}): Conversation => ({ id: "c1", title: "Hello", model: "m1", messages: [], ...over }) as Conversation;
const flush = () => new Promise((r) => setTimeout(r, 0));
async function until(check: () => boolean, tries = 50) {
  for (let i = 0; i < tries && !check(); i++) await flush();
  expect(check()).toBe(true);
}
const done = (content: string): Message[] => [
  { id: "u1", role: "user", content: "Hello", status: "completed" },
  { id: "a1", role: "assistant", content, status: "completed" },
];

describe("when the session ends", () => {
  it("drops what was set up for a new chat and typed into Steer", async () => {
    const { startAutoDraft } = await import("@/components/connections/chat-tools");
    const { chatDraft } = await import("@/components/projects/chat-draft");
    const { steerDraft } = await import("./steer-draft");
    const { store } = setup({});
    startAutoDraft();
    chatDraft.setProject("p1");
    chatDraft.setCollections(["c1"]);
    steerDraft.text = "use the other cluster";
    store.dispose();
    // The next person on this page starts a chat with Auto off and nothing picked.
    expect(chatDraft.get()).toEqual({ project: null, collections: [] });
    expect(steerDraft.text).toBe("");
    const applied: unknown[] = [];
    const { applyToolsDraft } = await import("@/components/connections/chat-tools");
    const { api } = await import("@/lib/api");
    const request = vi.spyOn(api, "request").mockImplementation(async (...args: unknown[]) => {
      applied.push(args);
      return {} as never;
    });
    await applyToolsDraft("c9");
    request.mockRestore();
    expect(JSON.stringify(applied)).not.toContain('"auto":true');
  });
});

describe("sending from the home screen", () => {
  it("creates the chat, streams the reply and settles on the saved one", async () => {
    const { store, calls, go, chats, stream } = setup({
      "POST /api/conversations": () => ({ conversation: conversation() }),
      "POST /api/conversations/c1/messages": () => ({ body: null }),
      "GET /api/conversations/c1": () => ({ conversation: conversation({ messages: done("Hi there") }) }),
    });
    store.draft = "Hello";
    const sending = store.send("Hello");
    await until(() => calls.some((c) => c.path.endsWith("/messages")));
    expect(go).toHaveBeenCalledWith("/chat/c1", true);
    expect(store.route).toEqual({ page: "chat", id: "c1" });
    expect(store.draft).toBe("");
    expect(store.streaming).toBe(true);
    const post = calls.find((c) => c.path.endsWith("/messages"))!;
    expect(post.body).toMatchObject({ content: "Hello", model: "m1", attachmentIds: [] });

    stream.send("meta", { messageId: "a1", userMessageId: "u1", generationId: "g1", settings: { model: "m1" }, resolvedModel: "m1" });
    stream.send("reasoning", { content: "Thinking. " });
    stream.send("delta", { content: "Hi " });
    stream.send("tool", { id: "t1", kind: "tool", name: "search", status: "running" });
    stream.send("tool", { id: "t1", kind: "tool", name: "search", status: "completed" });
    stream.send("delta", { content: "there" });
    await until(() => store.active?.messages?.[1]?.content === "Hi there");
    const reply = store.active!.messages[1]!;
    expect(reply).toMatchObject({ id: "a1", status: "streaming", reasoningContent: "Thinking. " });
    expect(reply.activity).toHaveLength(1);
    expect(reply.activity![0]!.status).toBe("completed");
    expect(store.active!.messages[0]!.id).toBe("u1");

    stream.send("done", { status: "completed" });
    await sending;
    expect(store.streaming).toBe(false);
    expect(store.operations.size).toBe(0);
    expect(store.active!.messages[1]).toMatchObject({ content: "Hi there", status: "completed" });
    expect(chats.upsert).toHaveBeenCalled();
  });

  it("puts the draft back when the server refuses the message", async () => {
    const { store } = setup({
      "POST /api/conversations": () => {
        throw new HttpError("Daily request limit reached.", 429);
      },
    });
    await store.send("Hello");
    expect(store.draft).toBe("Hello");
    expect(store.error).toBe("Daily request limit reached.");
    expect(store.streaming).toBe(false);
  });

  it("shows a stream error on the reply", async () => {
    const { store, stream } = setup({
      "POST /api/conversations": () => ({ conversation: conversation() }),
      "POST /api/conversations/c1/messages": () => ({ body: null }),
      "GET /api/conversations/c1": () => ({
        conversation: conversation({ messages: [done("")[0]!, { id: "a1", role: "assistant", content: "", status: "error", errorMessage: "The model went away." }] }),
      }),
    });
    const sending = store.send("Hello");
    await flush();
    stream.send("meta", { messageId: "a1", generationId: "g1" });
    stream.send("error", { message: "The model went away." });
    await sending;
    expect(store.active!.messages[1]).toMatchObject({ status: "error", errorMessage: "The model went away." });
    expect(store.streaming).toBe(false);
  });
});

describe("an open chat", () => {
  async function opened(handlers: Record<string, Handler>, messages = done("First answer")) {
    let saved = conversation({ messages });
    const t = setup({ "GET /api/conversations/c1": () => ({ conversation: structuredClone(saved) }), ...handlers });
    t.store.setRoute(routeOf("/chat/c1"));
    await until(() => !t.store.loadingChat && !!t.store.active);
    return { ...t, save: (c: Conversation) => (saved = c) };
  }

  it("stops a reply with its generation id", async () => {
    const t = await opened({
      "POST /api/conversations/c1/messages": () => ({ body: null }),
      "POST /api/conversations/c1/stop": () => ({
        conversation: conversation({ messages: [...done("First answer"), { id: "u2", role: "user", content: "More", status: "completed" }, { id: "a2", role: "assistant", content: "Par", status: "cancelled" }] }),
      }),
    });
    const sending = t.store.send("More");
    await flush();
    t.stream.send("meta", { messageId: "a2", userMessageId: "u2", generationId: "g2" });
    t.stream.send("delta", { content: "Par" });
    await until(() => t.store.active?.messages?.[3]?.content === "Par");
    await t.store.stopCurrent();
    const stop = t.calls.find((c) => c.path.endsWith("/stop"))!;
    expect(stop.body).toEqual({ generationId: "g2" });
    expect(t.store.streaming).toBe(false);
    expect(t.store.active!.messages[3]!.status).toBe("cancelled");
    t.stream.end();
    await sending;
  });

  it("reconnects to the saved reply after the stream drops", async () => {
    const t = await opened({ "POST /api/conversations/c1/messages": () => ({ body: null }) });
    const sending = t.store.send("More");
    await flush();
    t.stream.send("meta", { messageId: "a2", userMessageId: "u2", generationId: "g2" });
    t.stream.send("delta", { content: "Half" });
    await until(() => t.store.active?.messages?.[3]?.content === "Half");
    // The server kept writing while the connection was down.
    t.save(conversation({ messages: [...done("First answer"), { id: "u2", role: "user", content: "More", status: "completed" }, { id: "a2", role: "assistant", content: "Half and the rest", status: "completed" }] }));
    t.stream.fail(new TypeError("network error"));
    await sending;
    expect(t.store.streaming).toBe(false);
    expect(t.store.active!.messages[3]).toMatchObject({ content: "Half and the rest", status: "completed" });
  });

  it("regenerates a reply in place", async () => {
    const t = await opened({ "POST /api/conversations/c1/messages": () => ({ body: null }) });
    const sending = t.store.forkAndSend("a1", "regenerate");
    await until(() => t.calls.some((c) => c.path.endsWith("/messages")));
    const post = t.calls.find((c) => c.path.endsWith("/messages"))!;
    expect(post.body).toMatchObject({ content: "", regenerateMessageId: "a1" });
    expect(t.store.active!.messages.map((m) => m.role)).toEqual(["user", "assistant"]);
    expect(t.store.active!.messages[1]!.status).toBe("pending");
    t.save(conversation({ messages: done("Second answer") }));
    t.stream.send("meta", { messageId: "a1b", generationId: "g3" });
    t.stream.send("delta", { content: "Second answer" });
    t.stream.send("done", { status: "completed" });
    await sending;
    expect(t.store.active!.messages[1]!.content).toBe("Second answer");
  });

  it("edits a message and resends it", async () => {
    const t = await opened({ "POST /api/conversations/c1/messages": () => ({ body: null }) });
    const sending = t.store.forkAndSend("u1", "edit", "Hello again");
    await until(() => t.calls.some((c) => c.path.endsWith("/messages")));
    const post = t.calls.find((c) => c.path.endsWith("/messages"))!;
    expect(post.body).toMatchObject({ content: "Hello again", editMessageId: "u1" });
    expect(t.store.active!.messages[0]!.content).toBe("Hello again");
    expect(t.store.active!.messages).toHaveLength(2);
    t.stream.end();
    t.save(conversation({ messages: [{ ...done("")[0]!, content: "Hello again" }, { id: "a9", role: "assistant", content: "Ok", status: "completed" }] }));
    await sending;
    expect(t.store.streaming).toBe(false);
  });

  it("keeps someone else's shared chat read-only", async () => {
    const t = setup({
      "GET /api/conversations/s1": () => {
        throw new HttpError("Not found", 404);
      },
      "GET /api/shared/s1": () => ({ conversation: conversation({ id: "s1", messages: done("Theirs") }), share: { scope: "link", ownerName: "Ana" } }),
    });
    t.store.setRoute(routeOf("/chat/s1"));
    await until(() => !!t.store.shared);
    expect(t.store.shared!.ownerName).toBe("Ana");
    expect(t.store.chatError).toBe("");
    expect(t.store.active!.messages[1]!.content).toBe("Theirs");
  });
});

describe("temporary chats", () => {
  it("are left when the person goes elsewhere", async () => {
    const t = setup({ "GET /api/conversations/t1": () => ({ conversation: conversation({ id: "t1", temporary: true }) }) });
    t.store.setRoute(routeOf("/chat/t1"));
    await until(() => t.store.active?.id === "t1");
    t.store.setRoute(routeOf("/history"));
    expect(t.sent).toEqual(["/api/conversations/t1/leave"]);
  });

  it("are left on sign-out or closing the tab", async () => {
    const t = setup({ "GET /api/conversations/t1": () => ({ conversation: conversation({ id: "t1", temporary: true }) }) });
    t.store.setRoute(routeOf("/chat/t1"));
    await until(() => t.store.active?.id === "t1");
    await t.store.leaveOpenTemporary();
    expect(t.sent).toEqual(["/api/conversations/t1/leave"]);
    await t.store.leaveOpenTemporary();
    expect(t.sent).toHaveLength(1);
  });

  it("are created temporary from the home toggle and not added to Recent", async () => {
    const t = setup({
      "POST /api/conversations": (call) => ({ conversation: conversation({ id: "t2", temporary: !!call.body.temporary }) }),
      "POST /api/conversations/t2/messages": () => ({ body: null }),
      "GET /api/conversations/t2": () => ({ conversation: conversation({ id: "t2", temporary: true, messages: done("Ok") }) }),
    });
    t.store.toggleTemporary();
    expect(t.store.temporaryDraftOn()).toBe(true);
    const sending = t.store.send("Private");
    await until(() => t.calls.some((c) => c.path.endsWith("/messages")));
    expect(t.calls[0]!.body.temporary).toBe(true);
    t.stream.send("done", { status: "completed" });
    await sending;
    expect(t.chats.upsert).not.toHaveBeenCalled();
  });
});

describe("a new chat", () => {
  it("clears the draft and attachments, but opening / does not", async () => {
    const t = setup({});
    t.store.draft = "Half a thought";
    t.store.setRoute(routeOf("/history"));
    t.store.setRoute(routeOf("/"));
    expect(t.store.draft).toBe("Half a thought");
    t.store.newChat();
    expect(t.store.draft).toBe("");
  });
});
