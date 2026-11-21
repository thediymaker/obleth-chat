import { describe, expect, it } from "vitest";
import { followed, jobCards, jobTone, jobView, span, stateLabel } from "./job-card";

const now = 1795001000 * 1000;

describe("jobTone and stateLabel", () => {
  it("reads Slurm's states", () => {
    expect(["RUNNING", "PENDING", "COMPLETED", "OUT_OF_MEMORY", "CANCELLED by 1001", "TIMEOUT", "NODE_FAIL", "SPECIAL_EXIT", ""].map(jobTone)).toEqual(["running", "pending", "done", "failed", "failed", "failed", "failed", "ended", "ended"]);
    expect(stateLabel("OUT_OF_MEMORY")).toBe("Out of memory");
    expect(stateLabel("RUNNING,COMPLETING")).toBe("Running");
    expect(stateLabel(undefined)).toBe("Unknown");
  });
});

describe("span", () => {
  it("writes a time the way the scheduler does", () => {
    expect([45, 724, 7500].map(span)).toEqual(["45s", "12m04s", "2h05m"]);
  });
});

describe("jobCards", () => {
  it("keeps the newest word on each job and the last list", () => {
    const found = jobCards([
      { status: "completed", card: { kind: "slurm-job", connectionId: "k1", cluster: "Sol", job: { id: 7, name: "sweep", state: "PENDING", submitted: 1 } } },
      { status: "completed", card: { kind: "slurm-jobs", connectionId: "k1", cluster: "Sol", jobs: [{ id: 7 }, { id: 8 }] } },
      { status: "completed", card: { kind: "slurm-job", connectionId: "k1", cluster: "Sol", job: { id: 7, state: "RUNNING", started: 5 } } },
      { status: "error", card: { kind: "slurm-job", connectionId: "k1", job: { id: 9 } } },
      { status: "completed", detail: "no card" },
    ]);
    expect(found.jobs).toEqual([{ key: "k1:7", cluster: "Sol", connectionId: "k1", job: { id: 7, name: "sweep", state: "RUNNING", submitted: 1, started: 5 } }]);
    expect(found.list?.jobs.length).toBe(2);
    expect(jobCards(undefined)).toEqual({ jobs: [], list: null });
  });
});

describe("followed", () => {
  const job = { id: 7, name: "sweep", state: "PENDING", reason: "Priority" };
  it("takes what the cluster says now", () => {
    expect(followed(job, { jobs: [{ id: "7", state: "RUNNING", nodes: "g042", reason: "", elapsed: "3m10s" }], recent: [] })).toEqual({ ...job, state: "RUNNING", nodes: "g042", reason: "", elapsed: "3m10s" });
    expect(followed(job, { jobs: [], recent: [{ id: "7", state: "COMPLETED", ended: 99, exit: 0, elapsed: "12m34s" }] })).toEqual({ ...job, state: "COMPLETED", ended: 99, exit: 0, elapsed: "12m34s", reason: "" });
  });
  it("keeps what the step found when the cluster does not list the job", () => {
    expect(followed(job, { jobs: [], recent: [] })).toEqual(job);
    expect(followed(job, undefined)).toEqual(job);
  });
});

describe("jobView", () => {
  it("shows how far a running job is through its limit", () => {
    expect(jobView({ id: 1, state: "RUNNING", partition: "gpu", nodes: "g042", cpus: 8, timeLimit: "2h00m", limitMinutes: 120, started: 1795001000 - 724 }, now)).toEqual({
      tone: "running",
      state: "Running",
      share: 724 / 7200,
      line: "12m04s of 2h00m",
      facts: ["gpu", "g042", "8 CPUs"],
    });
    expect(jobView({ id: 1, state: "RUNNING", elapsed: "3m10s", timeLimit: "unlimited" }, now).line).toBe("3m10s of unlimited");
    expect(jobView({ id: 1, state: "RUNNING", elapsed: "3m10s", timeLimit: "unlimited" }, now).share).toBeNull();
  });
  it("says why a pending job waits", () => {
    expect(jobView({ id: 1, state: "PENDING", reason: "Priority", partition: "gpu", timeLimit: "45m" }, now)).toEqual({ tone: "pending", state: "Pending", share: null, line: "Waiting · Priority · limit 45m", facts: ["gpu"] });
  });
  it("says how a job ended", () => {
    expect(jobView({ id: 1, state: "COMPLETED", started: 100, ended: 854, exit: 0 }, now).line).toBe("Ran 12m34s · exit 0");
    expect(jobView({ id: 1, state: "FAILED", elapsed: "2m00s", exit: 1, reason: "NonZeroExitCode" }, now).line).toBe("Ran 2m00s · exit 1 · NonZeroExitCode");
  });
});
