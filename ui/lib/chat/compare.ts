// A reply's versions side by side. Asking again, with the same model or another,
// keeps every answer as a version of the reply; this reads them next to each other
// with what each cost, and lets the person keep the one they prefer. Now and then
// Chat asks other models by itself (api/src/compare.rs): the two models Auto ranked
// next answer in the background, and the reply says so.

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
  /** Auto chose its model. */
  auto?: boolean;
  /** Chat asked this model beside the reply, for a second opinion. */
  second?: boolean;
}

/** What a reply keeps about being compared: who else was asked, on what ground, and when. */
export interface CompareNote {
  models?: string[];
  /** close (Auto's pick was a close call), hard (a hard request) or sample (by chance). */
  why?: string;
  at?: string;
}

/** How long the other models have to answer before the page stops waiting for them. */
export const COMPARE_WAIT_MS = 200_000;

/**
 * Where a compared reply stands: how many other answers there are, and whether more
 * may still come. Null when there is nothing to say (never compared, or nobody answered).
 */
export function compareState(note: CompareNote | null | undefined, versions: number, now: number): { arrived: number; pending: boolean } | null {
  if (!note || typeof note !== "object" || !Array.isArray(note.models) || !note.models.length) return null;
  const arrived = Math.max(0, versions - 1);
  const asked = note.at ? Date.parse(note.at) : NaN;
  const pending = arrived < note.models.length && Number.isFinite(asked) && now - asked < COMPARE_WAIT_MS;
  return arrived || pending ? { arrived, pending } : null;
}

const GROUNDS: Record<string, string> = {
  close: "Auto's pick was a close call",
  hard: "This looked like a hard one",
  sample: "Now and then Chat checks Auto's pick",
};

/** Why Chat asked other models, as the start of a sentence. */
export const compareGround = (why: string | undefined) => GROUNDS[why || ""] || "Chat checked Auto's pick";

/** Up to `count` versions to show first: the one in the chat and the newest others, in the order they were written. */
export function firstShown(versions: ReplyVersion[], count: number): string[] {
  if (!versions.length) return [];
  const shown = versions.find((v) => v.selected) ?? versions.at(-1)!;
  const others = [...versions]
    .reverse()
    .filter((v) => v.id !== shown.id)
    .slice(0, Math.max(0, count - 1));
  return versions.filter((v) => v === shown || others.includes(v)).map((v) => v.id);
}

/** The two versions to show first: the one in the chat, and the newest other one. */
export function firstPair(versions: ReplyVersion[]): [string, string] | null {
  if (versions.length < 2) return null;
  return firstShown(versions, 2) as [string, string];
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

/** Whether a version did better on a stat than every other one shown that measured it. */
export function best(label: string, value: string | undefined, others: (string | undefined)[]): boolean {
  const measured = others.filter(Boolean);
  return measured.length > 0 && measured.every((other) => better(label, value, other) === "left");
}

/** Which side did better on a stat, for marking it: lower is better except for speed. */
export function better(label: string, left: string | undefined, right: string | undefined): "left" | "right" | null {
  if (!left || !right || !["Speed", "First token", "Time"].includes(label)) return null;
  const a = Number(left.replace(/,/g, "").split(" ")[0]),
    b = Number(right.replace(/,/g, "").split(" ")[0]);
  if (!Number.isFinite(a) || !Number.isFinite(b) || a === b) return null;
  return (label === "Speed" ? a > b : a < b) ? "left" : "right";
}
