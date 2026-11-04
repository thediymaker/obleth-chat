// Generation parameters: the model's published policy, the person's own defaults and
// the conversation's settings, in that order, with the model's locks winning.

import type { ModelInfo } from "./types";

export const generationKeys = ["systemPrompt", "temperature", "topP", "maxTokens", "reasoningEffort"] as const;
export type GenerationKey = (typeof generationKeys)[number];
export interface Generation {
  systemPrompt: string;
  temperature: number;
  topP: number;
  maxTokens: number;
  reasoningEffort: string;
}
const fallback: Generation = { systemPrompt: "", temperature: 0.7, topP: 1, maxTokens: 0, reasoningEffort: "medium" };

/** Whether the model's policy keeps this parameter at its default. */
export function isLocked(model: ModelInfo | undefined, key: string): boolean {
  return !!model?.lockedParameters?.includes(key) || (Array.isArray(model?.allowedOverrides) && !model.allowedOverrides.includes(key));
}

// Missing personal keys inherit the published model policy. Synthesized form
// defaults from the preferences API are deliberately not treated as overrides.
export function generationValues(model?: ModelInfo, personal: Record<string, unknown> = {}, conversation: Record<string, unknown> = {}): Generation {
  const defaults = { ...fallback, ...(model?.defaults || {}) } as Record<string, unknown>;
  return Object.fromEntries(
    generationKeys.map((key) => [key, isLocked(model, key) ? defaults[key] : (conversation[key] ?? personal[key] ?? defaults[key])]),
  ) as unknown as Generation;
}
