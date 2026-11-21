// The site's look as an administrator set it in Administration › Appearance, laid
// over what the installation's site.config.js says. It comes with the session answer
// (signed in or not) and is kept in this browser, so the next visit starts with it
// instead of showing the file's look for a moment first.

import { accentHex } from "./accent";
import type { Branding } from "./branding";

export type Corners = "sharp" | "soft" | "round";
export interface SiteAppearance {
  siteName?: string;
  accent?: string;
  defaultTheme?: "system" | "light" | "dark";
  personalAccent?: boolean;
  corners?: Corners;
}

const THEMES = ["system", "light", "dark"];
export const CORNER_NAMES: Corners[] = ["sharp", "soft", "round"];

/** Only the settings that are set and make sense; anything else is left to the file. */
export function readAppearance(input: unknown): SiteAppearance {
  const raw = input && typeof input === "object" && !Array.isArray(input) ? (input as Record<string, unknown>) : {};
  const out: SiteAppearance = {};
  if (typeof raw.siteName === "string" && raw.siteName.trim()) out.siteName = raw.siteName.trim().slice(0, 60);
  if (accentHex(raw.accent)) out.accent = accentHex(raw.accent);
  if (THEMES.includes(raw.defaultTheme as string)) out.defaultTheme = raw.defaultTheme as SiteAppearance["defaultTheme"];
  if (typeof raw.personalAccent === "boolean") out.personalAccent = raw.personalAccent;
  if (CORNER_NAMES.includes(raw.corners as Corners)) out.corners = raw.corners as Corners;
  return out;
}

/** The file's branding with the administrator's settings over it. */
export function withAppearance(branding: Branding, set: SiteAppearance): Branding {
  if (!Object.keys(set).length) return branding;
  return {
    ...branding,
    ...(set.siteName ? { siteName: set.siteName, siteNameLines: [set.siteName] } : {}),
    appearance: {
      defaultTheme: set.defaultTheme ?? branding.appearance.defaultTheme,
      accent: set.accent ?? branding.appearance.accent,
      personalAccent: set.personalAccent ?? branding.appearance.personalAccent,
    },
  };
}

/** How round boxes are: the custom properties app/globals.css reads for its radii. "soft" is Chat's own. */
export const CORNER_PROPERTIES = ["--corner-sm", "--corner-md", "--corner-lg", "--corner-xl"] as const;
export function cornerProperties(corners: Corners | undefined): Record<string, string> {
  if (corners === "sharp") return { "--corner-sm": "1px", "--corner-md": "2px", "--corner-lg": "3px", "--corner-xl": "4px" };
  if (corners === "round") return { "--corner-sm": "6px", "--corner-md": "10px", "--corner-lg": "14px", "--corner-xl": "20px" };
  return {};
}

const KEY = "obleth.site-appearance";
const EMPTY: SiteAppearance = {};
let current: SiteAppearance | null = null;
const listeners = new Set<() => void>();

function load(): SiteAppearance {
  try {
    return readAppearance(JSON.parse(window.localStorage.getItem(KEY) || "{}"));
  } catch {
    return EMPTY;
  }
}

export const siteAppearance = {
  /** What the server last said, or what this browser kept from the visit before. */
  get(): SiteAppearance {
    if (typeof window === "undefined") return EMPTY;
    return (current ??= load());
  },
  server: (): SiteAppearance => EMPTY,
  set(input: unknown) {
    const next = readAppearance(input);
    if (JSON.stringify(next) === JSON.stringify(siteAppearance.get())) return;
    current = next;
    try {
      if (Object.keys(next).length) window.localStorage.setItem(KEY, JSON.stringify(next));
      else window.localStorage.removeItem(KEY);
    } catch {
      // Without storage it lasts until the page closes, and comes again with the next session answer.
    }
    listeners.forEach((listener) => listener());
  },
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
  /** For tests: forget what was kept. */
  reset() {
    current = null;
  },
};
