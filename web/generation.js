export const generationKeys = ['systemPrompt', 'temperature', 'topP', 'maxTokens', 'reasoningEffort'];
const fallback = { systemPrompt: '', temperature: 0.7, topP: 1, maxTokens: 0, reasoningEffort: 'medium' };

// Missing personal keys inherit the published model policy. Synthesized form
// defaults from the preferences API are deliberately not treated as overrides.
export function generationValues(model, personal = {}, conversation = {}) {
  const defaults = { ...fallback, ...(model?.defaults || {}) };
  return Object.fromEntries(generationKeys.map(key => {
    const locked = model?.lockedParameters?.includes(key) || (Array.isArray(model?.allowedOverrides) && !model.allowedOverrides.includes(key));
    return [key, locked ? defaults[key] : conversation[key] ?? personal[key] ?? defaults[key]];
  }));
}
