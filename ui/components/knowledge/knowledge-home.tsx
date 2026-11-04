"use client";

// Library → Collections: every collection the person can search. Old knowledge.js home().

import { BookOpen, ChevronRight, Plus, Shield } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { LibraryTabs, StorageMeter } from "@/components/library/library-shell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { count } from "@/lib/format";
import { cn } from "@/lib/utils";
import { type Collection, scopeLabel, size, useCollections } from "./api";
import { CollectionDialog } from "./collection-dialog";

function CollectionCard({ c }: { c: Collection }) {
  const facts = [
    count(c.documents, "document"),
    c.processing ? `indexing ${c.processing}` : c.passages ? count(c.passages, "passage") : "",
    c.problems ? `${c.problems} need${c.problems === 1 ? "s" : ""} attention` : "",
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <Link href={`/knowledge/${encodeURIComponent(c.id)}`} className="group flex min-h-40 flex-col gap-3 rounded-lg border bg-card p-4 transition-colors hover:border-line-strong hover:bg-raised">
      <div className="flex items-start justify-between gap-2">
        <span className="flex size-9 items-center justify-center rounded-lg border bg-muted">
          <BookOpen className="size-4 text-text-2" />
        </span>
        <Badge variant={c.scope === "personal" ? "outline" : "secondary"} className="max-w-[65%] truncate">
          {c.mine ? scopeLabel(c) : `${scopeLabel(c)} · read only`}
        </Badge>
      </div>
      <h2 className="text-base font-semibold">{c.name}</h2>
      <p className="line-clamp-2 grow text-sm text-muted-foreground">{c.description || (c.mine ? "Add documents to search them in chats." : `Shared by ${c.ownerName}.`)}</p>
      <div className="flex items-center justify-between text-xs text-faint">
        <span className={cn(c.problems > 0 && "text-amber")}>{facts}</span>
        <ChevronRight className="size-4 transition-transform group-hover:translate-x-0.5" />
      </div>
    </Link>
  );
}

type Tab = "all" | "mine" | "shared";

export function KnowledgeHome() {
  usePageChrome("Collections");
  const { data, error, isPending, refetch } = useCollections(true, true);
  const [tab, setTab] = useState<Tab>("all");
  const [creating, setCreating] = useState(false);
  const all = data?.collections ?? [];
  const mine = all.filter((c) => c.mine);
  const shared = all.filter((c) => !c.mine);
  const shown = tab === "mine" ? mine : tab === "shared" ? shared : all;
  const newButton = (
    <Button onClick={() => setCreating(true)}>
      <Plus /> New collection
    </Button>
  );
  const tabButton = (value: Tab, label: string, n: number) => (
    <button
      type="button"
      aria-pressed={tab === value}
      onClick={() => setTab(value)}
      className={cn("inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm text-muted-foreground hover:bg-accent hover:text-foreground", tab === value && "bg-secondary text-foreground")}
    >
      {label}
      <span className="rounded-full bg-background/60 px-1.5 text-2xs">{n}</span>
    </button>
  );
  return (
    <Page wide>
      <PageHeader title="Your knowledge, connected." description="A home for the context that makes an answer yours." actions={newButton}>
        <LibraryTabs />
      </PageHeader>
      <ErrorBox error={error} onRetry={() => refetch()} />
      {all.length > 0 && (
        <div className="mb-4 flex flex-wrap items-end justify-between gap-4">
          <div className="flex flex-wrap gap-1">
            {tabButton("all", "All collections", all.length)}
            {tabButton("mine", "Personal", mine.length)}
            {tabButton("shared", "Shared with me", shared.length)}
          </div>
          {data?.usage && <StorageMeter label="Storage" used={data.usage.used} limit={data.usage.limit} format={size} />}
        </div>
      )}
      {isPending ? (
        <Loading label="Loading collections…" />
      ) : !all.length ? (
        !error && (
          <EmptyState
            icon={BookOpen}
            title="No collections yet"
            text="Make a collection, add PDFs, Word files or notes, then pick it in a chat. Answers cite the page they came from."
            action={newButton}
          />
        )
      ) : shown.length ? (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {shown.map((c) => (
            <CollectionCard key={c.id} c={c} />
          ))}
        </div>
      ) : (
        <p className="py-6 text-sm text-muted-foreground">{tab === "shared" ? "Nothing is shared with you yet." : "You have no collections of your own yet."}</p>
      )}
      <div className="mt-6 flex items-start gap-3 rounded-lg border bg-card p-4">
        <Shield className="mt-0.5 size-5 shrink-0 text-muted-foreground" />
        <div>
          <h3 className="text-md font-semibold">Personal by default. Shared with intention.</h3>
          <p className="text-sm text-muted-foreground">Every search, citation and download checks that you can open the collection. Files live in object storage; the searchable text stays in Chat&apos;s database.</p>
        </div>
      </div>
      <CollectionDialog open={creating} onOpenChange={setCreating} />
    </Page>
  );
}
