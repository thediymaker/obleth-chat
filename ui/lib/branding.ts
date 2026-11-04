// Public site branding from /site.config.js, which the Rust server serves from
// its branding folder so an install can change it without a rebuild. The file
// sets window.OblethChatConfig; this module checks every field and fills defaults.

export interface Link {
  label: string;
  url: string;
}
export interface Branding {
  siteName: string;
  siteNameLines: string[];
  siteSubtitle: string;
  assistantName: string;
  description: string;
  logo: { light: string; dark: string; alt: string };
  favicon: { light: string; dark: string };
  icons: { greeting: string; assistant: string; loading: string };
  login: { title: string; description: string; ssoLabel: string; ssoHint: string; help: string; helpLink: Link | null };
  links: Link[];
  home: { eyebrow: string; title: string; description: string; footer: string };
  appearance: { defaultTheme: "system" | "light" | "dark" };
}

declare global {
  interface Window {
    OblethChatConfig?: unknown;
  }
}

const defaults: Branding = {
  siteName: "Obleth Chat",
  siteNameLines: ["Obleth Chat"],
  siteSubtitle: "",
  assistantName: "Assistant",
  description: "A space to ask questions, explore ideas, and make something new.",
  logo: { light: "/assets/obleth-ink.svg", dark: "/assets/obleth.svg", alt: "" },
  favicon: { light: "/assets/obleth-ink.svg", dark: "/assets/obleth.svg" },
  icons: { greeting: "chat", assistant: "chat", loading: "chat" },
  login: {
    title: "",
    description: "",
    ssoLabel: "Continue with single sign-on",
    ssoHint: "",
    help: "Need an account? Your administrator can create one for you.",
    helpLink: null,
  },
  links: [
    { label: "Documentation", url: "https://www.obleth.com/docs/chat/introduction" },
    { label: "Obleth", url: "https://www.obleth.com" },
  ],
  home: {
    eyebrow: "Your space to explore",
    title: "What’s on your mind?",
    description: "Ask a question, explore an idea, or start something new.",
    footer: "",
  },
  appearance: { defaultTheme: "system" },
};

type Obj = Record<string, unknown>;
const object = (value: unknown): Obj => (value && typeof value === "object" && !Array.isArray(value) ? (value as Obj) : {});
const text = (value: unknown, fallback: string) => (typeof value === "string" ? value : fallback);

/** A path on this site, made absolute: the file's './assets/x.svg' works from any page. */
export function assetPath(value: unknown): string {
  if (typeof value !== "string") return "";
  const path = value.trim();
  if (!path || /[\u0000- \\]/.test(path) || path.startsWith("//") || /^[a-z][a-z\d+.-]*:/i.test(path) || /^[?#]/.test(path)) return "";
  try {
    const base = "https://branding.invalid/";
    const resolved = new URL(path, base);
    return resolved.origin === new URL(base).origin ? resolved.pathname + resolved.search : "";
  } catch {
    return "";
  }
}

function assets(value: unknown, fallback = "") {
  const source = object(value);
  const light = assetPath(source.light);
  const dark = assetPath(source.dark);
  return { light: light || dark || fallback, dark: dark || light || fallback };
}

/** Links may leave the site, but only to web or mail addresses. */
function linkUrl(value: unknown): string {
  if (typeof value !== "string") return "";
  const url = value.trim();
  if (/^(https?:\/\/|mailto:)/i.test(url)) return url;
  return assetPath(url);
}

function copyFields<T extends Record<string, string>>(source: unknown, fallback: T): T {
  const input = object(source);
  return Object.fromEntries(Object.entries(fallback).map(([key, value]) => [key, text(input[key], value)])) as T;
}

export const ICON_ROLES = ["greeting", "assistant", "loading"] as const;

export function normalizeBranding(value: unknown, knownIcons: ReadonlySet<string> = new Set()): Branding {
  const input = object(value);
  const configuredIcons = object(input.icons);
  const defaultTheme = object(input.appearance).defaultTheme;
  const nameLines = text(input.siteName, defaults.siteName)
    .split(/\r\n?|\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  const siteNameLines = nameLines.length ? nameLines : [defaults.siteName];
  const login = object(input.login);
  const helpLink = object(login.helpLink);
  const helpLabel = text(helpLink.label, "").trim();
  const helpUrl = linkUrl(helpLink.url);
  const loginCopy = copyFields(login, { title: "", description: "", ssoLabel: defaults.login.ssoLabel, ssoHint: "", help: defaults.login.help });
  return {
    siteName: siteNameLines.join(" "),
    siteNameLines,
    siteSubtitle: text(input.siteSubtitle, defaults.siteSubtitle),
    assistantName: text(input.assistantName, defaults.assistantName).trim() || defaults.assistantName,
    description: text(input.description, defaults.description),
    // A missing logo keeps the Obleth mark; blank paths give a text-only brand.
    logo: input.logo === undefined ? defaults.logo : { ...assets(input.logo), alt: text(object(input.logo).alt, "") },
    favicon: input.favicon === undefined ? defaults.favicon : assets(input.favicon, "/assets/chat.svg"),
    icons: Object.fromEntries(
      ICON_ROLES.map((role) => {
        const name = configuredIcons[role];
        return [role, name === "" || (typeof name === "string" && knownIcons.has(name)) ? name : defaults.icons[role]];
      }),
    ) as Branding["icons"],
    login: { ...loginCopy, ssoLabel: loginCopy.ssoLabel.trim() || defaults.login.ssoLabel, helpLink: helpLabel && helpUrl ? { label: helpLabel, url: helpUrl } : null },
    links: Array.isArray(input.links)
      ? input.links
          .slice(0, 8)
          .map((item) => ({ label: text(object(item).label, "").trim(), url: linkUrl(object(item).url) }))
          .filter((item) => item.label && item.url)
      : defaults.links,
    home: copyFields(input.home, defaults.home),
    appearance: { defaultTheme: defaultTheme === "light" || defaultTheme === "dark" || defaultTheme === "system" ? defaultTheme : "system" },
  };
}

export const defaultBranding = defaults;
export const pageTitle = (label: string, branding: Branding) => (label ? `${label} · ${branding.siteName}` : branding.siteName);
export const isExternal = (url: string) => /^(https?:|mailto:)/i.test(url);
