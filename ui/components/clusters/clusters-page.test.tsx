import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { ClustersPage } from "./clusters-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push, replace: vi.fn() }), usePathname: () => "/clusters" }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
vi.mock("@/components/app-shell/chrome", () => ({ usePageChrome: () => {} }));

const slurm = { id: "s1", kind: "slurm", name: "Sol", url: "https://sol:6820", shared: true, enabled: true, active: true, canManage: true, confirmActions: true, credential: { user: "ana" }, files: true, tools: [] };
const k8s = { id: "k1", kind: "kubernetes", name: "Lab k8s", url: "https://k8s:6443", shared: true, enabled: true, active: false, needsCredential: true, confirmActions: true, tools: [] };
const slurmStatus = {
  state: "connected",
  checkedAt: new Date().toISOString(),
  fileSession: { state: "live", jobId: 4471233, host: "c001", lastSeenAt: new Date().toISOString(), endsAt: new Date(Date.now() + 3600_000).toISOString() },
  status: {
    user: "ana",
    totals: { cpus: 100, allocCpus: 40, gpus: 8, allocGpus: 2, memoryMb: 409600, allocMemoryMb: 102400, nodes: 4, idleNodes: 1 },
    queue: { running: 1, pending: 2 },
    jobs: [{ id: 42, name: "train", state: "RUNNING", partition: "gpu", elapsed: "1:00", timeLimit: "4:00", nodes: "g1" }],
    recent: [{ id: 41, name: "assay-summary", state: "FAILED", partition: "general", elapsed: "0:12", ended: 1_762_200_000, exit: 2 }],
    partitions: [{ name: "gpu", state: "UP", nodes: 4, idleNodes: 1, cpus: 100, idleCpus: 60, gpus: 8, idleGpus: 6, maxTime: "7-00:00:00" }],
  },
};

beforeEach(() => {
  fakeStore();
});

describe("Compute › Clusters", () => {
  it("shows a Slurm cluster's state and asks for a token where one is needed", async () => {
    const calls = mockApi((path) => (path === "/api/connections" ? { connections: [slurm, k8s] } : path.endsWith("/status") ? slurmStatus : {}), { connections: true, clusters: true });
    renderPage(<ClustersPage />);
    expect(await screen.findByText("Connected")).toBeInTheDocument();
    expect(screen.getByText("40 / 100")).toBeInTheDocument();
    expect(screen.getByText("Your jobs as ana")).toBeInTheDocument();
    expect(screen.getByText("train")).toBeInTheDocument();
    const ended = screen.getByText("assay-summary").closest("tr")!;
    expect(ended).toHaveTextContent("FAILED");
    expect(ended).toHaveTextContent("2");
    expect(screen.getByText("Recent jobs")).toBeInTheDocument();
    expect(screen.getByText("Idle GPUs")).toBeInTheDocument();
    expect(screen.getByText("Needs your token")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add your token" })).toBeInTheDocument();
    // A cluster without a token is not asked for its status.
    expect(calls.some((c) => c.path === "/api/connections/k1/status")).toBe(false);
    // The person's file session, and a way to end it.
    expect(screen.getByText("File session")).toBeInTheDocument();
    expect(screen.getByText(/Job 4471233 on c001, last call/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "End session" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/api/connections/s1/file-session" && c.method === "DELETE")).toBe(true));
  });

  it("asks in a new chat about a cluster", async () => {
    const store = fakeStore();
    mockApi((path) => (path === "/api/connections" ? { connections: [slurm] } : slurmStatus), { connections: true });
    renderPage(<ClustersPage />);
    fireEvent.click(await screen.findByRole("button", { name: "Ask in a new chat" }));
    expect(store.starter).toHaveBeenCalledWith("Using the Sol cluster, ", false);
    expect(push).toHaveBeenCalledWith("/");
  });

  it("shows an unreachable cluster's error, and a Kubernetes namespace it may not use", async () => {
    const k = { ...k8s, needsCredential: false, active: true };
    mockApi(
      (path) =>
        path === "/api/connections"
          ? { connections: [{ ...slurm, id: "s2" }, k] }
          : path.includes("s2")
            ? { state: "error", error: "Connection refused" }
            : { state: "connected", status: { namespace: "lab", homeNamespace: "ana", allowed: [], pods: { error: "Forbidden" }, deployments: [], jobs: [], nodes: { error: "Forbidden" }, quota: [] } },
      { connections: true },
    );
    renderPage(<ClustersPage />);
    expect(await screen.findByText("Connection refused")).toBeInTheDocument();
    expect(await screen.findByText(/This account has no permissions in lab/)).toBeInTheDocument();
    expect(screen.getByText("Not visible")).toBeInTheDocument();
  });

  it("starts adding a cluster with the kind picker", async () => {
    mockApi(() => ({ connections: [] }), { connections: true });
    renderPage(<ClustersPage />);
    expect(await screen.findByText("No clusters yet")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Add cluster" })[0]!);
    expect(await screen.findByText("Slurm cluster")).toBeInTheDocument();
    expect(screen.getByText("Kubernetes cluster")).toBeInTheDocument();
  });

  it("says when connections are off", async () => {
    mockApi(() => ({}), {});
    renderPage(<ClustersPage />);
    await waitFor(() => expect(screen.getByText("Clusters are not enabled.")).toBeInTheDocument());
  });
});
