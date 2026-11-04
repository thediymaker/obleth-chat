import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api", () => ({ api: { request: vi.fn() } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: true }), useUser: () => ({ id: "u1", role: "admin" }) }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }), usePathname: () => "/", useParams: () => ({}) }));
import { api } from "@/lib/api";
import { toast } from "sonner";
import { moveChatToResearchProject } from "./api";
import { applyChatDraft, chatDraft } from "./chat-draft";
import { ProjectContextSection } from "./chat-project";
import { ProjectDialog } from "./project-dialog";

const request = api.request as unknown as ReturnType<typeof vi.fn>;
let calls: [string, string, unknown][];
const project = (id: string, name: string) => ({ id, name, color: "green", mine: true, scope: "personal", description: "", instructions: "", groupId: null, groupName: null, ownerName: "Ana", chats: 0, saved: 0, updatedAt: "" });

beforeEach(() => {
  calls = [];
  push.mockReset();
  (toast as unknown as ReturnType<typeof vi.fn>).mockReset();
  chatDraft.reset();
  request.mockReset();
  request.mockImplementation(async (path: string, options: { method?: string; body?: { name?: string; projectId?: string } } = {}) => {
    calls.push([options.method || "GET", path, options.body]);
    if (path === "/api/projects" && options.method === "POST") return { project: project("p9", options.body!.name!) };
    if (path === "/api/projects") return { projects: [project("p1", "Assay Lab")], groups: [] };
    if (path.endsWith("/project") && options.method === "PUT") return { project: project(options.body!.projectId!, "Moved") };
    if (path.endsWith("/project")) return { project: null };
    if (path === "/api/capabilities") return { features: {} };
    if (path === "/api/projects/p9/app") return { app: { id: "a1" } };
    return {};
  });
});

const wrap = (ui: React.ReactNode) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

describe("a new chat's draft", () => {
  it("puts the chat in the drafted project and collections once it exists, then clears", async () => {
    chatDraft.setProject("p1");
    chatDraft.setCollections(["c1", "c2"]);
    expect(await applyChatDraft("chat 1")).toEqual([]);
    expect(calls).toEqual(
      expect.arrayContaining([
        ["PUT", "/api/conversations/chat%201/project", { projectId: "p1" }],
        ["PUT", "/api/conversations/chat%201/knowledge", { collectionIds: ["c1", "c2"] }],
      ]),
    );
    expect(chatDraft.get()).toEqual({ project: null, collections: [] });
    calls.length = 0;
    await applyChatDraft("chat 2");
    expect(calls).toEqual([]);
  });
});

describe("moving a chat into a new project", () => {
  it("names a research project after its goal", async () => {
    const client = new QueryClient();
    await moveChatToResearchProject(client, "c1", "  Measure heat flow in thin films.  ");
    expect(calls[0]).toEqual(["POST", "/api/projects", { name: "Measure heat flow in thin films", description: "Research started from a chat.", color: "violet", scope: "personal" }]);
    expect(calls[1]).toEqual(["PUT", "/api/conversations/c1/project", { projectId: "p9" }]);
    calls.length = 0;
    await moveChatToResearchProject(client, "c1", " . ");
    expect((calls[0]![2] as { name: string }).name).toBe("Research");
  });
});

describe("the project form", () => {
  it("a new app makes the project, then its app, then opens the App tab", async () => {
    wrap(<ProjectDialog open onOpenChange={() => {}} asApp />);
    expect(screen.getByText(/An app is a project whose steps answer over an API/)).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Name"), "Assay Lab");
    await userEvent.click(screen.getByRole("button", { name: "Create app" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/projects/p9/app"));
    expect(calls.filter(([m]) => m === "POST").map(([, p]) => p)).toEqual(["/api/projects", "/api/projects/p9/app"]);
    expect((calls.find(([m]) => m === "POST")![2] as { color: string; scope: string }).color).toBe("green");
    expect(toast).toHaveBeenCalledWith("App created");
  });

  it("started from a chat, the chat moves into the new project and stays open", async () => {
    wrap(<ProjectDialog open onOpenChange={() => {}} fromChat="c7" />);
    await userEvent.type(screen.getByLabelText("Name"), "Thesis");
    await userEvent.click(screen.getByRole("button", { name: "Create project" }));
    await waitFor(() => expect(calls).toContainEqual(["PUT", "/api/conversations/c7/project", { projectId: "p9" }]));
    expect(push).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith("Moved to Moved");
  });

  it("shows the server's error inline", async () => {
    request.mockImplementation(async (path: string, options: { method?: string } = {}) => {
      if (path === "/api/projects" && options.method === "POST") throw new Error("You have 200 projects, the most there can be");
      return { projects: [], groups: [] };
    });
    wrap(<ProjectDialog open onOpenChange={() => {}} />);
    await userEvent.type(screen.getByLabelText("Name"), "One more");
    await userEvent.click(screen.getByRole("button", { name: "Create project" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("You have 200 projects, the most there can be");
  });
});

describe("the chat's Context panel", () => {
  it("offers to move a chat that is in no project", async () => {
    wrap(<ProjectContextSection conversationId="c1" />);
    expect(await screen.findByText(/Move this chat into a project/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Move to project" })).toBeInTheDocument();
  });
});
