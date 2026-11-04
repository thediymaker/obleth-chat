// The message box holds this many characters (store::MAX_USER_MESSAGE_CHARS); a paste longer than
// PASTE_AS_FILE, or one that would pass the limit, is attached as a text file instead.
export const PROMPT_LIMIT = 12000;
export const PASTE_AS_FILE = 4000;

/** Whether pasting `text` into `box` attaches it instead, replacing any selection. */
export const pastesAsFile = (text: string, box: { value: string; selectionStart: number | null; selectionEnd: number | null }) =>
  text.length > PASTE_AS_FILE || box.value.length - ((box.selectionEnd ?? 0) - (box.selectionStart ?? 0)) + text.length > PROMPT_LIMIT;

/** How close the message is to the box's limit, once it is near. */
export const promptCount = (n: number) => (n >= PROMPT_LIMIT * 0.9 ? `${n.toLocaleString()} / ${PROMPT_LIMIT.toLocaleString()}` : "");

export const EFFORTS = ["none", "low", "medium", "high"] as const;
export const effortLabel = (value: string) => (value ? value[0]!.toUpperCase() + value.slice(1) : "");
export const EFFORT_DESCRIPTIONS: Record<string, string> = {
  none: "Ask for a direct answer. Some models still reason.",
  low: "Quick reasoning for everyday questions.",
  medium: "A balance of depth and response time.",
  high: "More room to work through complex problems.",
};

/** Token counts in a few characters: 950, 1.2k, 34k, 1.5M. */
export const tokens = (n: number) => (n < 1000 ? String(n) : n < 10000 ? `${(n / 1000).toFixed(1)}k` : n < 1e6 ? `${Math.round(n / 1000)}k` : `${(n / 1e6).toFixed(1)}M`);

/** Minutes and seconds, as the live timers show them. */
export const clock = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

// Auto mode's watchdog writes these when an unattended turn stopped; they read as a status line.
export const NUDGE = "[Auto mode] ";
