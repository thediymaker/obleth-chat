// The person's own workspace container, now: what it uses of what it may, and when
// it goes to sleep if left alone. The numbers come from the cluster's metrics, so
// they can be missing; the card says what it knows and leaves the rest out.

import { bytes, durationWords } from "@/lib/format";

export interface SandboxUsage {
  status: "none" | "starting" | "running" | "stopped" | "error";
  cpuMillis?: number | null;
  memoryBytes?: number | null;
  diskBytes?: number | null;
  limits?: { cpuMillis?: number | null; memoryBytes?: number | null; diskBytes?: number | null };
  idleMinutes?: number;
  lastUsedAt?: string;
  stopsAt?: string | null;
}

export interface Meter {
  key: "cpu" | "memory" | "disk";
  label: string;
  /** "0.25 of 2 cores", "412 MB of 2.0 GB". */
  text: string;
  /** Short, for the status line: "0.25 cpu", "412 MB". */
  short: string;
  /** 0 to 1 of the limit, or null where there is no limit to hold it against. */
  share: number | null;
}

export interface SandboxView {
  state: SandboxUsage["status"];
  /** "Awake", "Starting", "Asleep"… */
  label: string;
  /** Whether it is alive now: the accent marks it. */
  alive: boolean;
  meters: Meter[];
  /** "Sleeps in 42 minutes if left alone", "Asleep: it wakes when you need it"… */
  note: string;
}

const LABEL: Record<SandboxUsage["status"], string> = { none: "Not started", starting: "Starting", running: "Awake", stopped: "Asleep", error: "Could not start" };

const cores = (millis: number) => {
  const n = millis / 1000;
  return n >= 10 ? String(Math.round(n)) : n.toFixed(n < 1 ? 2 : 1).replace(/\.?0+$/, "") || "0";
};
const share = (used: number, limit: number | null | undefined) => (limit && limit > 0 ? Math.min(1, Math.max(0, used / limit)) : null);

/** The meters the usage has numbers for, in the order CPU, memory, disk. */
export function sandboxMeters(usage: SandboxUsage): Meter[] {
  const limits = usage.limits || {};
  const out: Meter[] = [];
  if (usage.cpuMillis != null) {
    const limit = limits.cpuMillis;
    out.push({ key: "cpu", label: "CPU", text: limit ? `${cores(usage.cpuMillis)} of ${cores(limit)} ${limit === 1000 ? "core" : "cores"}` : `${cores(usage.cpuMillis)} cores`, short: `${cores(usage.cpuMillis)} cpu`, share: share(usage.cpuMillis, limit) });
  }
  if (usage.memoryBytes != null) out.push({ key: "memory", label: "Memory", text: limits.memoryBytes ? `${bytes(usage.memoryBytes)} of ${bytes(limits.memoryBytes)}` : bytes(usage.memoryBytes), short: bytes(usage.memoryBytes), share: share(usage.memoryBytes, limits.memoryBytes) });
  if (usage.diskBytes != null) out.push({ key: "disk", label: "Disk", text: limits.diskBytes ? `${bytes(usage.diskBytes)} of ${bytes(limits.diskBytes)}` : bytes(usage.diskBytes), short: bytes(usage.diskBytes), share: share(usage.diskBytes, limits.diskBytes) });
  return out;
}

/** When it sleeps, or what waking takes, in a line. */
export function sandboxNote(usage: SandboxUsage, now = Date.now()): string {
  const idle = usage.idleMinutes ? durationWords(usage.idleMinutes * 60) : "";
  if (usage.status === "running") {
    const left = usage.stopsAt ? Date.parse(usage.stopsAt) - now : NaN;
    if (!Number.isFinite(left)) return idle ? `Sleeps after ${idle} unused. Files are kept.` : "";
    return left <= 60_000 ? "Sleeps within a minute if left alone. Files are kept." : `Sleeps in ${durationWords(left / 1000)} if left alone. Files are kept.`;
  }
  if (usage.status === "starting") return "Waking up.";
  if (usage.status === "stopped") return "Asleep: it wakes when you open a project or the assistant needs it. Files are kept.";
  if (usage.status === "error") return "It could not start. Opening a workspace tries again.";
  return idle ? `Starts with your first workspace, and sleeps after ${idle} unused.` : "Starts with your first workspace.";
}

export function sandboxView(usage: SandboxUsage, now = Date.now()): SandboxView {
  return { state: usage.status, label: LABEL[usage.status] ?? usage.status, alive: usage.status === "running" || usage.status === "starting", meters: sandboxMeters(usage), note: sandboxNote(usage, now) };
}

/** For the status line: "0.25 cpu · 412 MB", or "" when the numbers are missing. */
export const sandboxShort = (usage: SandboxUsage | undefined) =>
  usage?.status === "running"
    ? sandboxMeters(usage)
        .filter((m) => m.key !== "disk")
        .map((m) => m.short)
        .join(" · ")
    : "";

/** How full a meter reads: calm, getting close, at the limit. */
export const meterTone = (value: number | null) => (value == null ? "calm" : value > 0.9 ? "full" : value > 0.75 ? "close" : "calm");
