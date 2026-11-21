// The site's branding as an administrator set it in Administration › Branding: its
// name and look, the words on the home and sign-in pages, links, icons and uploaded
// images. It is laid over what the installation's site.config.js says, comes with the
// session answer (signed in or not) and is kept in this browser, so the next visit
// starts with it instead of showing the file's version for a moment first.

import { accentHex } from "./accent";
import { type Branding, defaultBranding, normalizeBranding } from "./branding";

export type Corners = "sharp" | "soft" | "round";
export interface SiteAppearance {
  siteName?: string;
  accent?: string;
  defaultTheme?: "system" | "light" | "dark";
  personalAccent?: boolean;
  corners?: Corners;
  /** What was changed of the rest, in the shape of site.config.js, plus `images` by slot. */
  branding?: Record<string, unknown>;
}

type Obj = Record<string, unknown>;
const object = (value: unknown): Obj => (value && typeof value === "object" && !Array.isArray(value) ? (value as Obj) : {});

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
  // The words are checked where they are used (normalizeBranding), like the file's.
  if (Object.keys(object(raw.branding)).length) out.branding = object(raw.branding);
  return out;
}

/**
 * The file's configuration with the administrator's settings over it, still in the
 * file's shape. A setting that is present wins, an empty text included (it hides that
 * line, as in the file); an uploaded image takes the place of the file's.
 */
export function mergedConfig(file: unknown, set: SiteAppearance): Obj {
  const f = object(file),
    b = object(set.branding),
    images = object(b.images);
  const out: Obj = { ...f };
  for (const key of ["siteSubtitle", "assistantName", "description", "links"]) if (b[key] !== undefined) out[key] = b[key];
  for (const key of ["icons", "login", "home"]) if (Object.keys(object(b[key])).length) out[key] = { ...object(f[key]), ...object(b[key]) };
  if (set.siteName) out.siteName = set.siteName;
  const look = { ...object(f.appearance) };
  if (set.accent) look.accent = set.accent;
  if (set.defaultTheme) look.defaultTheme = set.defaultTheme;
  if (set.personalAccent !== undefined) look.personalAccent = set.personalAccent;
  out.appearance = look;
  const fileLogo = f.logo === undefined ? defaultBranding.logo : object(f.logo);
  const alt = typeof b.logoAlt === "string" ? b.logoAlt : fileLogo.alt;
  if (b.hideLogo === true) out.logo = { light: "", dark: "", alt: "" };
  else if (images["logo-light"] || images["logo-dark"]) out.logo = { light: images["logo-light"] ?? "", dark: images["logo-dark"] ?? "", alt };
  else if (typeof b.logoAlt === "string") out.logo = { ...fileLogo, alt };
  if (images.favicon) out.favicon = { light: images.favicon, dark: images.favicon };
  return out;
}

/** The branding the site shows: the file's with the administrator's settings over it. */
export function withAppearance(file: unknown, set: SiteAppearance, knownIcons?: ReadonlySet<string>): Branding {
  return normalizeBranding(mergedConfig(file, set), knownIcons);
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
