"use client";

import { createContext, useContext, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { defaultBranding, type Branding } from "@/lib/branding";
import { BRAND_ICON_NAMES } from "@/lib/icons";
import { CORNER_PROPERTIES, cornerProperties, siteAppearance, withAppearance } from "@/lib/site-appearance";

const Context = createContext<Branding>(defaultBranding);

/** The site's branding. /site.config.js loads before the app starts, so it is
 * there on the first client render; the server renders the defaults. What an
 * administrator set in Administration › Branding lies over the file's. */
export function BrandingProvider({ children }: { children: React.ReactNode }) {
  // The file's configuration as the page found it; undefined until the page is in the browser.
  const [file, setFile] = useState<{ config: unknown } | null>(null);
  useEffect(() => setFile({ config: window.OblethChatConfig }), []);
  const set = useSyncExternalStore(siteAppearance.subscribe, siteAppearance.get, siteAppearance.server);
  const shown = useMemo(() => (file ? withAppearance(file.config, set, BRAND_ICON_NAMES) : defaultBranding), [file, set]);
  // The tab's icon follows the branding, for the theme the device is in.
  const favicon = file ? (typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: dark)").matches ? shown.favicon.dark : shown.favicon.light) : "";
  useEffect(() => {
    if (!favicon) return;
    let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!link) {
      link = document.createElement("link");
      link.rel = "icon";
      document.head.appendChild(link);
    }
    link.href = favicon;
  }, [favicon]);
  // How round boxes are, on <html> for app/globals.css.
  useEffect(() => {
    const style = document.documentElement.style,
      radii = cornerProperties(set.corners);
    for (const name of CORNER_PROPERTIES) {
      if (radii[name]) style.setProperty(name, radii[name]);
      else style.removeProperty(name);
    }
  }, [set.corners]);
  return <Context.Provider value={shown}>{children}</Context.Provider>;
}

export const useBranding = () => useContext(Context);
