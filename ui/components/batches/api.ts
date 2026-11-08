"use client";

// Batches: one prompt run over every row of a spreadsheet by the gateway, in the
// background. Endpoints in api/src/batches.rs.

import { type QueryClient, useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { useSignedIn } from "@/lib/queries";

export type BatchStatus = "validating" | "failed" | "in_progress" | "finalizing" | "completed" | "expired" | "cancelling" | "cancelled";

export interface Batch {
  id: string;
  name: string;
  model: string;
  status: BatchStatus;
  total: number;
  completed: number;
  failed: number;
  createdAt: string | null;
  inProgressAt: string | null;
  finishedAt: string | null;
  deadlineAt: string | null;
  lastProgressAt: string | null;
  /** No request has finished for a while: the model is busy with people chatting. */
  waiting: boolean;
  window: string | null;
  error: string | null;
  fileName: string | null;
  rows: number | null;
  fromChat: boolean;
  /** The person's own tags for it. */
  tags: string[];
}

export interface BatchList {
  available: boolean;
  batches: Batch[];
  /** Older batches exist beyond the ones listed. */
  more?: boolean;
}

export interface NewBatch {
  name: string;
  model: string;
  system: string;
  template: string;
  columns: string[];
  rows: string[][];
  window: string;
  fileName: string;
}

export const BATCHES_KEY = ["batches"] as const;

const ACTIVE: BatchStatus[] = ["validating", "in_progress", "finalizing", "cancelling"];
export const isActive = (b: Pick<Batch, "status">) => ACTIVE.includes(b.status);

export function useBatches() {
  return useQuery({
    queryKey: BATCHES_KEY,
    queryFn: () => api.request<BatchList>("/api/batches"),
    enabled: useSignedIn(),
    refetchInterval: (q) => (q.state.data?.batches.some(isActive) ? 3_000 : 30_000),
  });
}
export const refreshBatches = (client: QueryClient) => client.invalidateQueries({ queryKey: BATCHES_KEY });

/** Starts a batch. Sent as a raw body so a large spreadsheet gets the long upload timeout. */
export async function startBatch(batch: NewBatch): Promise<Batch> {
  const r = await api.request<{ batch: Batch }>("/api/batches", { method: "POST", raw: JSON.stringify(batch), contentType: "application/json" });
  return r.batch;
}
export async function cancelBatch(id: string): Promise<Batch> {
  const r = await api.request<{ batch: Batch }>(`/api/batches/${encodeURIComponent(id)}/cancel`, { method: "POST" });
  return r.batch;
}
/** Renames a batch or sets its tags; only this person sees them. An empty name goes back to the first one. */
export async function updateBatch(id: string, change: { name?: string; tags?: string[] }): Promise<Batch> {
  const r = await api.request<{ batch: Batch }>(`/api/batches/${encodeURIComponent(id)}`, { method: "PATCH", body: change });
  return r.batch;
}
/** Deletes a batch that has stopped, with its answers and the spreadsheet. */
export async function deleteBatch(id: string): Promise<void> {
  await api.request(`/api/batches/${encodeURIComponent(id)}`, { method: "DELETE" });
}
export const resultsHref = (id: string) => `/api/batches/${encodeURIComponent(id)}/results`;

export const doneOf = (b: Pick<Batch, "completed" | "failed">) => b.completed + b.failed;
export function percentOf(b: Pick<Batch, "completed" | "failed" | "total">): number {
  return b.total > 0 ? Math.min(100, Math.floor((doneOf(b) / b.total) * 100)) : 0;
}

/** A length of time in words: "45 seconds", "12 minutes", "3 hours 5 minutes", "2 days". */
export function duration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"}`;
  if (s < 60) return plural(s, "second");
  const m = Math.round(s / 60);
  if (m < 60) return plural(m, "minute");
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${plural(h, "hour")} ${plural(m % 60, "minute")}` : plural(h, "hour");
  const d = Math.floor(h / 24);
  return h % 24 ? `${plural(d, "day")} ${plural(h % 24, "hour")}` : plural(d, "day");
}

function secsSince(iso: string | null | undefined, now: number): number | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isNaN(t) ? null : (now - t) / 1000;
}

/** Seconds until the rest finishes at the pace so far, or null without a pace yet. */
export function etaSecs(b: Batch, now: number): number | null {
  if (b.status !== "in_progress") return null;
  const done = doneOf(b);
  const running = secsSince(b.inProgressAt ?? b.createdAt, now);
  if (done <= 0 || done >= b.total || !running || running <= 0) return null;
  return (b.total - done) / (done / running);
}

export type Tone = "running" | "waiting" | "done" | "problem" | "stopped";

/** What a batch is doing, in plain words. */
export function stateOf(b: Batch, now: number): { label: string; tone: Tone; detail: string | null } {
  const left = b.total - doneOf(b);
  switch (b.status) {
    case "validating":
      return { label: "Checking the requests", tone: "running", detail: null };
    case "in_progress": {
      if (b.waiting) {
        const idle = secsSince(b.lastProgressAt ?? b.inProgressAt, now);
        return {
          label: "Waiting for the model",
          tone: "waiting",
          detail: `It's busy with people chatting, and batches wait their turn.${idle != null ? ` Nothing has finished in ${duration(idle)}.` : ""}`,
        };
      }
      const eta = etaSecs(b, now);
      return { label: "Running", tone: "running", detail: eta != null ? `About ${duration(eta)} left at this pace` : "Starting" };
    }
    case "finalizing":
      return { label: "Finishing up", tone: "running", detail: null };
    case "completed":
      return b.failed > 0
        ? { label: "Done", tone: "problem", detail: `${b.failed.toLocaleString()} row${b.failed === 1 ? "" : "s"} failed. The results show why.` }
        : { label: "Done", tone: "done", detail: null };
    case "failed":
      return { label: "Couldn't start", tone: "problem", detail: b.error ?? "The requests had errors" };
    case "expired":
      return { label: "Ran out of time", tone: "problem", detail: `${left.toLocaleString()} row${left === 1 ? "" : "s"} didn't run before the deadline. The rest are in the results.` };
    case "cancelling":
      return { label: "Stopping", tone: "stopped", detail: "Rows already running finish first" };
    case "cancelled":
      return { label: "Cancelled", tone: "stopped", detail: doneOf(b) > 0 ? "Rows that finished are in the results" : null };
    default:
      return { label: String(b.status), tone: "stopped", detail: null };
  }
}

export const WINDOWS = [
  ["24h", "Within a day"],
  ["3d", "Within 3 days"],
  ["7d", "Within a week"],
] as const;

/** The groups the list filters by. */
export type Group = "all" | "running" | "done" | "problems" | "cancelled";

export const GROUPS: readonly (readonly [Group, string])[] = [
  ["all", "All"],
  ["running", "Running"],
  ["done", "Done"],
  ["problems", "Needs a look"],
  ["cancelled", "Cancelled"],
];

export function groupOf(b: Pick<Batch, "status" | "failed">): Exclude<Group, "all"> {
  if (isActive(b)) return "running";
  if (b.status === "cancelled") return "cancelled";
  if (b.status === "completed" && b.failed === 0) return "done";
  return "problems";
}

export interface ListFilter {
  group: Group;
  tag: string;
  query: string;
}

/** Whether a batch shows under the filter: its group, its tag, and words in its name, model, file or tags. */
export function matches(b: Batch, f: ListFilter): boolean {
  if (f.group !== "all" && groupOf(b) !== f.group) return false;
  if (f.tag && !b.tags.includes(f.tag)) return false;
  const words = f.query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const text = [b.name, b.model, b.fileName ?? "", b.id, ...b.tags.map((t) => `#${t}`)].join(" ").toLowerCase();
  return words.every((w) => text.includes(w));
}

/** Each tag the batches use, most used first. */
export function tagCounts(batches: Pick<Batch, "tags">[]): { tag: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const b of batches) for (const t of b.tags) counts.set(t, (counts.get(t) ?? 0) + 1);
  return [...counts].map(([tag, count]) => ({ tag, count })).sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
}

export function groupCounts(batches: Pick<Batch, "status" | "failed">[]): Record<Group, number> {
  const counts: Record<Group, number> = { all: batches.length, running: 0, done: 0, problems: 0, cancelled: 0 };
  for (const b of batches) counts[groupOf(b)] += 1;
  return counts;
}

/** A batch can be deleted once it has stopped. */
export const deletable = (b: Pick<Batch, "status">) => !isActive(b);
