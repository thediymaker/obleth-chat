import type { Conversation } from "./types";

/**
 * How much of the model's window the chat uses: the last request of the last reply,
 * its input and output. A reply with tools is several requests; its totals add them up.
 */
export function contextUse(c: Conversation | null | undefined, { windowSize = 0, systemPrompt = "", open = true }: { windowSize?: number; systemPrompt?: string; open?: boolean } = {}) {
  const usage = open && c ? [...(c.messages || [])].reverse().find((m) => m.role === "assistant" && m.usage)?.usage : null;
  const output = Number(usage?.completion_tokens) || 0;
  // Text runs about four bytes a token; the rest of the request is tools, instructions and page text.
  const encoder = new TextEncoder();
  const bytes = (c?.messages || []).filter((m) => m.status === "completed").reduce((n, m) => n + encoder.encode(m.content || "").length, encoder.encode(systemPrompt).length);
  const messages = Math.round(bytes / 4);
  let input = Number(usage?.last_prompt_tokens ?? usage?.prompt_tokens) || 0,
    used = input + (Number(usage?.last_completion_tokens ?? usage?.completion_tokens) || 0),
    unknown = false;
  // Replies from before the last step was recorded only have the sum, which can pass
  // the window; their size is not known.
  if (usage && usage.last_prompt_tokens == null && windowSize && used > windowSize) {
    input = used = 0;
    unknown = true;
  }
  return { input, output, used, windowSize, unknown, steps: Number(usage?.steps) || 1, share: windowSize ? Math.min(1, used / windowSize) : 0, bytes, messages, rest: Math.max(0, input - messages) };
}
export type ContextUse = ReturnType<typeof contextUse>;
