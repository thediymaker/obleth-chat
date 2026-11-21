// A cluster job as a card in the reply. The Slurm tools leave what they found on
// their step (`card`): one job, or the person's jobs. The card starts from that
// and, while the job is still going, follows what the cluster says now.

import type { ActivityEntry } from "./types";

/** A job as api/src/connections/slurm.rs gives it; times are Unix seconds. */
export interface Job {
  id: string | number;
  name?: string;
  state?: string;
  partition?: string;
  /** "2h00m", "45m", "unlimited" or "-". */
  timeLimit?: string;
  limitMinutes?: number | null;
  nodes?: string;
  reason?: string;
  cpus?: number | null;
  /** How long it has run, as text, when the start time is not known. */
  elapsed?: string;
  submitted?: number | null;
  started?: number | null;
  ended?: number | null;
  exit?: number | null;
}

export interface JobRef {
  key: string;
  cluster: string;
  connectionId: string;
  job: Job;
}

export type JobTone = "running" | "pending" | "done" | "failed" | "ended";

export function jobTone(state: string | undefined): JobTone {
  const s = String(state ?? "").toUpperCase();
  if (/RUNNING|COMPLETING/.test(s)) return "running";
  if (/PENDING|CONFIGURING|SUSPENDED|REQUEUED|RESIZING/.test(s)) return "pending";
  if (/COMPLETED/.test(s)) return "done";
  if (/FAIL|TIMEOUT|CANCEL|OUT_OF_MEMORY|PREEMPTED|DEADLINE|REVOKED/.test(s)) return "failed";
  return "ended";
}
export const jobActive = (job: Job) => ["running", "pending"].includes(jobTone(job.state));

/** "OUT_OF_MEMORY" as "Out of memory". */
export const stateLabel = (state: string | undefined) => {
  const s = String(state ?? "").split(",")[0]!.replace(/_/g, " ").toLowerCase();
  return s ? s[0]!.toUpperCase() + s.slice(1) : "Unknown";
};

/**
 * The jobs a reply's steps found, the newest word on each: single jobs as cards,
 * and the last list of jobs (when no single job was looked at after it).
 */
export function jobCards(activity: ActivityEntry[] | undefined): { jobs: JobRef[]; list: { cluster: string; connectionId: string; jobs: Job[] } | null } {
  const single = new Map<string, JobRef>();
  let list: { cluster: string; connectionId: string; jobs: Job[] } | null = null;
  for (const step of activity || []) {
    const card = step.card;
    if (!card || step.status !== "completed") continue;
    const cluster = String(card.cluster ?? step.connection ?? ""),
      connectionId = String(card.connectionId ?? "");
    if (card.kind === "slurm-job" && card.job?.id != null) {
      const key = `${connectionId}:${card.job.id}`;
      // A later look at the same job replaces the earlier one, in its place.
      single.set(key, { key, cluster, connectionId, job: { ...single.get(key)?.job, ...card.job } });
    } else if (card.kind === "slurm-jobs" && Array.isArray(card.jobs)) list = { cluster, connectionId, jobs: card.jobs };
  }
  return { jobs: [...single.values()], list };
}

interface Now {
  jobs: { id: string; state: string; elapsed?: string; nodes?: string; reason?: string }[];
  recent: { id: string; state: string; ended?: number | null; exit?: number | null; elapsed?: string }[];
}

/** The job with what the cluster says of it now, when it says anything. */
export function followed(job: Job, now: Now | undefined): Job {
  if (!now) return job;
  const id = String(job.id);
  const live = now.jobs.find((j) => j.id === id);
  if (live) return { ...job, state: live.state || job.state, nodes: live.nodes || job.nodes, reason: live.reason ?? job.reason, elapsed: live.elapsed && live.elapsed !== "-" ? live.elapsed : job.elapsed };
  const over = now.recent.find((j) => j.id === id);
  if (over) return { ...job, state: over.state || job.state, ended: over.ended ?? job.ended, exit: over.exit ?? job.exit, elapsed: over.elapsed || job.elapsed, reason: "" };
  return job;
}

/** A span as the scheduler writes one: "45s", "12m04s", "2h05m". */
export function span(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600),
    m = Math.floor((s % 3600) / 60);
  return h ? `${h}h${String(m).padStart(2, "0")}m` : m ? `${m}m${String(s % 60).padStart(2, "0")}s` : `${s}s`;
}

export interface JobView {
  tone: JobTone;
  state: string;
  /** How far through its time limit a running job is, 0 to 1, when both are known. */
  share: number | null;
  /** "12m04s of 2h00m", "Waiting · Priority", "Ran 12m34s · exit 0". */
  line: string;
  /** Where and how big: "gpu", "g042", "8 CPUs". */
  facts: string[];
}

export function jobView(job: Job, now = Date.now()): JobView {
  const tone = jobTone(job.state);
  const limit = job.timeLimit && job.timeLimit !== "-" ? job.timeLimit : "";
  const ran = job.started ? ((job.ended || now / 1000) - job.started) : null;
  const elapsed = ran != null && ran >= 0 ? span(ran) : job.elapsed && job.elapsed !== "-" ? job.elapsed : "";
  let line = "",
    share: number | null = null;
  if (tone === "running") {
    line = [elapsed, limit && elapsed ? `of ${limit}` : limit ? `limit ${limit}` : ""].filter(Boolean).join(" ");
    if (ran != null && job.limitMinutes) share = Math.min(1, Math.max(0, ran / (job.limitMinutes * 60)));
  } else if (tone === "pending") line = ["Waiting", job.reason, limit ? `limit ${limit}` : ""].filter(Boolean).join(" · ");
  else line = [elapsed ? `Ran ${elapsed}` : "", job.exit != null ? `exit ${job.exit}` : "", tone === "failed" && job.reason ? job.reason : ""].filter(Boolean).join(" · ");
  const facts = [job.partition, job.nodes, job.cpus ? `${job.cpus} CPU${job.cpus === 1 ? "" : "s"}` : ""].filter((f): f is string => !!f);
  return { tone, state: stateLabel(job.state), share, line, facts };
}
