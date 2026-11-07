"use client";

// Administration › Usage & limits: requests and tokens over a period, by model, by
// purpose and by person; what the tokens went to (from Insights labels); who is at or
// near a daily limit; and the daily limits, for everyone and for one person.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Settings } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { INPUT, Legend, OUTPUT, TimeBars } from "@/components/manage/bar-chart";
import { CheckField, Chips, FormActions, FormDialog, FormError, Meter, Dropdown, Note, Pill, Quota, Section, TableWrap, TextField, Tiles, displayName, td, th, useSubmit } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { ChangeNote, GroupPicker, KindBar, KindLegend } from "./parts";
import { KINDS, change, num, percent, plural, share, tokens } from "./insights";
import { SearchForm } from "./people-tab";
import { useKept } from "./state";
import { METRICS, PURPOSES, SORTS, USAGE_PERIODS, allowance, buckets, periodName, sortPeople, type Limits, type Metric, type Sort, type UsageOverview, type UsagePerson } from "./usage";

export interface UsageState {
  days: number;
  group: string;
  model: string;
  query: string;
  sort: Sort;
  metric: Metric;
}
interface Handlers {
  setDays: (d: number) => void;
  setGroup: (g: string) => void;
  setModel: (m: string) => void;
  setQuery: (q: string) => void;
  setSort: (s: Sort) => void;
  setMetric: (m: Metric) => void;
  editLimits: (userId?: string) => void;
}

function Toolbar({ data, state, on }: { data?: UsageOverview; state: UsageState; on: Handlers }) {
  const models = [...new Set([...(data?.models || []), ...(state.model ? [state.model] : [])])];
  return (
    <div className="flex flex-wrap items-center justify-between gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Chips label="Period" options={USAGE_PERIODS} value={state.days} onChange={on.setDays} />
        <GroupPicker id="admin-usage-group" groups={data?.groups} value={state.group} onChange={on.setGroup} />
        {models.length > 0 && (
          <>
            <label className="sr-only" htmlFor="admin-usage-model">
              Model
            </label>
            <Dropdown id="admin-usage-model" value={state.model} onChange={(e) => on.setModel(e.target.value)} className="h-7 w-auto min-w-36 text-sm">
              <option value="">All models</option>
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Dropdown>
          </>
        )}
      </div>
      <Button size="sm" variant="outline" onClick={() => on.editLimits()}>
        <Settings /> Workspace limits
      </Button>
    </div>
  );
}

function Shares({ list, total, name, pick, current }: { list: (UsageOverview["byModel"] & object)[number][]; total: number; name: (r: { name: string | null }) => string; pick?: (name: string) => void; current?: string }) {
  const max = Math.max(1, ...list.map((r) => Number(r.totalTokens) || 0));
  return (
    <ul className="grid gap-3">
      {list.map((r) => {
        const label = name(r);
        const t = Number(r.totalTokens) || 0;
        return (
          <li key={r.name ?? "none"} className="grid gap-1">
            <div className="flex items-baseline justify-between gap-2 text-sm">
              {pick && r.name ? (
                <button type="button" className="truncate text-left underline-offset-2 hover:underline" title={r.name === current ? "Show all models" : "Show only this model"} onClick={() => pick(r.name!)}>
                  {label}
                </button>
              ) : (
                <span className="truncate">{label}</span>
              )}
              <span className="shrink-0 tabular-nums">
                <strong className="font-medium">{percent(share(t, total))}</strong> <span className="text-muted-foreground">{tokens(t)}</span>
              </span>
            </div>
            <Meter value={t} max={max} />
            <small className="text-xs text-muted-foreground">
              {plural(r.requests, "request", "requests")} · {plural(r.people, "person", "people")}
              {Number(r.errors) ? ` · ${percent(share(r.errors, r.requests))} errors` : ""}
            </small>
          </li>
        );
      })}
    </ul>
  );
}

function Mix({ data, days }: { data: UsageOverview; days: number }) {
  const m = data.mix || {};
  const head = { title: "What the tokens went to", description: "Kind of use from Insights labels, by tokens." };
  if (!m.enabled)
    return (
      <Section {...head}>
        <Note>Turn on Insights to see whether tokens went to research, learning, work or play, for each model and for the people who use the most.</Note>
        <Button size="sm" variant="outline" className="w-fit" asChild>
          <Link href="/settings/admin/insights">Open Insights</Link>
        </Button>
      </Section>
    );
  if (m.tooFew)
    return (
      <Section {...head}>
        <Note>
          This group has labelled chats from {plural(m.people, "person", "people")} in {periodName(days)}. Kinds of use show from {m.minPeople} people, so nobody can be singled out.
        </Note>
      </Section>
    );
  if (!m.sessions)
    return (
      <Section {...head}>
        <Note>No chats from {periodName(days)} are labelled yet.</Note>
      </Section>
    );
  const Line = ({ label, note, kinds }: { label: string; note: string; kinds?: { name: string; tokens?: number }[] }) => (
    <li className="grid gap-1.5">
      <div className="flex flex-wrap items-baseline justify-between gap-2 text-sm">
        <span>{label}</span>
        <small className="text-xs text-muted-foreground">{note}</small>
      </div>
      <KindBar kinds={kinds} label={label} />
    </li>
  );
  return (
    <Section {...head} actions={<KindLegend kinds={KINDS.filter(([id]) => (m.kinds || []).some((k) => k.name === id))} />}>
      <ul className="grid gap-3">
        <Line label="All labelled chats" note={`${plural(m.sessions, "chat", "chats")} · ${plural(m.people, "person", "people")}`} kinds={m.kinds} />
      </ul>
      <h3 className="text-md font-medium">By model</h3>
      <ul className="grid gap-3">
        {(m.byModel || []).map((r) => (
          <Line key={r.model ?? "none"} label={r.model || "Unknown model"} note={`${plural(r.sessions, "chat", "chats")} · ${tokens(r.tokens)} tokens`} kinds={r.kinds} />
        ))}
      </ul>
      {m.heavy ? (
        <>
          <h3 className="text-md font-medium">People who use the most</h3>
          <ul className="grid gap-3">
            <Line label={`The ${num(m.heavy.top.people)} who used the most`} note={`${percent(share(m.heavy.top.tokens, m.tokens))} of tokens`} kinds={m.heavy.top.kinds} />
            <Line label={`Everyone else (${num(m.heavy.rest.people)})`} note={`${percent(share(m.heavy.rest.tokens, m.tokens))} of tokens`} kinds={m.heavy.rest.kinds} />
          </ul>
        </>
      ) : (
        <Note>The split for the people who use the most shows once {num(2 * Number(m.minPeople || 0))} people have labelled chats.</Note>
      )}
      <Note>Counts chat replies in labelled chats only, by the model each chat used most, so totals differ from the figures above. No chat or person is shown.</Note>
    </Section>
  );
}

function Allowance({ person }: { person: UsagePerson }) {
  const a = allowance(person.today, person.limits);
  if (!a)
    return (
      <span className="grid gap-0.5">
        {tokens(person.today?.totalTokens)} <small className="text-xs text-muted-foreground">No daily limit</small>
      </span>
    );
  const show = (n: number) => (a.what === "tokens" ? tokens(n) : num(n));
  return (
    <span className="grid min-w-28 gap-1" data-level={a.level || undefined}>
      <Quota value={Math.min(a.used, a.limit)} max={a.limit} label={`Today’s ${a.what}`} />
      <small className="text-xs text-muted-foreground">
        {show(a.used)} / {show(a.limit)} {a.what}
      </small>
    </span>
  );
}

function LimitsCard({ data, on }: { data: UsageOverview; on: Handlers }) {
  const l: Limits = data.limits || {};
  return (
    <Section
      title="Daily limits"
      description={`Allowances reset at 00:00 UTC.${data.overrides ? ` ${plural(data.overrides, "person has", "people have")} their own limits.` : ""}`}
      actions={
        <Button size="sm" variant="outline" onClick={() => on.editLimits()}>
          <Settings /> Edit defaults
        </Button>
      }
    >
      <Tiles
        className="lg:grid-cols-3"
        tiles={(
          [
            ["Daily requests", l.dailyRequests],
            ["Daily tokens", l.dailyTokens],
            ["Replies at once", l.concurrentRequests],
          ] as const
        ).map(([label, value]) => ({ label, value: value == null ? "Unlimited" : num(value) }))}
      />
      {data.limited?.length ? (
        <>
          <h3 className="text-md font-medium">At or near a limit today</h3>
          <ul className="grid gap-2">
            {data.limited.map((p) => (
              <li key={p.id} className="flex items-center gap-3 rounded-md border px-3 py-2">
                <span className="grid min-w-0 grow">
                  <span className="flex items-center gap-1.5">
                    <strong className="truncate font-medium">{displayName(p)}</strong> {p.atLimit ? <Pill tone="danger">At limit</Pill> : <Pill tone="amber">Near limit</Pill>}
                  </span>
                  <small className="text-xs text-muted-foreground">{p.username}</small>
                </span>
                <Button size="xs" variant="outline" onClick={() => on.editLimits(p.id)}>
                  Limits
                </Button>
              </li>
            ))}
          </ul>
        </>
      ) : (
        <Note>Nobody is near a daily limit today.</Note>
      )}
    </Section>
  );
}

function People({ data, state, on }: { data: UsageOverview; state: UsageState; on: Handlers }) {
  const list = sortPeople(data.people || [], state.sort);
  const total = Number(data.totals?.totalTokens) || 0;
  const max = Math.max(1, ...list.map((p) => Number(p.totalTokens) || 0));
  return (
    <Section
      title="People"
      description={`Use in ${periodName(state.days)}, and today against each person’s daily limit.`}
      actions={<SearchForm id="usage-query" label="Search people" placeholder="Search people…" value={state.query} onSearch={on.setQuery} />}
    >
      <Chips label="Sort people" options={SORTS} value={state.sort} onChange={on.setSort} />
      {list.length ? (
        <TableWrap>
          <table className="w-full min-w-[820px]" data-testid="usage-people">
            <thead className="border-b bg-muted/40">
              <tr>
                <th className={th}>Person</th>
                <th className={th}>Requests</th>
                <th className={th}>Tokens</th>
                <th className={th}>Today</th>
                <th className={th}>Errors</th>
                <th className={th}>Last used</th>
                <th className={th}>
                  <span className="sr-only">Limits</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((p) => (
                <tr key={p.id} className="border-b border-line-soft last:border-0">
                  <td className={td}>
                    <span className="flex flex-wrap items-center gap-1.5">
                      <strong className="font-medium">{displayName(p)}</strong>
                      {p.enabled === false && <Pill>Disabled</Pill>}
                      {p.override && <Pill>Own limits</Pill>}
                    </span>
                    <small className="text-xs text-muted-foreground">{p.username}</small>
                  </td>
                  <td className={td}>{num(p.requests)}</td>
                  <td className={td}>
                    <span className="grid min-w-28 gap-1">
                      <span>
                        {tokens(p.totalTokens)} <small className="text-xs text-muted-foreground">{percent(share(p.totalTokens, total))}</small>
                      </span>
                      <Meter value={Number(p.totalTokens) || 0} max={max} />
                    </span>
                  </td>
                  <td className={td}>
                    <Allowance person={p} />
                  </td>
                  <td className={td}>{Number(p.errors) ? num(p.errors) : "—"}</td>
                  <td className={td}>
                    <small className="text-xs">{when(p.lastActive)}</small>
                  </td>
                  <td className={`${td} text-right`}>
                    <Button size="xs" variant="outline" onClick={() => on.editLimits(p.id)}>
                      Limits
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWrap>
      ) : (
        <EmptyState title={state.query ? "No matching people" : "Nobody used Chat"} text={state.query ? "Try another username or display name." : `No requests in ${periodName(state.days)}.`} />
      )}
      {data.hasMore && <Note>Showing the {num(list.length)} people who used the most tokens. Search to find someone else.</Note>}
    </Section>
  );
}

/** Everything under the toolbar, from one overview. */
export function UsageView({ data, state, on }: { data: UsageOverview; state: UsageState; on: Handlers }) {
  const { days, model, metric } = state;
  const t = data.totals || {};
  const p = data.previous || {};
  const points = buckets(data.series, days, data.start);
  const total = Number(t.totalTokens) || 0;
  const value = (d: (typeof points)[number]) => (metric === "tokens" ? d.input + d.output : d[metric]);
  const short = (v: number) => (metric === "tokens" ? tokens(Math.round(v)) : num(Math.round(v)));
  const metricName = METRICS.find(([id]) => id === metric)?.[1] || "Tokens";
  return (
    <>
      <Note>
        {days === 1 ? "Today" : `The last ${days} days`}
        {model ? `, ${model} only` : ""}, as providers report it. Days run in UTC.
        {t.incompleteUsage ? ` ${plural(t.incompleteUsage, "request", "requests")} did not report usage.` : ""}
      </Note>
      <Tiles
        tiles={[
          { label: "Requests", value: num(t.requests), note: <ChangeNote change={change(t.requests, p.requests, days)} /> },
          { label: "Tokens", value: tokens(t.totalTokens), note: `${tokens(t.promptTokens)} in · ${tokens(t.completionTokens)} out` },
          { label: "People", value: num(t.people), note: <ChangeNote change={change(t.people, p.people, days)} /> },
          { label: "Errors", value: num(t.errors), note: t.requests ? `${percent(share(t.errors, t.requests))} of requests` : "" },
          { label: "Running now", value: num(t.activeGenerations), note: "Replies being written" },
          { label: "At a daily limit", value: num(data.atLimit), note: data.nearLimit ? `${plural(data.nearLimit, "more person", "more people")} above 80%` : "Today, UTC" },
        ]}
      />
      <Section title={days === 1 ? "Today by hour" : "By day"} actions={<Chips label="Show" options={METRICS} value={metric} onChange={on.setMetric} />}>
        {metric === "tokens" && (
          <Legend
            items={[
              { name: "Input", color: INPUT },
              { name: "Output", color: OUTPUT },
            ]}
          />
        )}
        <TimeBars
          data={points}
          series={
            metric === "tokens"
              ? [
                  { key: "input", name: "Input", color: INPUT },
                  { key: "output", name: "Output", color: OUTPUT },
                ]
              : [{ key: metric, name: metricName, color: INPUT }]
          }
          format={short}
          label={`${metricName} per ${points.length === 24 ? "hour" : "day"}: ${short(points.reduce((s, d) => s + value(d), 0))} in all`}
          tip={(d) =>
            `${d.label}: ${plural(d.requests, "request", "requests")} · ${tokens(d.input)} in · ${tokens(d.output)} out · ${plural(d.people, "person", "people")}${d.errors ? ` · ${plural(d.errors, "error", "errors")}` : ""}`
          }
        />
        <Note>Hover a bar for its requests, tokens and people. Times are UTC.</Note>
      </Section>
      <div className="grid gap-4 lg:grid-cols-2">
        <Section title="Models">
          {data.byModel?.length ? (
            <Shares list={data.byModel} total={total} name={(r) => r.name || "Earlier chats"} pick={(m) => on.setModel(model === m ? "" : m)} current={model} />
          ) : (
            <Note>Nothing yet.</Note>
          )}
          {!model && <Note>Pick a model to see only its use.</Note>}
        </Section>
        <Section title="What requests were for">
          {data.byPurpose?.length ? <Shares list={data.byPurpose} total={total} name={(r) => PURPOSES[r.name ?? ""] || r.name || ""} /> : <Note>Nothing yet.</Note>}
          <Note>Follow-ups are the suggested next questions under a reply. Agents are tasks a model hands to a worker model.</Note>
        </Section>
      </div>
      <Mix data={data} days={days} />
      <LimitsCard data={data} on={on} />
      <People data={data} state={state} on={on} />
    </>
  );
}

interface LimitsResponse {
  limits?: Limits;
  overrides?: { userId: string; limits: Limits }[];
}

/** The limits form's body (admin.js submit for admin-limits-form). */
export function limitsBody(form: FormData, userId: string | null) {
  const body: Record<string, number | null> = {};
  for (const [name, key] of [
    ["requests", "dailyRequests"],
    ["tokens", "dailyTokens"],
    ["concurrent", "concurrentRequests"],
  ] as const) {
    const value = String(form.get(`limit-${name}`) ?? "");
    if (userId && name !== "concurrent" && form.get(`limit-${name}-unlimited`) === "on") body[key] = null;
    else if (value) body[key] = Number(value);
    else if (!userId && name !== "concurrent") body[key] = null;
  }
  return body;
}

function LimitsDialog({ userId, person, data, onClose, onSaved }: { userId: string | null; person?: UsagePerson; data?: LimitsResponse; onClose: () => void; onSaved: () => void }) {
  const limits: Limits = (userId ? data?.overrides?.find((o) => o.userId === userId)?.limits : data?.limits) || {};
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    await api.request(`/api/admin/limits${userId ? `/${encodeURIComponent(userId)}` : ""}`, { method: userId ? "PUT" : "PATCH", body: limitsBody(form, userId) });
    onSaved();
    toast("Usage limits saved");
  });
  const placeholder = userId ? "Inherit workspace default" : "Unlimited";
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title={userId ? `Limits for ${displayName(person)}` : "Workspace limits"}>
      <form onSubmit={onSubmit} className="grid gap-4">
        <Note>{userId ? "Leave a field blank to inherit the workspace default." : "Leave daily limits blank for unlimited usage."} Concurrent responses must be between 1 and 32.</Note>
        <TextField id="limit-requests" label="Requests per day" type="number" min={1} max={1000000} placeholder={placeholder} defaultValue={limits.dailyRequests ?? ""} />
        {userId && <CheckField id="limit-requests-unlimited" label="Unlimited daily requests" defaultChecked={Object.hasOwn(limits, "dailyRequests") && limits.dailyRequests === null} />}
        <TextField id="limit-tokens" label="Tokens per day" type="number" min={1} max={1000000000} placeholder={placeholder} defaultValue={limits.dailyTokens ?? ""} />
        {userId && <CheckField id="limit-tokens-unlimited" label="Unlimited daily tokens" defaultChecked={Object.hasOwn(limits, "dailyTokens") && limits.dailyTokens === null} />}
        <TextField id="limit-concurrent" label="Concurrent responses" type="number" min={1} max={32} placeholder="Inherit workspace default" required={!userId} defaultValue={limits.concurrentRequests ?? ""} />
        <Note>Token allowances include reported usage and reserved output tokens for active responses.</Note>
        <FormError message={error} />
        <FormActions>
          {userId && (
            <Button
              type="button"
              variant="ghost"
              onClick={async () => {
                try {
                  await api.request(`/api/admin/limits/${encodeURIComponent(userId)}`, { method: "DELETE" });
                  onSaved();
                  toast("Account uses workspace limits");
                } catch (e) {
                  toast((e as Error).message);
                }
              }}
            >
              Remove all overrides
            </Button>
          )}
          <Button type="submit" disabled={busy}>
            Save limits
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

export function UsageTab() {
  const queries = useQueryClient();
  const [days, setDays] = useKept("usage.days", 28);
  const [group, setGroup] = useKept("usage.group", "");
  const [model, setModel] = useKept("usage.model", "");
  const [query, setQuery] = useKept("usage.query", "");
  const [sort, setSort] = useKept<Sort>("usage.sort", "totalTokens");
  const [metric, setMetric] = useKept<Metric>("usage.metric", "tokens");
  const [limitsFor, setLimitsFor] = useState<string | null | undefined>(undefined);
  const params = new URLSearchParams({ days: String(days) });
  if (group) params.set("group", group);
  if (model) params.set("model", model);
  if (query) params.set("q", query);
  const overview = useQuery({
    queryKey: ["admin", "usage", String(params)],
    queryFn: () => api.request<UsageOverview>(`/api/admin/usage/overview?${params}`),
    placeholderData: (old) => old,
  });
  const limits = useQuery({ queryKey: ["admin", "limits"], queryFn: () => api.request<LimitsResponse>("/api/admin/limits") });
  const state: UsageState = { days, group, model, query, sort, metric };
  const on: Handlers = { setDays, setGroup, setModel, setQuery, setSort, setMetric, editLimits: (id) => setLimitsFor(id ?? null) };
  const data = overview.data;
  const people = [...(data?.people || []), ...(data?.limited || [])];
  return (
    <div className="grid gap-4">
      <Toolbar data={data} state={state} on={on} />
      <ErrorBox
        error={overview.error || limits.error}
        onRetry={() => {
          overview.refetch();
          limits.refetch();
        }}
      />
      {data ? <UsageView data={data} state={state} on={on} /> : !overview.error && <Loading label="Loading administration…" />}
      {limitsFor !== undefined && (
        <LimitsDialog
          userId={limitsFor}
          person={people.find((p) => p.id === limitsFor)}
          data={limits.data}
          onClose={() => setLimitsFor(undefined)}
          onSaved={() => {
            setLimitsFor(undefined);
            queries.invalidateQueries({ queryKey: ["admin", "usage"] });
            queries.invalidateQueries({ queryKey: ["admin", "limits"] });
          }}
        />
      )}
    </div>
  );
}
