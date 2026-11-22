import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";
import type { Notebook, Source } from "./api";
import { noteBody, suggestions } from "./api";

const h = vi.hoisted(() => ({
  request: vi.fn(),
  replace: vi.fn(),
  push: vi.fn(),
  admin: false,
  store: {
    model: "m1",
    active: null as any,
    chatMissing: false,
    streaming: false,
    error: "",
    version: 0,
    send: vi.fn(async (_text: string) => {}),
    run: async (action: () => unknown) => {
      await action();
    },
    saveFromChat: vi.fn(),
  },
}));

vi.mock("@/lib/api", async (orig) => ({ ...(await orig<typeof import("@/lib/api")>()), api: { request: (...args: unknown[]) => h.request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: h.admin }), useUser: () => ({ id: "u1", role: "user" }), useSignedIn: () => true }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: h.push, replace: h.replace }), usePathname: () => "/projects/p1/notebook", useParams: () => ({ id: "p1" }), useSearchParams: () => new URLSearchParams() }));
vi.mock("@/components/chat/chat-provider", () => ({ useChat: () => h.store, useChatContent: () => h.store, useChatStore: () => h.store }));
vi.mock("@/components/chat/composer", () => ({ Composer: ({ placeholder }: { placeholder: string }) => <textarea aria-label="Message" placeholder={placeholder} /> }));
// A reply as the chat draws it: its text with each [n] linked, and its sources card.
vi.mock("@/components/chat/message", async () => {
  const { citeHtml, KnowledgeSources } = await import("@/components/knowledge/chat-knowledge");
  return {
    messageSignature: () => "",
    MessageView: ({ m, index }: { m: any; index: number }) => (
      <article data-message={index}>
        <div dangerouslySetInnerHTML={{ __html: citeHtml(`<p>${m.content}</p>`, m) }} />
        {m.role === "assistant" && <KnowledgeSources message={m} />}
      </article>
    ),
  };
});
vi.mock("@/components/projects/project-frame", () => ({
  useProjectFrame: () => ({ detail: { project: { id: "p1", mine: true }, saved: [{ id: "s1", kind: "note", title: "Papers past 256", createdAt: "2026-11-20T10:00:00Z" }] } }),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { ProjectNotebook } from "./notebook-page";

polyfillLayout();

const show = (chatId: string | null) =>
  renderWithQueries(
    <TooltipProvider>
      <ProjectNotebook projectId="p1" chatId={chatId} />
    </TooltipProvider>,
  );

const source = (id: string, more: Partial<Source> = {}): Source => ({
  id,
  name: `${id}.pdf`,
  mimeType: "application/pdf",
  size: 10,
  status: "ready",
  stage: "",
  done: 0,
  total: 0,
  pages: 14,
  passages: 30,
  error: null,
  createdAt: "2026-11-20T10:00:00Z",
  ticked: true,
  guide: null,
  ...more,
});

function notebook(more: Partial<Notebook> = {}): Notebook {
  return {
    chatId: "c1",
    onlySources: true,
    collections: [
      {
        id: "k1",
        name: "Papers",
        mine: true,
        total: 3,
        documents: [
          source("serving", { guide: { status: "ready", summary: "How a serving stack batches requests.", questions: ["Where does throughput flatten?", "Which models were tried?"] } }),
          source("cache", { ticked: false }),
          source("scan", { status: "needs_ocr", pages: 9 }),
        ],
      },
    ],
    ticked: 2,
    total: 3,
    model: { set: true, available: true, name: "Long", contextWindow: 112000 },
    meter: { chars: 244000, tokens: 61000, capacity: 100000, percent: 61, fits: true },
    addTo: ["k1"],
    canMakeCollection: false,
    ...more,
  };
}

const reply = {
  id: "a1",
  role: "assistant",
  status: "completed",
  content: "Throughput is flat past 320 [1].",
  activity: [{ kind: "sources", items: [{ n: 1, documentId: "serving", name: "serving.pdf", page: 10, excerpt: "throughput is flat beyond 320 concurrent sequences" }] }],
};

beforeEach(() => {
  h.request.mockReset();
  h.replace.mockReset();
  h.push.mockReset();
  h.store.send.mockClear();
  h.store.saveFromChat.mockClear();
  h.admin = false;
  h.store.active = { id: "c1", messages: [{ id: "u1", role: "user", content: "Where does it go flat?" }, reply] };
});

test("sources are listed by collection with their ticks, state and the meter", async () => {
  let current = notebook();
  h.request.mockImplementation(async (path: string, init?: { method?: string; body?: { documentIds: string[]; ticked: boolean } }) => {
    if (init?.method === "PUT" && path === "/api/projects/p1/notebook/sources") {
      const ids = new Set(init.body!.documentIds);
      current = { ...current, ticked: 3, collections: current.collections.map((g) => ({ ...g, documents: g.documents.map((d) => (ids.has(d.id) ? { ...d, ticked: init.body!.ticked } : d)) })) };
      return current;
    }
    if (path === "/api/projects/p1/notebook") return current;
    throw new Error(`unexpected ${path}`);
  });
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  expect(within(pane).getByText("2 of 3 ticked")).toBeInTheDocument();
  expect(within(pane).getByText("Ticked sources use 61% of what the notebook can read")).toBeInTheDocument();
  expect(within(pane).getAllByText("PDF · 14 pages")).toHaveLength(2);
  expect(within(pane).getByText("Needs text recognition")).toBeInTheDocument();
  expect(within(pane).getByRole("checkbox", { name: "Use serving.pdf" })).toBeChecked();
  await userEvent.click(within(pane).getByRole("checkbox", { name: "Use cache.pdf" }));
  await waitFor(() => expect(h.request).toHaveBeenCalledWith("/api/projects/p1/notebook/sources", { method: "PUT", body: { documentIds: ["cache"], ticked: true } }));
  expect(await within(pane).findByText("3 of 3 ticked")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Message" })).toHaveAttribute("placeholder", "Ask the 3 ticked sources");
});

test("a source opens to its guide, and its questions are asked in the chat", async () => {
  h.request.mockResolvedValue(notebook());
  show("c1");
  // The ticked sources' questions sit above the chat.
  const asks = await screen.findByRole("region", { name: "Suggested questions" });
  await userEvent.click(within(asks).getByRole("button", { name: "Which models were tried?" }));
  expect(h.store.send).toHaveBeenCalledWith("Which models were tried?");
  await userEvent.click(screen.getByRole("button", { name: /serving\.pdf/ }));
  expect(screen.getByText("How a serving stack batches requests.")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /Open the file/ })).toHaveAttribute("href", "/api/knowledge/documents/serving/file");
});

test("a clicked citation shows its quoted passage instead of opening the file", async () => {
  h.request.mockResolvedValue(notebook());
  show("c1");
  const cite = await screen.findByRole("link", { name: "1" });
  expect(cite).toHaveAttribute("href", "/api/knowledge/documents/serving/file#page=10");
  fireEvent.click(cite);
  const passage = await screen.findByRole("region", { name: "Source 1" });
  expect(within(passage).getByText("“throughput is flat beyond 320 concurrent sequences”")).toBeInTheDocument();
  expect(within(passage).getByRole("link", { name: /Open at this page/ })).toHaveAttribute("href", "/api/knowledge/documents/serving/file#page=10");
  await userEvent.click(within(passage).getByRole("button", { name: "Close the passage" }));
  expect(screen.queryByRole("region", { name: "Source 1" })).not.toBeInTheDocument();
  // An answer is saved as a note in the project, with the sources it cites.
  await userEvent.click(screen.getByRole("button", { name: "Save as a note" }));
  expect(h.store.saveFromChat).toHaveBeenCalledWith({ kind: "note", title: "Where does it go flat?", body: "Throughput is flat past 320 [1].\n\nSources\n[1] serving.pdf, page 10" }, 1, "p1");
});

test("the first visit starts the notebook's chat with the person's model", async () => {
  h.store.active = null;
  h.request.mockImplementation(async (path: string, init?: { method?: string }) => {
    if (path === "/api/projects/p1/notebook/chat" && init?.method === "POST") return { chatId: "c9", onlySources: true };
    if (path === "/api/projects/p1/notebook") return notebook({ chatId: null });
    throw new Error(`unexpected ${path}`);
  });
  show(null);
  await waitFor(() => expect(h.replace).toHaveBeenCalledWith("/projects/p1/notebook/c9"));
  expect(h.request).toHaveBeenCalledWith("/api/projects/p1/notebook/chat", { method: "POST", body: { model: "m1" } });
});

test("Only from the sources is saved for the chat", async () => {
  h.request.mockImplementation(async (path: string) => (path === "/api/conversations/c1/notebook" ? { onlySources: false } : notebook()));
  show("c1");
  const toggle = await screen.findByRole("switch", { name: "Only from the sources" });
  expect(toggle).toBeChecked();
  await userEvent.click(toggle);
  await waitFor(() => expect(h.request).toHaveBeenCalledWith("/api/conversations/c1/notebook", { method: "PUT", body: { onlySources: false } }));
});

test("without a Notebook model there is no meter, and only administrators see where to set one", async () => {
  h.request.mockResolvedValue(notebook({ model: { set: false, available: false }, meter: null }));
  const first = show("c1");
  await screen.findByRole("complementary", { name: "Sources" });
  expect(screen.queryByText(/of what the notebook can read/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Name a Notebook model/)).not.toBeInTheDocument();
  first.unmount();
  h.admin = true;
  show("c1");
  expect(await screen.findByText(/Name a Notebook model/)).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Administration › System" })).toHaveAttribute("href", "/settings/admin/system");
});

test("suggested questions take turns between sources, and notes list what they cite", () => {
  const n = notebook();
  n.collections[0]!.documents[1] = source("cache", { guide: { status: "ready", questions: ["Q2a", "Q2b"] } });
  expect(suggestions(n)).toEqual(["Where does throughput flatten?", "Q2a", "Which models were tried?", "Q2b"]);
  expect(noteBody("No citations here.", [])).toBe("No citations here.");
  expect(noteBody("A [2] and B [1, 2].", [{ n: 1, name: "a.pdf" }, { n: 2, name: "b.pdf", page: 3 }, { n: 3, name: "c.pdf" }])).toBe("A [2] and B [1, 2].\n\nSources\n[1] a.pdf\n[2] b.pdf, page 3");
});
