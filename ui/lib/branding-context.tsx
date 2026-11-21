"use client";

import { createContext, useContext, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { defaultBranding, normalizeBranding, type Branding } from "@/lib/branding";
import { BRAND_ICON_NAMES } from "@/lib/icons";
import { CORNER_PROPERTIES, cornerProperties, siteAppearance, withAppearance } from "@/lib/site-appearance";

const Context = createContext<Branding>(defaultBranding);

/** The site's branding. /site.config.js loads before the app starts, so it is
 * there on the first client render; the server renders the defaults. What an
 * administrator set in Administration › Appearance lies over the file's. */
export function BrandingProvider({ children }: { children: React.ReactNode }) {
  const [branding, setBranding] = useState<Branding>(defaultBranding);
  useEffect(() => {
    const value = normalizeBranding(window.OblethChatConfig, BRAND_ICON_NAMES);
    setBranding(value);
    const favicon = window.matchMedia?.("(prefers-color-scheme: dark)").matches ? value.favicon.dark : value.favicon.light;
    if (favicon) {
      let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
      if (!link) {
        link = document.createElement("link");
        link.rel = "icon";
        document.head.appendChild(link);
      }
      link.href = favicon;
    }
  }, []);
  const set = useSyncExternalStore(siteAppearance.subscribe, siteAppearance.get, siteAppearance.server);
  const shown = useMemo(() => withAppearance(branding, set), [branding, set]);
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
