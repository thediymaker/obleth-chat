"use client";

// What Library's pages share: the section tabs (Saved, Collections, Generated) and
// the page shown when the server has no knowledge feature.

import { BookOpen, Bookmark, Image as ImageIcon } from "lucide-react";
import { usePageChrome } from "@/components/app-shell/chrome";
import { FeatureOff, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { SectionTabs } from "@/components/app-shell/section-tabs";
import { useCapabilities } from "@/lib/queries";

export const LIBRARY_TABS = [
  { href: "/library", label: "Saved", icon: Bookmark },
  { href: "/knowledge", label: "Collections", icon: BookOpen },
  { href: "/library/generated", label: "Generated", icon: ImageIcon },
];
export const LibraryTabs = () => <SectionTabs label="Library" tabs={LIBRARY_TABS} />;

const OFF = {
  knowledge: { icon: BookOpen, what: "Collections", plural: true },
  library: { icon: Bookmark, what: "The library", plural: false },
};

/** Renders `children` only when the server has knowledge; otherwise says it is off. */
export function KnowledgeOnly({ page, title, children }: { page: keyof typeof OFF; title: string; children: React.ReactNode }) {
  const { data, isPending } = useCapabilities();
  if (isPending) return <Loading />;
  if (data?.features?.knowledge) return <>{children}</>;
  return <LibraryOff page={page} title={title} />;
}

function LibraryOff({ page, title }: { page: keyof typeof OFF; title: string }) {
  usePageChrome(title);
  return (
    <Page>
      <PageHeader title="Library">
        <LibraryTabs />
      </PageHeader>
      <FeatureOff {...OFF[page]} />
    </Page>
  );
}

/** A storage meter: used of limit. */
export function StorageMeter({ label, used, limit, format }: { label: string; used: number; limit: number; format: (n: number) => string }) {
  const share = limit ? Math.min(100, (100 * used) / limit) : 0;
  return (
    <div className="grid min-w-52 gap-1.5">
      <div className="flex justify-between gap-4 text-xs text-muted-foreground">
        <span>{label}</span>
        <span>
          {format(used)} of {format(limit)}
        </span>
      </div>
      <div role="progressbar" aria-label="Storage used" aria-valuemin={0} aria-valuemax={limit} aria-valuenow={Math.min(used, limit)} className="h-1.5 overflow-hidden rounded-full bg-secondary">
        <div className={share > 90 ? "h-full bg-amber" : "h-full bg-foreground/70"} style={{ width: `${share}%` }} />
      </div>
    </div>
  );
}
