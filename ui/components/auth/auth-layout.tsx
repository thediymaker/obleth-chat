"use client";

import { Brand, SiteFooter } from "@/components/brand";
import { SiteBanner } from "@/components/site-banner";
import { ThemeToggle } from "@/components/theme-toggle";
import { useSession } from "@/lib/session";

/** One quiet column, the same for every signed-out screen. */
export function AuthLayout({ children }: { children: React.ReactNode }) {
  const { banner } = useSession();
  return (
    <main id="main" className="flex min-h-dvh flex-col">
      {banner && (
        <div className="flex justify-center px-4 pt-3">
          <SiteBanner banner={banner} />
        </div>
      )}
      <header className="flex items-center justify-between px-6 py-5">
        <Brand />
        <ThemeToggle />
      </header>
      <section className="flex flex-1 items-center justify-center px-4 py-8">
        <div className="w-full max-w-sm">{children}</div>
      </section>
      <SiteFooter />
    </main>
  );
}

export function AuthIntro({ title, text }: { title: React.ReactNode; text?: React.ReactNode }) {
  return (
    <div className="mb-6">
      <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
      {text && <p className="mt-2 text-sm text-muted-foreground">{text}</p>}
    </div>
  );
}
