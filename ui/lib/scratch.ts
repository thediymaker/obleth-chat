// The scratch pad's rules that need no drawing: its shortcut, the project a page
// belongs to, how a jotting becomes a board card, and the draft each browser keeps
// of what is typed but not saved yet (api/src/scratch.rs keeps the saved ones).

/** Opens the scratch pad from elsewhere (the ⌘K menu); the app shell listens. */
export const OPEN_EVENT = "obleth:scratch";
/** The longest a jotting may be, as the API keeps it. */
export const MAX_TEXT = 4000;
/** The longest a card's title may be (api/src/board/mod.rs). */
const CARD_TITLE = 200;

/**
 * ⌘⇧X (Ctrl+Shift+X): free in Chrome, Edge and Safari; Firefox uses it only to flip
 * the direction of text in a text box. ⌘K and ⌘⇧O are the app's; ⌘⇧S, ⌘⇧P, ⌘⇧J,
 * ⌘⇧N and ⌘⇧Y open things in browsers.
 */
export const SHORTCUT = { label: "⇧⌘X", spoken: "⌘ / Ctrl Shift X" } as const;
export const isShortcut = (e: Pick<KeyboardEvent, "metaKey" | "ctrlKey" | "shiftKey" | "altKey" | "key">) =>
  (e.metaKey || e.ctrlKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === "x";

/** The project a page is about (any /projects/<id>/… page) or the chat it shows; the chat's project is looked up. */
export function pageOf(path: string | null | undefined): { project: string | null; chat: string | null } {
  const [, first, id] = String(path || "").split("/");
  const decoded = id ? safeDecode(id) : "";
  if (first === "projects" && decoded) return { project: decoded, chat: null };
  if (first === "chat" && decoded) return { project: null, chat: decoded };
  return { project: null, chat: null };
}
function safeDecode(s: string) {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** A card from a jotting: its first line is the title, the rest the notes. A first line too long for a title is cut, and kept whole in the notes. */
export function cardOf(text: string): { title: string; notes: string } {
  const trimmed = text.trim();
  const at = trimmed.indexOf("\n");
  const first = (at < 0 ? trimmed : trimmed.slice(0, at)).trim();
  const rest = at < 0 ? "" : trimmed.slice(at + 1).trim();
  const chars = [...first];
  if (chars.length <= CARD_TITLE) return { title: first, notes: rest };
  return { title: `${chars.slice(0, CARD_TITLE - 1).join("").trimEnd()}…`, notes: trimmed };
}

/** A short form of a jotting for labels: its first line, cut at 40 characters. */
export function preview(text: string) {
  const first = text.trim().split("\n")[0]!.trim();
  const chars = [...first];
  return chars.length > 40 ? `${chars.slice(0, 39).join("").trimEnd()}…` : first;
}

// The draft: per browser and per person, so someone else signing in here does not see it.
const draftKey = (user: string) => `obleth.scratch-draft.${user}`;
export function readDraft(user: string): string {
  try {
    return window.localStorage.getItem(draftKey(user)) || "";
  } catch {
    return "";
  }
}
export function writeDraft(user: string, text: string) {
  try {
    if (text) window.localStorage.setItem(draftKey(user), text);
    else window.localStorage.removeItem(draftKey(user));
  } catch {
    // Storage full or blocked: the draft lasts until the page closes.
  }
}
