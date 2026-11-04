import type { Message } from "./types";

// Why a reply was not good; the keys are what the server stores (feedback::REASONS).
export const REASONS: [string, string][] = [
  ["wrong", "Wrong or made up"],
  ["instructions", "Didn’t follow instructions"],
  ["too_long", "Too long"],
  ["too_short", "Too short"],
  ["harmful", "Harmful or offensive"],
  ["other", "Other"],
];
export const COMMENT_LIMIT = 2000;
export const reasonLabel = (key: string) => REASONS.find(([k]) => k === key)?.[1] || key;

/** Whether the person may rate this reply: a finished reply in their own chat. */
export const ratable = (m: Message | undefined | null, shared: unknown) =>
  !shared && !!m?.id && m.role === "assistant" && ["completed", "interrupted"].includes(m.status || "");

/** Clicking the thumb that is already chosen clears it; the other one switches. */
export const nextRating = (current: number, clicked: number) => (current === clicked ? 0 : clicked);

/** The rating the thumbs-down form sends. */
export function feedbackDetails(reasons: string[], comment: string) {
  const text = (comment || "").trim().slice(0, COMMENT_LIMIT);
  return { rating: -1, reasons, ...(text ? { comment: text } : {}) };
}
