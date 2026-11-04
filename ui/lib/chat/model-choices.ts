import type { ModelInfo } from "./types";

/**
 * The models the chooser lists for a search, Auto then favorites first. With a short list
 * and no search, it starts with the short list plus Auto, the person's favorites, the
 * default and the model in use; `hidden` counts the rest, behind See all models.
 */
export function chooserModels(
  models: ModelInfo[],
  { query = "", all = false, favorites = [], current = "" }: { query?: string; all?: boolean; favorites?: string[]; current?: string } = {},
) {
  const rank = (m: ModelInfo) => (m.id === "auto" ? -2 : favorites.includes(m.id) ? -1 : 0);
  const matches = models
    .filter((m) => [m.name, m.id, m.description, ...(m.tags || [])].join(" ").toLowerCase().includes(query.toLowerCase()))
    .sort((a, b) => rank(a) - rank(b));
  const short = !query && !all && models.some((m) => m.shortList);
  const choices = short ? matches.filter((m) => m.id === "auto" || m.shortList || favorites.includes(m.id) || m.isDefault || m.id === current) : matches;
  return { choices, hidden: matches.length - choices.length };
}
