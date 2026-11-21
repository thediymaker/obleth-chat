// Cluster jobs as cards above a reply's answer.

import { screen } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
vi.mock("next/link", () => ({ default: ({ href, children, ...rest }: { href: string; children: React.ReactNode }) => <a href={href} {...rest}>{children}</a> }));
import type { Message } from "@/lib/chat/types";
import { JobCards } from "./job-card";
import { renderWithQueries } from "./test-queries";

/** The clusters asked for their status. */
const asked = () => apiRequest.mock.calls.map(([path]) => String(path)).filter((path) => path.endsWith("/status"));
beforeEach(() => {
  apiRequest.mockReset();
  apiRequest.mockRejectedValue(new Error("offline"));
});

const step = (card: unknown) => ({ status: "completed", connection: "Sol", card });
const message = (activity: unknown[]) => ({ id: "m", role: "assistant", status: "completed", content: "", activity }) as unknown as Message;

test("a reply without jobs shows no card", () => {
  const { container } = renderWithQueries(<JobCards m={message([{ status: "completed", detail: "text" }])} />);
  expect(container.textContent).toBe("");
  expect(asked()).toEqual([]);
});

test("a finished job is shown as the step found it, without asking the cluster", () => {
  renderWithQueries(<JobCards m={message([step({ kind: "slurm-job", connectionId: "k1", cluster: "Sol", job: { id: 41, name: "sweep-32", state: "COMPLETED", partition: "gpu", started: 100, ended: 854, exit: 0 } })])} />);
  const card = screen.getByRole("article", { name: "Job sweep-32 on Sol: Completed" });
  expect(card.textContent).toContain("Ran 12m34s · exit 0");
  expect(card.getAttribute("data-job-card")).toBe("done");
  expect(asked()).toEqual([]);
});

test("a job that was waiting follows the cluster, and the rest of a list gets its own card", async () => {
  apiRequest.mockImplementation(async (path: string) => {
    if (path === "/api/capabilities") return { features: { connections: true, clusters: true } };
    if (path === "/api/connections") return { connections: [{ id: "k1", kind: "slurm", name: "Sol", active: true, tools: [] }] };
    if (path === "/api/connections/k1/status") return { state: "connected", status: { jobs: [{ id: 42, name: "sweep-64", state: "RUNNING", nodes: "g042", elapsed: "3m10s" }], recent: [] } };
    throw new Error(`unexpected ${path}`);
  });
  renderWithQueries(
    <JobCards
      m={message([
        step({ kind: "slurm-job", connectionId: "k1", cluster: "Sol", job: { id: 42, name: "sweep-64", state: "PENDING", reason: "Priority", timeLimit: "2h00m" } }),
        step({ kind: "slurm-jobs", connectionId: "k1", cluster: "Sol", jobs: [{ id: 42, name: "sweep-64", state: "PENDING" }, { id: 43, name: "sweep-128", state: "PENDING", reason: "Resources" }] }),
      ])}
    />,
  );
  expect(screen.getByRole("article", { name: "Job sweep-64 on Sol: Pending" }).textContent).toContain("Waiting · Priority");
  const running = await screen.findByRole("article", { name: "Job sweep-64 on Sol: Running" });
  expect(running.textContent).toContain("3m10s of 2h00m");
  expect(running.textContent).toContain("g042");
  const list = screen.getByRole("article", { name: "Your jobs on Sol" });
  expect(list.textContent).toContain("sweep-128");
  expect(list.textContent).not.toContain("sweep-64");
});
