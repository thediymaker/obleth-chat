import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Definition } from "./model";

vi.mock("@/lib/api", () => ({ api: { request: vi.fn() } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: true }), useUser: () => ({ id: "u1", role: "admin" }) }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
import { api } from "@/lib/api";
import { AppTab } from "./app-tab";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  globalThis.DOMMatrixReadOnly ??= class {
    m22 = 1;
    constructor() {}
  } as unknown as typeof DOMMatrixReadOnly;
});

const definition: Definition = {
  inputs: [{ name: "question", type: "text", required: true }],
  steps: [
    {
      type: "decide",
      id: "route",
      title: "",
      model: "glm",
      question: "What is it about?",
      input: "{{inputs.question}}",
      paths: [{ name: "Samples", description: "a sample", steps: [{ type: "search", id: "search", title: "", query: "{{inputs.question}}", limit: 8 }] }],
      otherwise: [{ type: "response", id: "nothing", title: "", value: "Ask about <samples>" }],
    },
    { type: "agent", id: "agent", title: "Answer", model: "glm", instructions: "", prompt: "{{inputs.question}}", json: false },
  ],
};
const app = { id: "k7f3q2xm", projectId: "p1", name: "Assay Lab", slug: "assay-lab", description: "", definition, live: true, version: 2, changed: true, endpoint: "/api/apps/k7f3q2xm/run" };
const request = api.request as unknown as ReturnType<typeof vi.fn>;
let sent: [string, { definition: Definition; name: string }][];
let current: typeof app | null;

function renderTab() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AppTab projectId="p1" />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  sent = [];
  current = app;
  request.mockReset();
  request.mockImplementation(async (path: string, options: { method?: string; body?: { definition: Definition; name: string } } = {}) => {
    if (options.method === "PUT") {
      sent.push([path, options.body!]);
      return { app: { ...app, definition: options.body!.definition } };
    }
    if (path === "/api/projects/p1/app") return options.method === "POST" ? { app } : { app: current };
    if (path.endsWith("/runs")) return { runs: [{ id: "r1", trigger: "api", version: 2, status: "failed", startedAt: "2026-11-02T10:00:00Z", ms: 40 }] };
    if (path.endsWith("/keys")) return { keys: [{ id: "k1", name: "Lab <pipeline>", createdBy: "Ana", lastUsedAt: null }] };
    if (path === "/api/models") return { models: [{ id: "auto" }, { id: "glm" }] };
    return {};
  });
});

describe("the App tab", () => {
  it("offers to make an app when the project has none", async () => {
    current = null;
    renderTab();
    expect(await screen.findByText("Turn this project into an app")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Create app/ }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/projects/p1/app", { method: "POST" }));
    expect(await screen.findByText("k7f3q2xm")).toBeInTheDocument();
  });

  it("shows its status, steps, paths, API and runs, with names as text", async () => {
    renderTab();
    expect(await screen.findByText("k7f3q2xm")).toBeInTheDocument();
    expect(screen.getByText("v2 live · draft changed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Publish changes" })).toBeEnabled();
    expect(await screen.findByText("When: Samples")).toBeInTheDocument();
    expect(screen.getByText("Otherwise")).toBeInTheDocument();
    expect(screen.getByText("Ask about <samples>")).toBeInTheDocument();
    expect(screen.getByText(/curl http:\/\/localhost(:\d+)?\/api\/apps\/k7f3q2xm\/run/)).toHaveTextContent('-d \'{"question":"…"}\'');
    expect(await screen.findByText("Lab <pipeline>")).toBeInTheDocument();
    expect(screen.getByText("API · v2")).toBeInTheDocument();
  });

  it("a step from the palette goes after the selected one, saved at once, then edited in the panel", async () => {
    renderTab();
    await screen.findByText("When: Samples");
    // Select the Decide step (a click, as the canvas takes it), then add Code from the palette: it lands right after it.
    fireEvent.click(screen.getByText("glm picks a path for {{inputs.question}}"));
    expect(await screen.findByLabelText("Paths, one per line")).toHaveValue("Samples: a sample");
    await userEvent.click(screen.getByRole("button", { name: "Add Code" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]![0]).toBe("/api/apps/k7f3q2xm");
    expect(sent[0]![1].name).toBe("Assay Lab");
    expect(sent[0]![1].definition.steps.map((s) => s.id)).toEqual(["route", "code", "agent"]);
    // The new step is selected: edit and save it.
    const editor = (await screen.findByLabelText("JavaScript")).closest("form")!;
    const id = within(editor).getByLabelText("Id");
    await userEvent.clear(id);
    await userEvent.type(id, "shape");
    await userEvent.type(within(editor).getByLabelText("Title"), "Shape rows");
    const code = within(editor).getByLabelText("JavaScript");
    await userEvent.clear(code);
    await userEvent.type(code, "return steps.search.length;");
    await userEvent.click(within(editor).getByRole("button", { name: "Save step" }));
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]![1].definition.steps[1]).toEqual({ type: "code", id: "shape", title: "Shape rows", code: "return steps.search.length;" });
  });

  it("the palette adds again straight after an add (nothing waits on a form)", async () => {
    renderTab();
    await screen.findByText("When: Samples");
    await userEvent.click(screen.getByRole("button", { name: "Add HTTP request" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    await userEvent.click(screen.getByRole("button", { name: "Add Response" }));
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]![1].definition.steps.map((s) => s.type)).toEqual(["decide", "agent", "http", "response"]);
  });

  it("a step the server won't take yet waits as a draft with the reason", async () => {
    request.mockImplementation(async (path: string, options: { method?: string; body?: { definition: Definition; name: string } } = {}) => {
      if (options.method === "PUT") throw new Error("Step 3: choose a model and write the prompt");
      if (path === "/api/projects/p1/app") return { app };
      if (path.endsWith("/runs")) return { runs: [] };
      if (path.endsWith("/keys")) return { keys: [] };
      if (path === "/api/models") return { models: [] };
      return {};
    });
    renderTab();
    await screen.findByText("When: Samples");
    await userEvent.click(screen.getByRole("button", { name: "Add Agent" }));
    expect(await screen.findByRole("heading", { name: "Add Agent" })).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("choose a model");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("heading", { name: "Add Agent" })).not.toBeInTheDocument();
  });

  it("Delete has an Undo that puts the steps back", async () => {
    const { toast } = await import("sonner");
    const shown = toast as unknown as ReturnType<typeof vi.fn>;
    shown.mockClear();
    renderTab();
    await screen.findByText("When: Samples");
    fireEvent.click(screen.getByText("Answer"));
    await userEvent.click(await screen.findByRole("button", { name: "Delete step" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]![1].definition.steps.map((s) => s.id)).toEqual(["route"]);
    const [message, options] = shown.mock.calls.at(-1)!;
    expect(message).toBe("Deleted Answer");
    await options.action.onClick();
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]![1].definition).toEqual(definition);
  });
});
