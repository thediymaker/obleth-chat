"use client";

// Administration › Activity: administrative changes and account security events, by
// day, as plain sentences, with a kind filter, a search and older pages.

import { useInfiniteQuery } from "@tanstack/react-query";
import { ChevronDown, RefreshCw } from "lucide-react";
import { Fragment, useState } from "react";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { NativeSelect, Note } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { cn } from "@/lib/utils";
import { AUDIT_KINDS, auditDay, auditDetails, auditSentence, auditTime, kindInfo, type AuditEvent, type Part } from "./audit";
import { SearchForm } from "./people-tab";
import { useKept } from "./state";

export function Sentence({ parts }: { parts: Part[] }) {
  return (
    <>
      {parts.map((p, i) =>
        typeof p === "string" ? (
          <Fragment key={i}>{p}</Fragment>
        ) : (
          <strong key={i} className="font-semibold">
            {p.bold}
          </strong>
        ),
      )}
    </>
  );
}

function Details({ event }: { event: AuditEvent }) {
  const [open, setOpen] = useState(false);
  const rows = auditDetails(event);
  if (!rows.length) return null;
  return (
    <div className="mt-1">
      <button type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)} className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ChevronDown className={cn("size-3", open && "rotate-180")} /> Details
      </button>
      {open && (
        <dl className="mt-1.5 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 rounded-md bg-muted px-3 py-2 text-xs">
          {rows.map(([k, v], i) => (
            <Fragment key={i}>
              <dt className="text-muted-foreground">{k}</dt>
              <dd className="font-mono break-all">{v}</dd>
            </Fragment>
          ))}
        </dl>
      )}
    </div>
  );
}

export function ActivityList({ events }: { events: AuditEvent[] }) {
  let last = "";
  return (
    <div className="grid">
      {events.map((e, i) => {
        const heading = auditDay(e.createdAt);
        const head = heading !== last;
        last = heading;
        const k = kindInfo(e.kind);
        const Icon = k.icon;
        return (
          <Fragment key={e.id ?? i}>
            {head && <h3 className="mt-4 mb-1 text-2xs font-medium tracking-wider text-muted-foreground uppercase first:mt-0">{heading}</h3>}
            <article className="flex gap-3 border-b border-line-soft py-2.5 last:border-0">
              <span className="mt-0.5 grid size-7 shrink-0 place-items-center rounded-full bg-secondary" title={k.name}>
                <Icon className="size-3.5 text-muted-foreground" />
              </span>
              <div className="min-w-0 grow">
                <div className="flex flex-wrap items-baseline justify-between gap-x-3">
                  <p className="text-md">
                    <Sentence parts={auditSentence(e)} />
                  </p>
                  <time dateTime={e.createdAt} title={when(e.createdAt)} className="shrink-0 text-xs text-muted-foreground tabular-nums">
                    {auditTime(e.createdAt)}
                  </time>
                </div>
                <small className="text-xs text-faint">{k.name}</small>
                <Details event={e} />
              </div>
            </article>
          </Fragment>
        );
      })}
    </div>
  );
}

export function ActivityTab() {
  const [kind, setKind] = useKept("audit.kind", "");
  const [query, setQuery] = useKept("audit.query", "");
  const audit = useInfiniteQuery({
    queryKey: ["admin", "audit", kind, query],
    initialPageParam: "",
    queryFn: ({ pageParam }) =>
      api.request<{ events?: AuditEvent[]; nextCursor?: string | null }>(
        `/api/admin/audit?limit=50${kind ? `&kind=${encodeURIComponent(kind)}` : ""}${query ? `&q=${encodeURIComponent(query)}` : ""}${pageParam ? `&before=${encodeURIComponent(pageParam)}` : ""}`,
      ),
    getNextPageParam: (last) => last.nextCursor || undefined,
  });
  const events = (audit.data?.pages ?? []).flatMap((p) => p.events ?? []);
  const filtered = !!(kind || query);
  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <SearchForm id="admin-audit-query" label="Search activity" placeholder="Search people, groups, models…" value={query} onSearch={setQuery} />
        <div className="flex flex-wrap items-center gap-2">
          <label className="sr-only" htmlFor="admin-audit-kind">
            Kind of activity
          </label>
          <NativeSelect id="admin-audit-kind" value={kind} onChange={(e) => setKind(e.target.value)} className="h-8 w-auto">
            <option value="">All activity</option>
            {AUDIT_KINDS.map(([id, , name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </NativeSelect>
          <Button size="sm" variant="outline" onClick={() => audit.refetch()}>
            <RefreshCw /> Refresh
          </Button>
        </div>
      </div>
      <Note>Administrative changes and account security events. Conversation content stays private.</Note>
      <ErrorBox error={audit.error} onRetry={() => audit.refetch()} />
      {audit.isLoading ? (
        <Loading label="Loading administration…" />
      ) : events.length ? (
        <div className="rounded-lg border bg-card px-4 py-3">
          <ActivityList events={events} />
        </div>
      ) : filtered ? (
        <EmptyState
          title="No matching activity"
          text="Try another kind or search."
          action={
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                setKind("");
                setQuery("");
              }}
            >
              Clear filters
            </Button>
          }
        />
      ) : (
        !audit.error && <EmptyState title="No activity yet" text="Account and administration events will appear here." />
      )}
      {audit.hasNextPage && (
        <div className="flex justify-center">
          <Button size="sm" variant="outline" disabled={audit.isFetchingNextPage} onClick={() => audit.fetchNextPage()}>
            {audit.isFetchingNextPage ? "Loading…" : "Load older activity"}
          </Button>
        </div>
      )}
    </div>
  );
}
