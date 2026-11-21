// The Build view's two tools over the preview. Versions: every checkpoint the chat
// made, oldest first, so the person can step the project back and forth. Point and
// say: the element the person clicked in the preview, checked and put into words
// for the message box.

import type { Message } from "./types";

export interface Version {
  sha: string;
  /** "Checkpoint 3". */
  label: string;
  /** "2 files changed, 14 insertions(+)", or "". */
  stat: string;
  /** When the reply that made it was written. */
  at: string;
}

/** Every checkpoint in the chat, oldest first, once each. */
export function versionsOf(messages: Message[] | undefined): Version[] {
  const seen = new Set<string>();
  const out: Version[] = [];
  for (const m of messages || []) {
    if (m.role !== "assistant") continue;
    for (const e of m.activity || []) {
      const sha = typeof e.sha === "string" ? e.sha : "";
      if ((e.kind || e.type) !== "checkpoint" || !/^[0-9a-f]{7,64}$/.test(sha) || seen.has(sha)) continue;
      seen.add(sha);
      out.push({ sha, label: String(e.summary || `Checkpoint ${out.length + 1}`), stat: String(e.stat || ""), at: m.createdAt || "" });
    }
  }
  return out;
}

/** Which version the project is at: the one restored to, or the newest. -1 with none. */
export const versionAt = (versions: Version[], restored: string) => {
  const at = restored ? versions.findIndex((v) => v.sha === restored) : -1;
  return at >= 0 ? at : versions.length - 1;
};

export interface Picked {
  tag: string;
  selector: string;
  text: string;
  path: string;
  width: number;
  height: number;
}

const clip = (value: unknown, most: number) => {
  const text = typeof value === "string" ? value.replace(/[\u0000-\u001f\u007f`]+/g, " ").replace(/\s+/g, " ").trim() : "";
  return text.length > most ? `${text.slice(0, most - 1)}…` : text;
};

/**
 * The element a preview says was clicked, as far as it can be trusted: the preview is
 * the person's own app, but what it sends is still only text, clipped, with nothing
 * that could break out of the code spans it is quoted in.
 */
export function pickedElement(data: unknown): Picked | null {
  const el = (data as { element?: Record<string, unknown> } | null)?.element;
  if (!el || typeof el !== "object") return null;
  const tag = clip(el.tag, 24).toLowerCase();
  const selector = clip(el.selector, 300);
  if (!/^[a-z][a-z0-9-]*$/.test(tag) || !selector) return null;
  const size = (value: unknown) => (typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.min(100_000, Math.round(value))) : 0);
  return { tag, selector, text: clip(el.text, 140), path: clip(el.path, 200), width: size(el.width), height: size(el.height) };
}

/** What goes into the message box: the element, then room for what to do with it. */
export function pickedText(el: Picked): string {
  const where = el.path && el.path !== "/" ? ` on \`${el.path}\`` : "";
  return `In the preview${where}, the \`${el.tag}\`${el.text ? ` “${el.text}”` : ""} (\`${el.selector}\`): `;
}

/** The message box after a pick: the element first, then whatever was already typed. */
export const withPicked = (draft: string, el: Picked) => {
  const said = draft.trim();
  return said ? `${pickedText(el)}${said}` : pickedText(el);
};
