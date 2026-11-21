// A reply's versions side by side.

import { fireEvent, screen } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
import { ChatStore } from "@/lib/chat/store";
import { ChatStoreProvider } from "./chat-provider";
import { CompareDialog } from "./compare-dialog";
import { polyfillLayout, renderWithQueries } from "./test-queries";

polyfillLayout();

const versions = [
  { id: "v1", status: "completed", createdAt: new Date().toISOString(), model: "small", content: "Use a **queue**.", usage: { completion_tokens: 400, generation_ms: 4000, first_token_ms: 900 }, selected: false },
  { id: "v2", status: "completed", createdAt: new Date().toISOString(), model: "large", content: "Use a *lock*.", usage: { completion_tokens: 900, generation_ms: 3000, first_token_ms: 300 }, selected: true },
];

function show(list: object[] = versions, props: { waiting?: boolean; suggest?: string } = {}) {
  apiRequest.mockReset();
  apiRequest.mockImplementation(async (path: string) => {
    if (path === "/api/conversations/c1/messages/v2/versions") return { versions: list };
    throw new Error(`unexpected ${path}`);
  });
  const store = new ChatStore({ api: { request: vi.fn().mockRejectedValue(new Error("offline")), send: vi.fn() } as never, toast: vi.fn(), go: vi.fn(), chats: { upsert: vi.fn(), remove: vi.fn(), refresh: vi.fn(), find: vi.fn() } });
  store.models = [
    { id: "small", name: "Small" },
    { id: "large", name: "Large" },
  ] as never;
  const onOpenChange = vi.fn();
  renderWithQueries(
    <ChatStoreProvider store={store}>
      <CompareDialog chatId="c1" messageId="v2" open onOpenChange={onOpenChange} {...props} />
    </ChatStoreProvider>,
  );
  return { store, onOpenChange };
}

test("shows two versions with their model, what they cost and their text", async () => {
  show();
  const left = await screen.findByRole("region", { name: "Version 1" });
  const right = screen.getByRole("region", { name: "Version 2" });
  expect(left.textContent).toContain("1 · Small");
  expect(left.textContent).toContain("speed100 tokens/s");
  expect(left.querySelector("strong")?.textContent).toBe("queue");
  expect(right.textContent).toContain("2 · Large");
  expect(right.textContent).toContain("first token0.30 s");
  expect(right.textContent).toContain("Shown in the chat");
});

test("keeping the other version selects it and closes", async () => {
  const { store, onOpenChange } = show();
  const select = vi.spyOn(store, "selectMessageVersion").mockResolvedValue(undefined);
  fireEvent.click(await screen.findByRole("button", { name: "Keep this answer, from Small" }));
  expect(select).toHaveBeenCalledWith("v1");
  expect(onOpenChange).toHaveBeenCalledWith(false);
});

test("a reply with one version says how to get a second", async () => {
  show([versions[1]!]);
  expect(await screen.findByText("This reply has one version. Ask another model below to get a second.")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Ask" }) as HTMLButtonElement).disabled).toBe(true);
});

test("three answers are read side by side, with Auto's pick named and the fastest marked", async () => {
  const third = { id: "v3", status: "completed", createdAt: new Date().toISOString(), model: "small", content: "Use *both*.", usage: { completion_tokens: 500, generation_ms: 1000, first_token_ms: 200 }, selected: false, second: true };
  show([{ ...versions[0]!, selected: false, second: true }, { ...versions[1]!, auto: true }, third]);
  const columns = await screen.findAllByRole("region", { name: /^Version / });
  expect(columns.map((c) => c.getAttribute("data-compare-side"))).toEqual(["1", "2", "3"]);
  expect(document.querySelector("[data-compare-columns]")?.getAttribute("data-compare-columns")).toBe("3");
  expect(columns[1]!.textContent).toContain("Auto’s pick");
  expect(columns[1]!.textContent).toContain("Shown in the chat");
  // The third was first to its first token and wrote fastest: both are marked there and nowhere else.
  const marked = (column: HTMLElement) => [...column.querySelectorAll("dd.font-medium")].map((d) => d.textContent);
  expect(marked(columns[2]!)).toEqual(["500 tokens/s", "0.20 s", "1.0 s"]);
  expect(marked(columns[0]!)).toEqual([]);
  expect(screen.getByText(/answered by Auto’s pick and by the models it ranked next/)).toBeTruthy();
  expect(screen.getAllByRole("button", { name: /^Keep this answer/ })).toHaveLength(2);
});

test("while the other models are still answering it says so, and offers Auto's next choice to ask", async () => {
  show([versions[1]!], { waiting: true, suggest: "small" });
  expect(await screen.findByText("The other models are still answering…")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Ask" }) as HTMLButtonElement).disabled).toBe(false);
});
