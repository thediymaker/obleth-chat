// Why Auto chose the model it did, in words. The gateway says how it read the
// request (topics, how hard it looks, what it must be able to do) and what happened
// to the conversation's model (kept, moved up, picked again); Chat keeps that with
// the reply. Older gateways say less, so every part is optional.

export interface AutoWhy {
  /** What happened to the conversation's model: fresh, kept, harder, topic, returned, forced, escalated:<signal>. */
  reason?: string;
  topics?: string[];
  /** How hard the request looked, 1 (simple) to 3 (hard). */
  difficulty?: number;
  /** Where that reading came from: classifier, heuristic, header (the person's effort setting), default. */
  from?: string;
  needs?: string[];
  /** How many models the pick was made among. */
  candidates?: number;
  /** The models that scored best after the one chosen, and how far behind (0 is a tie). */
  next?: { model: string; gap?: number }[];
}

const HARD = ["", "simple", "moderate", "hard"];
// What the gateway calls a need is a limit on the models, not a finding about the
// message: a chat with tools on sends them with every message, a greeting included,
// so "needs tools" would be wrong. It is said as what the model had to be able to do.
const ABLE: Record<string, string> = { tools: "use tools", vision: "read images", schema: "return structured output" };
const ABLE_SHORT: Record<string, string> = { tools: "tools on", vision: "image", schema: "structured output" };
const list = (words: string[]) => (words.length < 3 ? words.join(" and ") : `${words.slice(0, -1).join(", ")} and ${words.at(-1)}`);
const topic = (name: string) => name.replace(/[_-]+/g, " ");

/** "a hard coding request", or "" when the gateway did not say how it read it. */
export function autoRead(why: AutoWhy): string {
  const hard = HARD[Math.min(why.difficulty ?? 0, 3)] || "";
  const topics = (why.topics || []).map(topic);
  if (!hard && !topics.length) return "";
  const kind = [hard, topics.length ? list(topics) : ""].filter(Boolean).join(" ");
  const article = /^[aeiou]/i.test(kind) ? "an" : "a";
  return `${article} ${kind} request`;
}

/** What the model had to be able to do: "use tools and read images", or "". */
export function autoAble(why: AutoWhy): string {
  return list((why.needs || []).map((n) => ABLE[n]).filter(Boolean) as string[]);
}

// What happened to the conversation's model, as a sentence.
const MOVES: Record<string, string> = {
  kept: "Kept the model this chat was already on.",
  harder: "This message looked harder than the last, so Auto moved up.",
  topic: "The topic changed, so Auto picked again.",
  returned: "Back to an earlier topic, and to the model that served it then.",
  forced: "The model this chat was on could not take this message, so Auto picked again.",
  "escalated:regenerate": "You asked for another answer, so Auto moved up.",
  "escalated:complaint": "The last answer was not right, so Auto moved up.",
  "escalated:bad_output": "The last answer came out broken, so Auto moved up.",
  "escalated:tool_loop": "The last model was repeating itself, so Auto moved up.",
  "escalated:tool_errors": "The last model kept failing with its tools, so Auto moved up.",
};

/** A few words for the line under a reply: "hard coding · tools on", "kept", "moved up". */
export function autoWhyShort(why: AutoWhy | null | undefined): string {
  if (!why) return "";
  const reason = why.reason || "";
  if (reason === "kept") return "kept for this chat";
  if (reason.startsWith("escalated") || reason === "harder") return "moved up";
  const hard = HARD[Math.min(why.difficulty ?? 0, 3)] || "";
  const topics = (why.topics || []).slice(0, 2).map(topic).join(", ");
  const able = (why.needs || []).map((n) => ABLE_SHORT[n]).filter(Boolean);
  return [[hard, topics].filter(Boolean).join(" "), able.join(", ")].filter(Boolean).join(" · ");
}

/** The whole reason, in a sentence or two, for the reply's details. `name` gives a model's display name. */
export function autoWhyText(why: AutoWhy | null | undefined, name: (id: string) => string = (id) => id): string {
  if (!why) return "";
  const next = (why.next || []).filter((n) => n && typeof n.model === "string").map((n) => name(n.model));
  const read = autoRead(why);
  const source = why.from === "header" && why.difficulty ? " (from your effort setting)" : "";
  // A chat that kept its model chose among nothing.
  const count = why.reason === "kept" ? 0 : why.candidates || 0;
  // The models it chose among were the ones able to take the request.
  const able = why.reason === "kept" ? "" : autoAble(why);
  const models = count > 1 ? `${count} models${able ? ` that can ${able}` : ""}` : "";
  const alone = !models && able ? ` It had to be a model that can ${able}.` : "";
  const first = read ? `Read as ${read}${source}${models ? `, picked from ${models}` : ""}.${alone}` : models ? `Picked from ${models}.` : alone.trim();
  return [MOVES[why.reason || ""] || "", first, next.length ? `Next in line: ${list(next)}.` : ""].filter(Boolean).join(" ");
}
