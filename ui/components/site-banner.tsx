"use client";

import { Info, X } from "lucide-react";
import { useEffect, useState } from "react";
import { ExternalAwareLink } from "@/components/brand";
import type { Banner } from "@/lib/session";
import { cn } from "@/lib/utils";

// The banner an administrator sets in Administration › System. One that people may
// close stays closed in that browser until its text or link changes.
const STORAGE_KEY = "obleth.banner-closed";
const signature = (banner: Banner) => `${banner.text}\n${(banner.link as { url?: string } | undefined)?.url || ""}`;

export function SiteBanner({ banner, className }: { banner: Banner | null; className?: string }) {
  const [closed, setClosed] = useState<string | null>(null);
  useEffect(() => {
    try {
      setClosed(localStorage.getItem(STORAGE_KEY));
    } catch {
      // Closing still works for this page.
    }
  }, []);
  if (!banner?.text || (banner.dismissible && closed === signature(banner))) return null;
  const link = banner.link as { url: string; label: string } | undefined;
  const close = () => {
    const value = signature(banner);
    setClosed(value);
    try {
      localStorage.setItem(STORAGE_KEY, value);
    } catch {
      // Closed for this page only.
    }
  };
  return (
    <div
      className={cn(
        "flex min-w-0 items-center gap-2 rounded-md border px-2.5 py-1 text-xs",
        banner.tone === "warning" ? "border-amber/40 bg-amber/10 text-amber" : "border-blue/30 bg-blue/10 text-blue",
        className,
      )}
    >
      <Info className="size-3.5 shrink-0" />
      <span className="truncate" title={banner.text}>
        {banner.text}
      </span>
      {link && (
        <ExternalAwareLink url={link.url} className="shrink-0 underline underline-offset-2">
          {link.label}
        </ExternalAwareLink>
      )}
      {banner.dismissible && (
        <button type="button" onClick={close} aria-label="Close the banner" className="shrink-0 opacity-70 hover:opacity-100">
          <X className="size-3.5" />
        </button>
      )}
    </div>
  );
}
