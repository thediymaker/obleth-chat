// The two lists the message box opens while typing: "/" as the first thing in the
// box lists what can be done to the chat, and "@" anywhere lists what a message can
// point at (a workspace file, a job, a knowledge collection, a tool).

export interface Trigger {
  kind: "command" | "mention";
  /** What was typed after the sign. */
  query: string;
  /** The span of the sign and the query in the text. */
  from: number;
  to: number;
}

/** The list the text asks for with the caret where it is, if any. */
export function menuTrigger(text: string, caret: number): Trigger | null {
  const before = text.slice(0, caret);
  // A command is the whole message so far: a path such as /usr/bin in a sentence is not one.
  const command = /^\/([^\n/]{0,40})$/.exec(before);
  if (command && caret === text.length) return { kind: "command", query: command[1]!, from: 0, to: caret };
  const mention = /(?:^|[\s(])@([\w./-]{0,40})$/.exec(before);
  if (mention) return { kind: "mention", query: mention[1]!, from: caret - mention[1]!.length - 1, to: caret };
  return null;
}

export interface MenuItem {
  id: string;
  /** The heading it is listed under. */
  group: string;
  label: string;
  /** For a command: the word typed after the slash. */
  slash?: string;
  /** A few words at the right: a state, a kind. */
  note?: string;
  /** Other words it can be found by. */
  keywords?: string;
  /** Shown in the monospace face: a path. */
  mono?: boolean;
}

const words = (text: string) => text.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);

/**
 * The items a query finds, the best first: every word of the query must start a word
 * of the item (or, for a path, appear in it). An item whose slash word or label
 * begins with the query comes before one that only holds it, and the items of a group
 * stay together.
 */
export function matchItems<T extends MenuItem>(items: T[], query: string, limit = 40, each = 6): T[] {
  const q = query.trim().toLowerCase();
  // Before a query, a few of each group, so a long one (files, models) hides no other.
  if (!q) {
    const seen = new Map<string, number>();
    return items.filter((item) => {
      const n = (seen.get(item.group) ?? 0) + 1;
      seen.set(item.group, n);
      return n <= each;
    });
  }
  const asked = words(q);
  const scored: [number, number, T][] = [];
  items.forEach((item, index) => {
    const label = item.label.toLowerCase(),
      slash = (item.slash ?? "").toLowerCase();
    const hay = words(`${slash} ${label} ${item.keywords ?? ""}`);
    const all = asked.every((a) => hay.some((w) => w.startsWith(a)));
    const inside = item.mono && label.includes(q);
    if (!all && !inside) return;
    const score = slash === q || label === q ? 0 : slash.startsWith(q) || label.startsWith(q) ? 1 : (item.mono && label.split("/").at(-1)!.startsWith(q)) ? 2 : all ? 3 : 4;
    scored.push([score, index, item]);
  });
  const found = scored
    .sort((a, b) => a[0] - b[0] || a[1] - b[1])
    .slice(0, limit)
    .map(([, , item]) => item);
  // Each heading once: a group sits where its best item fell, with its others after it.
  const groups = [...new Set(found.map((item) => item.group))];
  return groups.flatMap((group) => found.filter((item) => item.group === group));
}

/** A label cut around the first place the query appears, to mark it: [before, match, after]. */
export function marked(label: string, query: string): [string, string, string] {
  const q = query.trim();
  const at = q ? label.toLowerCase().indexOf(q.toLowerCase()) : -1;
  return at < 0 ? [label, "", ""] : [label.slice(0, at), label.slice(at, at + q.length), label.slice(at + q.length)];
}

/** The text with the trigger's span replaced, and where the caret goes after it. */
export function replaceTrigger(text: string, trigger: Pick<Trigger, "from" | "to">, insert: string): { text: string; caret: number } {
  const after = text.slice(trigger.to);
  const spaced = insert && !/^\s/.test(after) ? `${insert} ` : insert;
  return { text: text.slice(0, trigger.from) + spaced + after, caret: trigger.from + spaced.length };
}

/** A workspace path as a message names it. */
export const fileMention = (path: string) => `\`${path}\``;
