"use client";

// Your usage: the last 30 days of requests and tokens as your providers report them,
// by day and by model, and today's allowance.

import { ChevronDown } from "lucide-react";
import { useState } from "react";
import { INPUT, Legend, OUTPUT, TimeBars } from "@/components/manage/bar-chart";
import { Eyebrow } from "@/components/app-shell/page";
import { Meter, Note, Quota, Section, TableWrap, Tiles, td, th } from "@/components/manage/kit";
import type { Model } from "@/lib/queries";

export interface UsageCounts {
  requests?: number;
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
  errors?: number;
  reservedTokens?: number;
  activeGenerations?: number;
  incompleteUsage?: number;
}
export interface OwnUsage {
  periodDays?: number;
  today?: UsageCounts;
  totals?: UsageCounts;
  limits?: { dailyRequests?: number | null; dailyTokens?: number | null; concurrentRequests?: number | null };
  daily?: (UsageCounts & { date: string })[];
  byModel?: (UsageCounts & { model: string | null })[];
}

/** 950, 9.5k, 48k, 1.3M. */
export const short = (value: number | null | undefined) => {
  const n = Number(value) || 0;
  return n < 1000 ? String(n) : n < 1e4 ? `${(n / 1000).toFixed(1)}k` : n < 1e6 ? `${Math.round(n / 1000)}k` : `${(n / 1e6).toFixed(1)}M`;
};
const num = (n: number | null | undefined) => Number(n || 0).toLocaleString();

/** Every day of the period, ending today (UTC), with zeros where nothing was used. */
export function usageDays(daily: OwnUsage["daily"], days: number, today = new Date()) {
  const byDate = new Map((daily || []).map((d) => [d.date, d]));
  return Array.from({ length: days }, (_, i) => {
    const day = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate() - (days - 1 - i)));
    const date = day.toISOString().slice(0, 10);
    const d = byDate.get(date) || ({} as UsageCounts);
    return {
      date,
      label: day.toLocaleDateString(undefined, { month: "short", day: "numeric", timeZone: "UTC" }),
      input: Number(d.promptTokens) || 0,
      output: Number(d.completionTokens) || 0,
      requests: Number(d.requests) || 0,
    };
  });
}

function Allowance({ label, used, limit, value = used }: { label: string; used?: number; limit?: number | null; value?: number }) {
  return (
    <div className="grid gap-1.5">
      <div className="flex justify-between gap-2 text-sm">
        <span>{label}</span>
        <span className="text-muted-foreground tabular-nums">
          {num(used)} / {limit == null ? "no limit" : num(limit)}
        </span>
      </div>
      {!!limit && <Quota value={Number(value || 0)} max={Number(limit)} label={label} />}
    </div>
  );
}

export function UsageCard({ usage, models }: { usage: OwnUsage; models: Model[] }) {
  const [table, setTable] = useState(false);
  const today = usage.today || {};
  const totals = usage.totals || {};
  const limits = usage.limits || {};
  const days = Number(usage.periodDays || 30);
  const series = usageDays(usage.daily, days);
  const total = series.reduce((n, d) => n + d.input + d.output, 0);
  const named = (id: string) => models.find((m) => m.id === id)?.name || id;
  const list = (usage.byModel || []).filter((r) => Number(r.totalTokens) > 0 || Number(r.requests) > 0);
  const top = Math.max(1, ...list.map((r) => Number(r.totalTokens) || 0));
  return (
    <Section title="Your usage" description={`Last ${days} days, as your providers report it. Days run in UTC.`} className="mb-6">
      <Tiles
        tiles={[
          { label: "Requests", value: num(totals.requests) },
          { label: "Tokens", value: short(totals.totalTokens), note: `${short(totals.promptTokens)} in · ${short(totals.completionTokens)} out` },
          { label: "Today", value: num(today.requests), note: `${short(today.totalTokens)} tokens` },
          { label: "Errors", value: num(totals.errors) },
        ]}
      />
      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_280px]">
        <div className="grid min-w-0 content-start gap-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Eyebrow>Tokens per day</Eyebrow>
            <Legend
              items={[
                { name: "Input", color: INPUT },
                { name: "Output", color: OUTPUT },
              ]}
            />
          </div>
          <TimeBars
            data={series}
            every={7}
            series={[
              { key: "input", name: "Input", color: INPUT },
              { key: "output", name: "Output", color: OUTPUT },
            ]}
            format={short}
            height={170}
            label={`Tokens per day over the last ${days} days: ${short(total)} in all`}
            tip={(d) => `${d.label}: ${d.requests} request${d.requests === 1 ? "" : "s"} · ${short(d.input)} in · ${short(d.output)} out`}
          />
        </div>
        <div className="grid content-start gap-3">
          <Eyebrow>By model</Eyebrow>
          {list.length ? (
            <ul className="grid gap-2.5">
              {list.slice(0, 8).map((r) => {
                const name = r.model ? named(r.model) : "Earlier chats";
                return (
                  <li key={r.model ?? ""} className="grid gap-1">
                    <div className="flex justify-between gap-2 text-sm">
                      <span className="truncate" title={r.model || ""}>
                        {name}
                      </span>
                      <span className="shrink-0 text-muted-foreground tabular-nums">
                        {short(r.totalTokens)} · {num(r.requests)} req
                      </span>
                    </div>
                    <Meter value={Number(r.totalTokens) || 0} max={top} label={`${name} tokens`} />
                  </li>
                );
              })}
            </ul>
          ) : (
            <Note>No usage yet.</Note>
          )}
          <Eyebrow className="mt-2">Today’s allowance</Eyebrow>
          <Allowance label="Requests" used={today.requests} limit={limits.dailyRequests} />
          <Allowance label="Tokens" used={today.totalTokens} limit={limits.dailyTokens} value={Number(today.totalTokens || 0) + Number(today.reservedTokens || 0)} />
          <Allowance label="At once" used={today.activeGenerations} limit={limits.concurrentRequests || 3} />
        </div>
      </div>
      <Note>
        Allowances reset at 00:00 UTC.
        {Number(today.reservedTokens) ? ` ${num(today.reservedTokens)} output tokens are held for responses running now.` : ""}
        {totals.incompleteUsage ? ` ${Number(totals.incompleteUsage)} requests did not report their usage.` : ""}
      </Note>
      {!!usage.daily?.length && (
        <div className="grid gap-2">
          <button type="button" aria-expanded={table} onClick={() => setTable((v) => !v)} className="inline-flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground">
            <ChevronDown className={table ? "size-3.5 rotate-180" : "size-3.5"} /> Show as a table
          </button>
          {table && (
            <TableWrap>
              <table className="w-full">
                <thead className="border-b">
                  <tr>
                    <th className={th}>Date (UTC)</th>
                    <th className={th}>Requests</th>
                    <th className={th}>Input</th>
                    <th className={th}>Output</th>
                  </tr>
                </thead>
                <tbody>
                  {usage.daily
                    .slice()
                    .reverse()
                    .map((d) => (
                      <tr key={d.date} className="border-b border-line-soft last:border-0">
                        <td className={td}>{d.date}</td>
                        <td className={td}>{Number(d.requests || 0)}</td>
                        <td className={td}>{num(d.promptTokens)}</td>
                        <td className={td}>{num(d.completionTokens)}</td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </TableWrap>
          )}
        </div>
      )}
    </Section>
  );
}
