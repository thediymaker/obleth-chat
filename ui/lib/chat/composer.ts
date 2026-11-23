// The message box holds this many characters (store::MAX_USER_MESSAGE_CHARS); a paste longer than
// PASTE_AS_FILE, or one that would pass the limit, is attached as a text file instead.
export const PROMPT_LIMIT = 12000;
export const PASTE_AS_FILE = 4000;

/** Whether pasting `text` into `box` attaches it instead, replacing any selection. */
export const pastesAsFile = (text: string, box: { value: string; selectionStart: number | null; selectionEnd: number | null }) =>
  text.length > PASTE_AS_FILE || box.value.length - ((box.selectionEnd ?? 0) - (box.selectionStart ?? 0)) + text.length > PROMPT_LIMIT;

interface Clipboard {
  files?: ArrayLike<File> | null;
  items?: ArrayLike<{ kind: string; getAsFile(): File | null }> | null;
  getData(type: string): string;
}

/** The files a paste or a drop carries. Some browsers list a copied picture only under
 *  `items`, so both lists are read; empty files are left out. */
export function clipboardFiles(data?: Clipboard | null): File[] {
  if (!data) return [];
  const found = [...Array.from(data.files || [])];
  for (const item of Array.from(data.items || [])) {
    const file = item.kind === "file" ? item.getAsFile() : null;
    if (file && !found.some((f) => f.name === file.name && f.size === file.size && f.type === file.type)) found.push(file);
  }
  return found.filter((file) => file.size > 0);
}

/** Whether a paste is a copied picture and nothing else. "Copy image" in a browser, and
 *  some screenshot tools, put the picture's address or an <img> tag beside it as text;
 *  that paste is the picture. Text copied from a document or a spreadsheet can carry a
 *  picture of itself, and that paste stays text. */
export function pastesAsPicture(data?: Clipboard | null): boolean {
  const files = clipboardFiles(data);
  if (!data || !files.length || !files.every((file) => file.type.startsWith("image/"))) return false;
  const text = data.getData("text/plain").trim();
  if (!text) return true;
  const html = data
    .getData("text/html")
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<\/?(?:html|head|body|meta)\b[^>]*>/gi, "")
    .trim();
  if (/^<img\b[^>]*>$/i.test(html)) return true;
  // One line that is an address or a path to a file, as a copied picture's own text is.
  return !/\s/.test(text) && /^(?:https?:\/\/|file:\/\/|data:image\/|blob:|\/|[A-Za-z]:\\)/.test(text);
}

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

/** Minutes and seconds, as the live timers show them. */
export const clock = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

// Auto mode's watchdog writes these when an unattended turn stopped; they read as a status line.
export const NUDGE = "[Auto mode] ";
