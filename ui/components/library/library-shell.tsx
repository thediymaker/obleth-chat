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
