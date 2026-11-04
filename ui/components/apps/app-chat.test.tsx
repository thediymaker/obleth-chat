import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const replace = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ replace }) }));
vi.mock("@/lib/api", () => ({
  api: { request: vi.fn() },
  ApiError: class extends Error {
    status = 0;
  },
}));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready" }), useUser: () => ({ id: "u1", username: "ada", display_name: "Ada Lovelace", role: "admin" }) }));
vi.mock("@/components/brand", () => ({ Brand: () => null }));
vi.mock("@/components/theme-toggle", () => ({ ThemeToggle: () => null }));
vi.mock("@/components/chat/markdown-view", () => ({ Markdown: ({ text }: { text: string }) => <div data-testid="reply">{text}</div> }));
import { api } from "@/lib/api";
import { AppChatPage, replyText } from "./app-chat";
import { chatPageHints } from "./app-tab";

const request = api.request as unknown as ReturnType<typeof vi.fn>;
const page = { id: "k7f3q2xm", name: "Assay Lab", slug: "assay-lab", description: "Answers about samples", version: 3, publishedAt: null, message: "question", history: true, missing: [] as string[], projectId: null as string | null };
let shown: typeof page;
let sent: { message: string; history: unknown[] }[];
let replies: unknown[];

function renderPage(slug?: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AppChatPage id="k7f3q2xm" slug={slug} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  sessionStorage.clear();
  replace.mockReset();
  shown = { ...page };
  sent = [];
  replies = [];
  request.mockReset();
  request.mockImplementation(async (path: string, options: { body?: { message: string; history: unknown[] } } = {}) => {
    if (path.endsWith("/page")) return { app: shown };
    if (path.endsWith("/chat")) {
      sent.push(options.body!);
      const next = replies.shift();
      if (next instanceof Error) throw next;
      return next;
    }
    throw new Error(`unexpected ${path}`);
  });
});

describe("replyText", () => {
  it("shows text, an answer field, or else the JSON", () => {
    expect(replyText("Hello")).toBe("Hello");
    expect(replyText({ answer: "42", count: 2 })).toBe("42");
    expect(replyText({ summary: "Short" })).toBe("Short");
    expect(replyText({ a: 1, b: 2 })).toContain('"a": 1');
    expect(replyText(null)).toBe("");
  });
});

describe("AppChatPage", () => {
  it("corrects the name in the address", async () => {
    renderPage("old-name");
    await screen.findByRole("heading", { name: "Assay Lab" });
    expect(replace).toHaveBeenCalledWith("/a/k7f3q2xm/assay-lab");
  });

  it("sends each message with the earlier turns and shows the reply", async () => {
    replies = [{ runId: "r1", status: "completed", output: { answer: "Four samples" }, error: null }, { runId: "r2", status: "completed", output: "Two passed", error: null }];
    renderPage("assay-lab");
    const box = await screen.findByLabelText("Message Assay Lab");
    await userEvent.type(box, "How many?{Enter}");
    expect(await screen.findByText("Four samples")).toBeTruthy();
    await userEvent.type(box, "Which passed?{Enter}");
    expect(await screen.findByText("Two passed")).toBeTruthy();
    expect(sent).toEqual([
      { message: "How many?", history: [] },
      { message: "Which passed?", history: [{ role: "user", content: "How many?" }, { role: "assistant", content: "Four samples" }] },
    ]);
    expect(replace).not.toHaveBeenCalled();
    // A reload in this tab keeps the conversation.
    expect(JSON.parse(sessionStorage.getItem("obleth-chat:app-chat:k7f3q2xm")!)).toHaveLength(4);
  });

  it("shows a failed run with Try again, and leaves it out of the history", async () => {
    replies = [{ runId: "r1", status: "failed", output: null, error: "The model timed out" }, { runId: "r2", status: "completed", output: "Done", error: null }];
    renderPage("assay-lab");
    await userEvent.type(await screen.findByLabelText("Message Assay Lab"), "Run it{Enter}");
    expect(await screen.findByText("The model timed out")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Try again/ }));
    expect(await screen.findByText("Done")).toBeTruthy();
    expect(sent[1]).toEqual({ message: "Run it", history: [] });
    expect(screen.queryByText("The model timed out")).toBeNull();
  });

  it("sends no history to an app without a history input", async () => {
    shown = { ...page, history: false };
    replies = [{ runId: "r1", status: "completed", output: "A", error: null }, { runId: "r2", status: "completed", output: "B", error: null }];
    renderPage("assay-lab");
    const box = await screen.findByLabelText("Message Assay Lab");
    await userEvent.type(box, "one{Enter}");
    await screen.findByText("A");
    await userEvent.type(box, "two{Enter}");
    await screen.findByText("B");
    expect(sent[1].history).toEqual([]);
    expect(screen.getByText(/sees each message on its own/)).toBeTruthy();
  });

  it("explains an app a chat can't fill, and offers the owner a way back", async () => {
    shown = { ...page, missing: ["limit"], projectId: "p1" };
    renderPage("assay-lab");
    expect(await screen.findByText(/also needs limit/)).toBeTruthy();
    expect(screen.queryByLabelText("Message Assay Lab")).toBeNull();
    expect(screen.getByRole("link", { name: /Edit app/ }).getAttribute("href")).toBe("/projects/p1/app");
  });

  it("says when the app isn't published", async () => {
    request.mockImplementation(async () => {
      throw Object.assign(new Error("This app isn't published"), { status: 404 });
    });
    renderPage("assay-lab");
    await waitFor(() => expect(screen.getByText("This app isn’t available")).toBeTruthy());
  });
});

describe("chatPageHints", () => {
  it("matches what the server lets a chat fill", () => {
    expect(chatPageHints([{ name: "limit", type: "number", required: true }]).blocked).toMatch(/Add a text input/);
    expect(chatPageHints([{ name: "question", required: true }, { name: "limit", type: "number", required: true }]).blocked).toMatch(/Make limit optional/);
    expect(chatPageHints([{ name: "question", required: true }]).tip).toMatch(/Add a JSON input named history/);
    expect(chatPageHints([{ name: "question", required: true }, { name: "history", type: "json" }]).tip).toMatch(/earlier turns into history/);
  });
});
