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
}

const HARD = ["", "simple", "moderate", "hard"];
const NEEDS: Record<string, string> = { tools: "tools", vision: "images", schema: "structured output" };
const list = (words: string[]) => (words.length < 3 ? words.join(" and ") : `${words.slice(0, -1).join(", ")} and ${words.at(-1)}`);
const topic = (name: string) => name.replace(/[_-]+/g, " ");

/** "a hard coding request that needs tools", or "" when the gateway did not say. */
export function autoRead(why: AutoWhy): string {
  const hard = HARD[Math.min(why.difficulty ?? 0, 3)] || "";
  const topics = (why.topics || []).map(topic);
  const needs = (why.needs || []).map((n) => NEEDS[n]).filter(Boolean) as string[];
  if (!hard && !topics.length && !needs.length) return "";
  const kind = [hard, topics.length ? list(topics) : ""].filter(Boolean).join(" ");
  const article = /^[aeiou]/i.test(kind) ? "an" : "a";
  return `${kind ? `${article} ${kind} request` : "a request"}${needs.length ? ` that needs ${list(needs)}` : ""}`;
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

/** A few words for the line under a reply: "hard coding · needs tools", "kept", "moved up". */
export function autoWhyShort(why: AutoWhy | null | undefined): string {
  if (!why) return "";
  const reason = why.reason || "";
  if (reason === "kept") return "kept for this chat";
  if (reason.startsWith("escalated") || reason === "harder") return "moved up";
  const hard = HARD[Math.min(why.difficulty ?? 0, 3)] || "";
  const topics = (why.topics || []).slice(0, 2).map(topic).join(", ");
  const needs = (why.needs || []).map((n) => NEEDS[n]).filter(Boolean);
  return [[hard, topics].filter(Boolean).join(" "), needs.length ? `needs ${list(needs as string[])}` : ""].filter(Boolean).join(" · ");
}

/** The whole reason, in a sentence or two, for the reply's details. */
export function autoWhyText(why: AutoWhy | null | undefined): string {
  if (!why) return "";
  const read = autoRead(why);
  const source = why.from === "header" && why.difficulty ? " (from your effort setting)" : "";
  // A chat that kept its model chose among nothing.
  const count = why.reason === "kept" ? 0 : why.candidates || 0;
  const among = count > 1 ? `, picked from ${count} models` : "";
  const first = read ? `Read as ${read}${source}${among}.` : count > 1 ? `Picked from ${count} models.` : "";
  return [MOVES[why.reason || ""] || "", first].filter(Boolean).join(" ");
}
