import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";
import type { Notebook, NotebookItem, Source } from "./api";
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
    openDialog: vi.fn(),
  },
}));

vi.mock("@/lib/api", async (orig) => ({ ...(await orig<typeof import("@/lib/api")>()), api: { request: (...args: unknown[]) => h.request(...args) } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: h.admin }), useUser: () => ({ id: "u1", role: "user" }), useSignedIn: () => true }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: h.push, replace: h.replace }), usePathname: () => "/projects/p1/notebook", useParams: () => ({ id: "p1" }), useSearchParams: () => new URLSearchParams() }));
vi.mock("@/components/chat/chat-provider", () => ({ useChat: () => h.store, useChatContent: () => h.store, useChatStore: () => h.store }));
vi.mock("@/components/chat/markdown-view", () => ({ Markdown: ({ text, cite }: { text: string; cite?: (html: string) => string }) => <div dangerouslySetInnerHTML={{ __html: cite ? cite(`<p>${text}</p>`) : text }} /> }));
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
    items: [],
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
  expect(await screen.findByText(/are not set up on this server/)).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Set it up" })).not.toBeInTheDocument();
  first.unmount();
  h.admin = true;
  show("c1");
  expect(await screen.findByText(/are not set up on this server/)).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Set it up" })).toHaveAttribute("href", "/settings/admin/system");
});

test("suggested questions take turns between sources, and notes list what they cite", () => {
  const n = notebook();
  n.collections[0]!.documents[1] = source("cache", { guide: { status: "ready", questions: ["Q2a", "Q2b"] } });
  expect(suggestions(n)).toEqual(["Where does throughput flatten?", "Q2a", "Which models were tried?", "Q2b"]);
  expect(noteBody("No citations here.", [])).toBe("No citations here.");
  expect(noteBody("A [2] and B [1, 2].", [{ n: 1, name: "a.pdf" }, { n: 2, name: "b.pdf", page: 3 }, { n: 3, name: "c.pdf" }])).toBe("A [2] and B [1, 2].\n\nSources\n[1] a.pdf\n[2] b.pdf, page 3");
});

// ---- Make from the sources ----

const item = (id: string, more: Partial<NotebookItem> = {}): NotebookItem => ({
  id,
  kind: "briefing",
  title: `Briefing ${id}`,
  status: "ready",
  sources: [{ id: "serving", name: "serving.pdf" }],
  cited: 1,
  unverified: 0,
  error: null,
  createdAt: "2026-11-20T10:00:00Z",
  finishedAt: "2026-11-20T10:02:00Z",
  ...more,
});

test("without a Notebook model there are no tiles to make anything", async () => {
  h.request.mockResolvedValue(notebook({ model: { set: false, available: false }, meter: null }));
  show("c1");
  await screen.findByRole("complementary", { name: "Sources" });
  expect(screen.queryByRole("button", { name: /Briefing/ })).not.toBeInTheDocument();
  expect(screen.queryByText("Make from the sources")).not.toBeInTheDocument();
});

test("a tile starts making it from the ticked sources", async () => {
  h.request.mockImplementation(async (path: string) => (path === "/api/projects/p1/notebook/items" ? { item: item("i9", { status: "running" }) } : notebook()));
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Make from the sources" });
  await userEvent.click(within(pane).getByRole("button", { name: /Study guide/ }));
  expect(h.request).toHaveBeenCalledWith("/api/projects/p1/notebook/items", { method: "POST", body: { kind: "study_guide" } });
});

test("sources too long for the model are refused before anything is asked", async () => {
  h.request.mockResolvedValue(notebook({ meter: { chars: 500000, tokens: 125000, capacity: 100000, percent: 125, fits: false } }));
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Make from the sources" });
  expect(within(pane).getByRole("button", { name: /Briefing/ })).toBeDisabled();
  expect(within(pane).getByText(/too long for the Notebook model to read at once: they come to about 125%/)).toBeInTheDocument();
});

test("a running item is alive, and a ready one opens with its checked citations", async () => {
  const full = { ...item("i2"), body: "# Briefing i2\n\nFlat past 320 [1].", citations: [{ n: 1, documentId: "serving", name: "serving.pdf", page: 10, section: null, quote: "throughput is flat beyond 320" }] };
  h.request.mockImplementation(async (path: string) => {
    if (path === "/api/notebook/items/i2") return { item: full };
    return notebook({ items: [item("i1", { status: "running", title: "Timeline of 1 source" }), item("i2"), item("i3", { status: "failed", error: "The model wrote nothing" })] });
  });
  const { container } = show("c1");
  const kept = await screen.findByRole("region", { name: "In this notebook" });
  expect(within(kept).getByRole("status")).toHaveTextContent("Reading 1 source");
  expect(container.querySelectorAll(".signal-pulse")).toHaveLength(1);
  expect(within(kept).getByText("The model wrote nothing")).toBeInTheDocument();
  await userEvent.click(within(kept).getByRole("button", { name: /^Briefing i2/ }));
  const dialog = await screen.findByRole("dialog");
  expect(await within(dialog).findByText(/1 quotation checked against the sources/)).toBeInTheDocument();
  const cite = within(dialog).getByRole("link", { name: "1" });
  expect(cite).toHaveAttribute("href", "/api/knowledge/documents/serving/file#page=10");
  fireEvent.click(cite);
  expect(within(dialog).getByRole("region", { name: "Source 1" })).toHaveTextContent("“throughput is flat beyond 320”");
  await userEvent.click(within(dialog).getByRole("button", { name: "Save as a note" }));
  expect(h.store.openDialog).toHaveBeenCalledWith({ kind: "save", item: { kind: "note", title: "Briefing i2", body: "# Briefing i2\n\nFlat past 320 [1].\n\nSources\n[1] serving.pdf, page 10", projectId: "p1" } });
  await userEvent.keyboard("{Escape}");
  await userEvent.click(within(kept).getByRole("button", { name: "Delete Briefing i3" }));
  expect(h.request).toHaveBeenCalledWith("/api/notebook/items/i3", { method: "DELETE" });
  expect(within(kept).queryByRole("button", { name: /^Delete Timeline/ })).not.toBeInTheDocument();
});

// ---- Web pages ----

const webPage = (id: string, more: Partial<Source> = {}): Source =>
  source(id, { name: `${id} notes.md`, mimeType: "text/plain; charset=utf-8", pages: null, passages: 12, sourceUrl: `https://www.example.org/${id}?ref=feed`, readAt: new Date(Date.now() - 5 * 60_000).toISOString(), ...more });

test("Add offers Web page only when the browser is on, and reads the addresses given", async () => {
  let current = notebook({ webPages: true });
  h.request.mockImplementation(async (path: string, init?: { method?: string; body?: { addresses: string[] } }) => {
    if (path === "/api/projects/p1/notebook/pages" && init?.method === "POST") {
      current = notebook({ webPages: true, collections: [{ ...current.collections[0]!, reading: init.body!.addresses.map((url, i) => ({ id: `r${i}`, url, status: "reading" as const, error: null, requestedAt: "2026-11-22T10:00:00Z" })) }] });
      return current;
    }
    if (path === "/api/projects/p1/notebook") return current;
    throw new Error(`unexpected ${path}`);
  });
  const { container } = show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  await userEvent.click(within(pane).getByRole("button", { name: "Add" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Web page" }));
  const dialog = await screen.findByRole("dialog", { name: "Add web pages as sources" });
  const box = within(dialog).getByRole("textbox", { name: "Addresses" });
  fireEvent.change(box, { target: { value: Array.from({ length: 11 }, (_, i) => `example.org/${i}`).join("\n") } });
  expect(within(dialog).getByText("Up to 10 pages at a time; this has 11.")).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Add 11 pages" })).toBeDisabled();
  fireEvent.change(box, { target: { value: "https://example.org/post\n\nexample.org/paper.pdf" } });
  await userEvent.click(within(dialog).getByRole("button", { name: "Add 2 pages" }));
  expect(h.request).toHaveBeenCalledWith("/api/projects/p1/notebook/pages", { method: "POST", body: { addresses: ["https://example.org/post", "example.org/paper.pdf"] } });
  // Each page shows on its own row while it is read, the only thing in the accent colour.
  const rows = await within(pane).findAllByRole("status");
  expect(rows.map((r) => r.textContent)).toEqual(["Reading the page…", "Reading the page…"]);
  expect(within(pane).getByText("example.org/post")).toBeInTheDocument();
  expect(container.querySelectorAll("[data-read] .signal-pulse")).toHaveLength(2);
});

test("without the browser there is no Web page to add", async () => {
  h.request.mockResolvedValue(notebook({ webPages: false }));
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  await userEvent.click(within(pane).getByRole("button", { name: "Add" }));
  expect(await screen.findByRole("menuitem", { name: "Paste text" })).toBeInTheDocument();
  expect(screen.queryByRole("menuitem", { name: "Web page" })).not.toBeInTheDocument();
});

test("a page that failed says why, and is tried again or removed", async () => {
  const failed = { id: "r1", url: "https://example.org/members/report", status: "failed" as const, error: "The page asked for a sign-in. Sign in to the site in the browser beside a chat, then add the page from there.", requestedAt: "2026-11-22T10:00:00Z" };
  h.request.mockImplementation(async (path: string) => {
    if (path === "/api/notebook/reads/r1/retry") return notebook({ webPages: true, collections: [{ ...notebook().collections[0]!, reading: [{ ...failed, status: "reading", error: null }] }] });
    if (path === "/api/notebook/reads/r1") return null;
    return notebook({ webPages: true, collections: [{ ...notebook().collections[0]!, reading: [failed] }] });
  });
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  expect(await within(pane).findByText(/asked for a sign-in/)).toBeInTheDocument();
  await userEvent.click(within(pane).getByRole("button", { name: "Try again" }));
  expect(h.request).toHaveBeenCalledWith("/api/notebook/reads/r1/retry", { method: "POST" });
  expect(await within(pane).findByRole("status")).toHaveTextContent("Reading the page…");
});

test("a failed page can be removed", async () => {
  const failed = { id: "r2", url: "https://example.org/gone", status: "failed" as const, error: "The page was not found (HTTP 404).", requestedAt: "2026-11-22T10:00:00Z" };
  h.request.mockImplementation(async (path: string) => (path === "/api/notebook/reads/r2" ? null : notebook({ webPages: true, collections: [{ ...notebook().collections[0]!, reading: [failed] }] })));
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  await userEvent.click(await within(pane).findByRole("button", { name: "Remove" }));
  expect(h.request).toHaveBeenCalledWith("/api/notebook/reads/r2", { method: "DELETE" });
});

test("a web source shows its site and when it was read, opens its address, and is read again", async () => {
  let current = notebook({ webPages: true, collections: [{ ...notebook().collections[0]!, documents: [webPage("warmup")] }] });
  h.request.mockImplementation(async (path: string, init?: { method?: string }) => {
    if (path === "/api/projects/p1/notebook/sources/warmup/read" && init?.method === "POST") {
      current = notebook({ webPages: true, collections: [{ ...current.collections[0]!, documents: [webPage("warmup", { reading: { id: "r9", status: "reading", error: null } })] }] });
      return current;
    }
    return current;
  });
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  expect(await within(pane).findByText("example.org · Web page · read 5 min ago")).toBeInTheDocument();
  expect(within(pane).getByRole("checkbox", { name: "Use warmup notes" })).toBeChecked();
  await userEvent.click(within(pane).getByRole("button", { name: /warmup notes/ }));
  const open = within(pane).getByRole("link", { name: /Open the page/ });
  expect(open).toHaveAttribute("href", "https://www.example.org/warmup?ref=feed");
  expect(open).toHaveAttribute("target", "_blank");
  expect(open).toHaveAttribute("rel", "noopener noreferrer");
  await userEvent.click(within(pane).getByRole("button", { name: "Read it again" }));
  expect(h.request).toHaveBeenCalledWith("/api/projects/p1/notebook/sources/warmup/read", { method: "POST" });
  expect(await within(pane).findByRole("status")).toHaveTextContent("Reading the page again…");
  expect(within(pane).queryByRole("button", { name: "Read it again" })).not.toBeInTheDocument();
});

test("a failed reading again keeps the source and says why", async () => {
  h.request.mockResolvedValue(notebook({ webPages: true, collections: [{ ...notebook().collections[0]!, documents: [webPage("warmup", { reading: { id: "r3", status: "failed", error: "The site answered with an error (HTTP 503)." } })] }] }));
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  expect(await within(pane).findByText("Could not read it again: The site answered with an error (HTTP 503).")).toBeInTheDocument();
  await userEvent.click(within(pane).getByRole("button", { name: /warmup notes/ }));
  await userEvent.click(within(pane).getByRole("button", { name: "Dismiss" }));
  expect(h.request).toHaveBeenCalledWith("/api/notebook/reads/r3", { method: "DELETE" });
});

test("someone else's web source cannot be read again by this person", async () => {
  h.request.mockResolvedValue(notebook({ webPages: true, collections: [{ ...notebook().collections[0]!, mine: false, documents: [webPage("shared")] }] }));
  show("c1");
  const pane = await screen.findByRole("complementary", { name: "Sources" });
  await userEvent.click(await within(pane).findByRole("button", { name: /shared notes/ }));
  expect(within(pane).getByRole("link", { name: /Open the page/ })).toBeInTheDocument();
  expect(within(pane).queryByRole("button", { name: "Read it again" })).not.toBeInTheDocument();
});
