// The accent: the one color that means something is alive (a reply being written,
// a step running, a chat still working). A site sets it in site.config.js and a
// person may pick their own; either way it has to stay readable on both themes,
// so each theme gets a shade of it that passes on that theme's ground.

/** The grounds the accent is read against, as app/globals.css sets them. */
export const GROUND = { dark: "#09090b", light: "#fafafa" } as const;
export const DEFAULT_ACCENT = "#d7f25a";

/** Accents offered in Settings. The first is the default. */
export const ACCENTS: { name: string; value: string }[] = [
  { name: "Chartreuse", value: DEFAULT_ACCENT },
  { name: "Teal", value: "#5fd3bc" },
  { name: "Blue", value: "#7cb7ff" },
  { name: "Orange", value: "#ff9d5c" },
  { name: "Gold", value: "#f2c14e" },
  { name: "Rose", value: "#f08ca8" },
];

const HEX = /^#[0-9a-f]{6}$/i;

/** "#rrggbb" in lower case, or "" for anything else. "#rgb" is widened. */
export function accentHex(value: unknown): string {
  if (typeof value !== "string") return "";
  const text = value.trim().toLowerCase();
  if (HEX.test(text)) return text;
  if (/^#[0-9a-f]{3}$/.test(text)) return `#${[...text.slice(1)].map((c) => c + c).join("")}`;
  return "";
}

type Rgb = [number, number, number];
const rgb = (hex: string): Rgb => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as Rgb;
const hex = (color: Rgb) => `#${color.map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("")}`;

function luminance(color: string) {
  const [r, g, b] = rgb(color).map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  }) as Rgb;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast between two "#rrggbb" colors, 1 to 21. */
export function contrast(a: string, b: string) {
  const x = luminance(a),
    y = luminance(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

export interface Shade {
  /** The accent as shown on this ground: the color itself, or the nearest shade that reads. */
  color: string;
  /** Text on a fill of that color. */
  foreground: string;
  /** Whether the color had to move to stay readable. */
  adjusted: boolean;
}

/** The accent on a ground: moved toward black or white until text in it passes 4.5 to 1. */
export function shadeFor(accent: string, ground: string): Shade {
  const from = rgb(accent);
  const toward: Rgb = luminance(ground) > 0.5 ? [0, 0, 0] : [255, 255, 255];
  let color = accent;
  for (let k = 0.02; contrast(color, ground) < 4.5 && k < 1; k += 0.02) color = hex(from.map((v, i) => v + (toward[i]! - v) * k) as Rgb);
  return { color, foreground: contrast(color, "#0b0b0c") >= contrast(color, "#ffffff") ? "#0b0b0c" : "#ffffff", adjusted: color !== accent };
}

/** The custom properties that carry an accent to app/globals.css, or none for the default. */
export function accentProperties(accent: string): Record<string, string> {
  const value = accentHex(accent);
  if (!value || value === DEFAULT_ACCENT) return {};
  const dark = shadeFor(value, GROUND.dark),
    light = shadeFor(value, GROUND.light);
  return {
    "--signal-dark": dark.color,
    "--signal-dark-foreground": dark.foreground,
    "--signal-light": light.color,
    "--signal-light-foreground": light.foreground,
  };
}

export const ACCENT_PROPERTIES = ["--signal-dark", "--signal-dark-foreground", "--signal-light", "--signal-light-foreground"] as const;
