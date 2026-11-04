// Workspaces and browsers for administrators: the pods open now with what they use, and over
// a period their pod hours, the busiest moment against the slots, and starts that failed or
// were turned away. Shows who and when, never what ran or which pages a browser had open.

import { when } from "@/lib/format";

export const WORKSPACE_PERIODS = [
  [1, "Today"],
  [7, "7 days"],
  [28, "28 days"],
  [90, "90 days"],
] as const;

export const bytes = (value: number | null | undefined) => {
  const n = Number(value || 0);
  return n >= 2 ** 30 ? `${(n / 2 ** 30).toFixed(1)} GiB` : n >= 2 ** 20 ? `${Math.round(n / 2 ** 20)} MiB` : n >= 1024 ? `${Math.round(n / 1024)} KiB` : `${n} B`;
};
export const cores = (m: number | null | undefined) => (m == null ? "—" : Number(m) >= 1000 ? `${(Number(m) / 1000).toFixed(1)} cores` : `${Math.round(Number(m))}m`);
export const hours = (value: number | null | undefined) => {
  const h = Number(value || 0);
  return h >= 100 ? `${Math.round(h).toLocaleString()} h` : h >= 1 ? `${h.toFixed(1)} h` : `${Math.round(h * 60)} min`;
};
/** How long until `at`, in plain words. */
export function until(at: string | null | undefined, now = Date.now()) {
  if (!at) return "—";
  const minutes = Math.round((new Date(at).getTime() - now) / 60000);
  const words = (m: number) => (m >= 120 ? `${Math.round(m / 60)} hours` : m >= 60 ? "1 hour" : `${Math.max(1, m)} min`);
  return minutes > 0 ? `in ${words(minutes)}` : "any moment";
}
/** How long since `at`, in plain words. */
export function since(at: string | null | undefined, now = Date.now()) {
  if (!at) return "—";
  const minutes = Math.round((now - new Date(at).getTime()) / 60000);
  return minutes < 1 ? "just now" : minutes < 60 ? `${minutes} min ago` : minutes < 48 * 60 ? `${Math.round(minutes / 60)} h ago` : when(at);
}

export interface PodCounts {
  running?: number;
  starting?: number;
  error?: number;
}
export interface OpenPod {
  kind: "workspace" | "browser";
  ownerId: string;
  username: string;
  displayName?: string;
  status: string;
  errorMessage?: string;
  startedAt?: string;
  lastUsedAt?: string;
  stopsAt?: string;
  chats?: number;
  previews?: number;
  cpuMillis?: number | null;
  memoryBytes?: number | null;
  diskBytes?: number | null;
}
export interface HistoryPoint {
  at: string;
  workspaceHours?: number;
  browserHours?: number;
  peak?: number;
  starts?: number;
  failed?: number;
  refused?: number;
}
export interface WorkspacesOverview {
  enabled: boolean;
  off?: string;
  days?: number;
  start?: string;
  slots?: number;
  everyone?: boolean;
  browsers?: boolean;
  metrics?: boolean;
  idleMinutes?: { workspace?: number; browser?: number };
  limits?: { cpu?: string; memory?: string; disk?: string; browserCpu?: string; browserMemory?: string };
  now?: { inUse?: number; workspaces?: PodCounts; browsers?: PodCounts; previews?: number; cpuMillis?: number; memoryBytes?: number };
  open?: OpenPod[];
  history?: {
    series?: HistoryPoint[];
    totals?: HistoryPoint & { peakAt?: string; people?: number; medianStartSeconds?: number | null };
  };
  recordedSince?: string;
}

/** The label of one bar: its hour for today, else its day. */
export const pointLabel = (at: string, days: number) => {
  const d = new Date(at);
  return days === 1 ? `${String(d.getUTCHours()).padStart(2, "0")}:00` : d.toLocaleDateString(undefined, { month: "short", day: "numeric", timeZone: "UTC" });
};
const count = (n: number | null | undefined) => Number(n || 0).toLocaleString();
const plural = (n: number | null | undefined, one: string, many: string) => `${count(n)} ${Number(n) === 1 ? one : many}`;
/** Everything about one bar of the period chart, in a sentence. */
export const pointTip = (p: HistoryPoint, days: number) =>
  `${pointLabel(p.at, days)}: ${hours(p.workspaceHours)} of workspaces, ${hours(p.browserHours)} of browsers, at most ${plural(p.peak, "pod", "pods")} at once, ${plural(p.starts, "start", "starts")}${p.failed ? `, ${count(p.failed)} failed` : ""}${p.refused ? `, ${count(p.refused)} turned away` : ""}`;
