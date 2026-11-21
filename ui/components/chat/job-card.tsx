"use client";

// Cluster jobs as cards in a reply. A step that looked at a job, or submitted one,
// leaves the job's facts; the card shows them and, while the job is still going,
// keeps up with the cluster: waiting, then running with how far through its time
// limit it is, then how it ended.

import { ChevronRight } from "lucide-react";
import Link from "next/link";
import { useEffect, useState } from "react";
import { useClusterQueues } from "@/components/clusters/status";
import { followed, jobActive, jobCards, jobView, type Job, type JobRef, type JobTone } from "@/lib/chat/job-card";
import type { Message } from "@/lib/chat/types";
import { ago } from "@/lib/format";
import { cn } from "@/lib/utils";

// The accent is for a job that runs; a waiting job is drawn, not lit.
const DOT: Record<JobTone, string> = { running: "bg-signal", pending: "border border-dashed border-focus-border", done: "bg-focus-border", failed: "bg-destructive", ended: "bg-line-strong" };
const STATE: Record<JobTone, string> = { running: "text-foreground", pending: "text-text-2", done: "text-text-2", failed: "text-destructive-foreground", ended: "text-muted-foreground" };

function JobCard({ item, now }: { item: JobRef; now: number }) {
  const { job } = item;
  const view = jobView(job, now);
  return (
    <article className="rounded-lg border bg-card px-4 py-3" data-job-card={view.tone} aria-label={`Job ${job.name || job.id} on ${item.cluster}: ${view.state}`}>
      <div className="flex items-center gap-2 text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase">
        <span className="relative inline-flex size-2 shrink-0" aria-hidden="true">
          {view.tone === "running" && <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />}
          <span className={cn("absolute inset-0 rounded-full", DOT[view.tone])} />
        </span>
        <span className="min-w-0 flex-1 truncate">Job · {item.cluster}</span>
        <span className={cn("shrink-0 tracking-normal normal-case", STATE[view.tone])}>{view.state}</span>
      </div>
      <div className="mt-2 flex items-baseline gap-2">
        <strong className="min-w-0 truncate text-base leading-[normal] font-medium">{job.name || `Job ${job.id}`}</strong>
        <span className="shrink-0 font-mono text-xs text-muted-foreground">{job.id}</span>
      </div>
      {(view.tone === "running" || view.tone === "pending") && (
        <div
          className={cn("mt-2.5 h-1.5 overflow-hidden rounded-full", view.tone === "pending" ? "border border-dashed border-line-strong" : "bg-line-strong")}
          {...(view.share != null ? { role: "meter", "aria-label": "Time used", "aria-valuemin": 0, "aria-valuemax": 100, "aria-valuenow": Math.round(view.share * 100) } : { "aria-hidden": true })}
        >
          {view.tone === "running" && <div className={cn("h-full rounded-full bg-signal", view.share == null && "signal-breathe w-full opacity-40")} style={view.share != null ? { width: `${Math.max(2, view.share * 100)}%` } : undefined} />}
        </div>
      )}
      <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm leading-normal text-muted-foreground">
        {view.line && <span className="font-mono text-xs text-text-2 tabular-nums">{view.line}</span>}
        {view.facts.map((fact) => (
          <span key={fact}>{fact}</span>
        ))}
        {job.submitted ? <span>submitted {ago(job.submitted * 1000, now)}</span> : null}
        <Link href="/clusters" className="ml-auto inline-flex shrink-0 items-center gap-0.5 hover:text-foreground">
          Clusters <ChevronRight className="size-3.5" />
        </Link>
      </div>
    </article>
  );
}

function JobList({ cluster, jobs, now }: { cluster: string; jobs: Job[]; now: number }) {
  const shown = jobs.slice(0, 6);
  return (
    <article className="rounded-lg border bg-card px-4 py-3" data-job-list aria-label={`Your jobs on ${cluster}`}>
      <div className="flex items-center gap-2 text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase">
        <span className="min-w-0 flex-1 truncate">Your jobs · {cluster}</span>
        <Link href="/clusters" className="inline-flex shrink-0 items-center gap-0.5 tracking-normal normal-case hover:text-foreground">
          Clusters <ChevronRight className="size-3.5" />
        </Link>
      </div>
      <ul className="mt-2 grid gap-1.5">
        {shown.map((job) => {
          const view = jobView(job, now);
          return (
            <li key={String(job.id)} className="flex items-center gap-2.5 text-md">
              <span className={cn("size-2 shrink-0 rounded-full", DOT[view.tone])} aria-hidden="true" />
              <span className="min-w-0 flex-1 truncate">{job.name || `Job ${job.id}`}</span>
              <span className="shrink-0 font-mono text-xs text-muted-foreground">{job.id}</span>
              <span className="w-40 shrink-0 truncate text-right font-mono text-xs text-text-2 tabular-nums max-[560px]:hidden">{view.line}</span>
              <span className={cn("w-20 shrink-0 text-right text-sm", STATE[view.tone])}>{view.state}</span>
            </li>
          );
        })}
      </ul>
      {jobs.length > shown.length && <p className="mt-2 text-sm text-muted-foreground">and {jobs.length - shown.length} more</p>}
    </article>
  );
}

/** The jobs this reply looked at or started, above its answer. Nothing for a reply without any. */
export function JobCards({ m }: { m: Message }) {
  const found = jobCards(m.activity);
  const active = found.jobs.some((item) => jobActive(item.job)) || !!found.list?.jobs.some(jobActive);
  // The cluster is only asked while one of these jobs may still change.
  const queues = useClusterQueues(active);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  if (!found.jobs.length && !found.list) return null;
  const current = (connectionId: string) => queues.find((q) => q.cluster.id === connectionId && q.state === "connected");
  const jobs = found.jobs.map((item) => ({ ...item, job: followed(item.job, current(item.connectionId)) }));
  // A list is worth a card of its own only for the jobs no card already shows.
  const single = new Set(jobs.map((item) => String(item.job.id)));
  const rest = found.list ? found.list.jobs.filter((job) => !single.has(String(job.id))).map((job) => followed(job, current(found.list!.connectionId))) : [];
  return (
    <>
      {jobs.length > 0 && (
        <div className={cn("grid gap-2", jobs.length > 1 && "min-[900px]:grid-cols-2")}>
          {jobs.slice(-4).map((item) => (
            <JobCard key={item.key} item={item} now={now} />
          ))}
        </div>
      )}
      {rest.length > 0 && <JobList cluster={found.list!.cluster} jobs={rest} now={now} />}
    </>
  );
}
