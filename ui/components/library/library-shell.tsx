"use client";

// What Library's pages share: the section tabs (Saved, Collections, Generated) and
// the page shown when the server has no knowledge feature.

import { BookOpen, Bookmark, Image as ImageIcon, MessageSquare } from "lucide-react";
import Link from "next/link";
import { usePageChrome } from "@/components/app-shell/chrome";
import { Loading, Page, PageHeader } from "@/components/app-shell/page";
import { SectionTabs } from "@/components/app-shell/section-tabs";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useCapabilities } from "@/lib/queries";

export const LIBRARY_TABS = [
  { href: "/library", label: "Saved", icon: Bookmark },
  { href: "/knowledge", label: "Collections", icon: BookOpen },
  { href: "/library/generated", label: "Generated", icon: ImageIcon },
];
export const LibraryTabs = () => <SectionTabs label="Library" tabs={LIBRARY_TABS} />;

const OFF = {
  knowledge: { icon: BookOpen, heading: "Your knowledge, connected.", title: "Knowledge is not enabled.", text: "Document collections with citations need object storage and pgvector on this server." },
  library: { icon: Bookmark, heading: "Everything worth keeping.", title: "The library is not enabled.", text: "Saved items and document collections need object storage and pgvector on this server." },
};

/** Renders `children` only when the server has knowledge; otherwise says it isn't set up. */
export function KnowledgeOnly({ page, title, children }: { page: keyof typeof OFF; title: string; children: React.ReactNode }) {
  const { data, isPending } = useCapabilities();
  if (isPending) return <Loading />;
  if (data?.features?.knowledge) return <>{children}</>;
  return <FeatureOff page={page} title={title} />;
}

function FeatureOff({ page, title }: { page: keyof typeof OFF; title: string }) {
  usePageChrome(title);
  const copy = OFF[page];
  const Icon = copy.icon;
  return (
    <Page>
      <PageHeader title={copy.heading} description="Your models. Your knowledge. Your compute." />
      <section className="flex flex-col items-center gap-3 rounded-lg border bg-card px-6 py-12 text-center">
        <Icon className="size-6 text-muted-foreground" />
        <Badge variant="outline">Not set up on this server</Badge>
        <h2 className="text-lg font-semibold">{copy.title}</h2>
        <p className="max-w-md text-sm text-muted-foreground">{copy.text}</p>
        <Button asChild>
          <Link href="/">
            <MessageSquare /> Back to chat
          </Link>
        </Button>
      </section>
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
