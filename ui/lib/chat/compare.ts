// A reply's versions side by side. Asking again, with the same model or another,
// keeps every answer as a version of the reply; this reads them next to each other
// with what each cost, and lets the person keep the one they prefer.

import type { Usage } from "./types";

/** One version of a reply, as GET …/messages/:id/versions gives it. */
export interface ReplyVersion {
  id: string;
  status: string;
  createdAt: string;
  /** The model that wrote it; the one Auto chose, when Auto did. */
  model: string;
  content: string;
  usage?: Usage | null;
  /** The version the chat shows. */
  selected: boolean;
}

/** The two versions to show first: the one in the chat, and the newest other one. */
export function firstPair(versions: ReplyVersion[]): [string, string] | null {
  if (versions.length < 2) return null;
  const shown = versions.find((v) => v.selected) ?? versions.at(-1)!;
  const other = [...versions].reverse().find((v) => v.id !== shown.id)!;
  // In the order they were written.
  return versions.indexOf(shown) < versions.indexOf(other) ? [shown.id, other.id] : [other.id, shown.id];
}

/** What a version cost and how fast it came, as label and value. Empty for what was not measured. */
export function versionStats(v: Pick<ReplyVersion, "usage">): [string, string][] {
  const usage = v.usage || {};
  const written = Number(usage.completion_tokens) || 0,
    writing = Number(usage.generation_ms) || 0,
    rate = writing >= 200 && written > 1 ? written / (writing / 1000) : 0,
    first = usage.first_token_ms != null ? Number(usage.first_token_ms) : null;
  const stats: [string, string | null][] = [
    ["Output", written ? `${written.toLocaleString()} tokens` : null],
    ["Speed", rate ? `${rate >= 100 ? Math.round(rate).toLocaleString() : rate.toFixed(1)} tokens/s` : null],
    ["First token", first != null ? `${(first / 1000).toFixed(first < 10000 ? 2 : 1)} s` : null],
    ["Time", writing ? `${(writing / 1000).toFixed(writing < 10000 ? 1 : 0)} s` : null],
    ["Steps", (usage.steps || 0) > 1 ? String(usage.steps) : null],
  ];
  return stats.filter((s): s is [string, string] => s[1] != null);
}

/** Which side did better on a stat, for marking it: lower is better except for speed. */
export function better(label: string, left: string | undefined, right: string | undefined): "left" | "right" | null {
  if (!left || !right || !["Speed", "First token", "Time"].includes(label)) return null;
  const a = Number(left.replace(/,/g, "").split(" ")[0]),
    b = Number(right.replace(/,/g, "").split(" ")[0]);
  if (!Number.isFinite(a) || !Number.isFinite(b) || a === b) return null;
  return (label === "Speed" ? a > b : a < b) ? "left" : "right";
}
