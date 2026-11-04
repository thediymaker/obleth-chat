"use client";

import Link from "next/link";
import { useBranding } from "@/lib/branding-context";
import { isExternal } from "@/lib/branding";
import { cn } from "@/lib/utils";

/** The site logo, one image per theme. */
export function BrandLogo({ className, imageClassName = "size-full" }: { className?: string; imageClassName?: string }) {
  const { logo } = useBranding();
  if (!logo.light && !logo.dark) return null;
  return (
    <span className={cn("relative inline-flex size-7 shrink-0", className)}>
      {/* eslint-disable-next-line @next/next/no-img-element -- branding is served at run time */}
      <img src={logo.light} alt={logo.alt} className={cn(imageClassName, "object-contain dark:hidden")} />
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={logo.dark} alt={logo.alt} className={cn(imageClassName, "hidden object-contain dark:block")} />
    </span>
  );
}

/** Logo and site name, linking home. */
export function Brand({ className }: { className?: string }) {
  const branding = useBranding();
  return (
    <Link href="/" aria-label={`${branding.siteName} home`} className={cn("flex min-w-0 items-center gap-2.5", className)}>
      {/* 32px tall, up to 72px wide. */}
      <BrandLogo className="h-8 w-auto max-w-[72px] items-center" imageClassName="h-8 w-auto max-w-[72px]" />
      <span className="min-w-0">
        <span className="block text-base leading-[1.15] font-semibold tracking-[-0.2px]">
          {branding.siteNameLines.map((line, i) => (
            <span key={i} className="block truncate">
              {line}
            </span>
          ))}
        </span>
        {branding.siteSubtitle && <span className="block truncate text-2xs font-normal tracking-[.08em] text-muted-foreground uppercase">{branding.siteSubtitle}</span>}
      </span>
    </Link>
  );
}

/** Plain configured text, keeping intentional line breaks. */
export function Copy({ text }: { text: string }) {
  const lines = text.split(/\r\n?|\n/);
  return (
    <>
      {lines.map((line, i) => (
        <span key={i}>
          {i > 0 && <br />}
          {line}
        </span>
      ))}
    </>
  );
}

export function ExternalAwareLink({ url, children, className }: { url: string; children: React.ReactNode; className?: string }) {
  return (
    <a href={url} className={className} {...(isExternal(url) ? { target: "_blank", rel: "noopener noreferrer" } : {})}>
      {children}
    </a>
  );
}

export function SiteFooter() {
  const branding = useBranding();
  return (
    <footer className="flex flex-wrap items-center justify-center gap-x-4 gap-y-1 px-4 py-6 text-xs text-muted-foreground">
      <span>
        © {new Date().getFullYear()} {branding.siteName}
      </span>
      {branding.links.length > 0 && (
        <nav aria-label="Site links" className="flex gap-4">
          {branding.links.map((link) => (
            <ExternalAwareLink key={link.url} url={link.url} className="hover:text-foreground">
              {link.label}
            </ExternalAwareLink>
          ))}
        </nav>
      )}
    </footer>
  );
}
