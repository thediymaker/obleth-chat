"use client";

// Administration › Insights: what kind of work chats were, from the labels Chat keeps
// after a chat's first reply — kinds of use, fields, activities, what people wanted,
// flags (counted only), and the workstreams. No chat, message or person is shown.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, RefreshCw, Settings } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { Legend, TimeBars } from "@/components/manage/bar-chart";
import { CheckField, Chips, FormActions, FormDialog, FormError, Meter, Note, Pill, Section, TableWrap, TextField, Tiles, td, th, useSubmit } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { useModels } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { ChangeNote, GroupPicker, KindBar, KindLegend } from "./parts";
import { FLAGS, INSIGHT_PERIODS, KINDS, change, kindColor, nameOf, num, outlier, percent, share, tokens, type Insights, type InsightsSettings, type KindCount, type Workstream } from "./insights";
import { useKept } from "./state";

export interface InsightsState {
  days: number;
  group: string;
  sort: "sessions" | "tokens";
  open: Set<string>;
  all: Set<string>;
}
interface Handlers {
  setSort: (s: "sessions" | "tokens") => void;
  toggle: (name: string) => void;
  showAll: (name: string) => void;
  openSettings: () => void;
  findAgain: () => void;
}

const label = (at: string) => new Date(`${at}T00:00:00Z`).toLocaleDateString(undefined, { month: "short", day: "numeric", timeZone: "UTC" });

/** Labelled chats by kind of use over the period: by day for short periods, by week for longer ones. */
export function KindTrend({ trend }: { trend: Insights["trend"] }) {
  const points = trend?.points || [];
  if (points.length < 2) return null;
  const present = KINDS.filter(([id]) => points.some((p) => p.kinds?.[id]));
  const data = points.map((p) => ({ label: label(p.at), at: p.at, ...Object.fromEntries(KINDS.map(([id]) => [id, p.kinds?.[id] || 0])) })) as ({ label: string; at: string } & Record<string, number | string>)[];
  return (
    <TimeBars
      data={data}
      height={150}
      series={present.map(([id, name, color]) => ({ key: id, name, color: kindColor(color) }))}
      label={`Labelled chats by kind of use, by ${trend!.unit}`}
      tip={(d) =>
        `${trend!.unit === "week" ? "Week of " : ""}${d.label}: ${present
          .filter(([id]) => Number(d[id]))
          .map(([id, name]) => `${name} ${d[id]}`)
          .join(", ")}`
      }
    />
  );
}

function Stack({ data }: { data: Insights }) {
  const total = Number(data.sessions) || 0;
  const by = Object.fromEntries((data.kinds || []).map((k) => [k.name, k.sessions]));
  const present = KINDS.filter(([id]) => by[id]);
  const studied = share((by.research || 0) + (by.learning || 0), total);
  return (
    <Section title="Kind of use" actions={<span className="text-sm text-muted-foreground">{percent(studied)} research and learning</span>}>
      <KindBar kinds={data.kinds} label="Kind of use" by="sessions" className="h-4" />
      <KindLegend kinds={present}>
        {(id) => (
          <>
            {" "}
            <strong className="font-medium">{percent(share(by[id], total))}</strong> <small className="text-xs text-muted-foreground">{num(by[id])}</small>
          </>
        )}
      </KindLegend>
      <KindTrend trend={data.trend} />
    </Section>
  );
}

function Counts({ title, group, list, total }: { title: string; group: string; list?: KindCount[]; total: number }) {
  const max = Math.max(1, ...(list || []).map((x) => x.sessions));
  return (
    <Section title={title}>
      {list?.length ? (
        <ul className="grid gap-2">
          {list.map((x) => (
            <li key={x.name} className="grid grid-cols-[minmax(0,1fr)_minmax(60px,120px)_3rem] items-center gap-3 text-sm">
              <span className="truncate">{nameOf(group, x.name)}</span>
              <Meter value={x.sessions} max={max} />
              <strong className="text-right font-medium tabular-nums">{percent(share(x.sessions, total))}</strong>
            </li>
          ))}
        </ul>
      ) : (
        <Note>Nothing yet.</Note>
      )}
    </Section>
  );
}

function Flags({ data }: { data: Insights }) {
  const f = data.flags || {};
  return (
    <Section title="Flags" description="Counted only. Flagged chats are not saved or shown.">
      <ul className="grid gap-1.5">
        {FLAGS.map(([id, name]) => (
          <li key={id} className="flex items-baseline justify-between gap-3 text-sm">
            <span>{name}</span>
            <strong className={cn("font-medium tabular-nums", f[id] ? "text-amber" : "text-muted-foreground")} data-flagged={f[id] ? "" : undefined}>
              {num(f[id])}
            </strong>
          </li>
        ))}
      </ul>
    </Section>
  );
}

const SHOWN = 15;
function Detail({ stream, all, onShowAll }: { stream: Workstream; all: boolean; onShowAll: () => void }) {
  const chips = [
    ...Object.entries(stream.kinds || {}).map(([k, n]) => `${nameOf("kind", k)} ${num(n)}`),
    ...Object.entries(stream.outputs || {}).map(([k, n]) => `${nameOf("output", k)} ${num(n)}`),
  ];
  const chipRow = (
    <div className="flex flex-wrap gap-1">
      {chips.map((c) => (
        <Pill key={c}>{c}</Pill>
      ))}
    </div>
  );
  if (stream.private)
    return (
      <div className="grid gap-2 px-3 pb-3">
        {chipRow}
        <Note>These chats are counted but never listed: they were about the person’s own life, flagged explicit or self-harm, or have no topic line.</Note>
      </div>
    );
  const listed = stream.rows || [];
  const shown = all ? listed : listed.slice(0, SHOWN);
  return (
    <div className="grid gap-2 px-3 pb-3">
      {chipRow}
      <TableWrap>
        <table className="w-full min-w-[640px]">
          <thead className="border-b bg-muted/40">
            <tr>
              <th className={th}>Topic</th>
              <th className={th}>Use</th>
              <th className={th}>Wanted</th>
              <th className={th}>Field</th>
              <th className={th}>Messages</th>
              <th className={th}>Tokens</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((r, i) => (
              <tr key={i} className="border-b border-line-soft last:border-0">
                <td className={`${td} max-w-96`}>{r.topic}</td>
                <td className={td}>{nameOf("kind", r.kind)}</td>
                <td className={td}>{nameOf("output", r.output)}</td>
                <td className={td}>{nameOf("field", r.field)}</td>
                <td className={td}>{num(r.turns)}</td>
                <td className={td}>{tokens(r.tokens)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableWrap>
      {shown.length < listed.length ? (
        <Button size="xs" variant="outline" className="w-fit" onClick={onShowAll}>
          Show all {num(listed.length)}
        </Button>
      ) : stream.sessions > listed.length ? (
        <Note>Showing the {num(listed.length)} chats with the most tokens.</Note>
      ) : null}
    </div>
  );
}

function Workstreams({ data, state, on }: { data: Insights; state: InsightsState; on: Handlers }) {
  const w = data.workstreams || {};
  const s = w.state || {};
  const listed = w.list || [];
  const head = {
    title: "Workstreams",
    description: "What people use Chat for most. Chat keeps one list for every period and group, finds it again each week or when many chats fit none, and places each chat as it is labelled.",
    actions: s.building ? undefined : (
      <Button size="sm" variant="outline" onClick={on.findAgain}>
        <RefreshCw /> {listed.length ? "Find again" : "Find now"}
      </Button>
    ),
  };
  const building = s.building && (
    <p role="status" className="text-sm text-muted-foreground">
      {listed.length ? "Finding the list again" : "Finding workstreams"} with {s.model || "the workstreams model"}…
    </p>
  );
  const failed = !s.building && s.status === "failed" && <ErrorBox error={s.error || "Finding workstreams failed."} />;
  if (!listed.length)
    return (
      <Section {...head} aria-busy={s.building || undefined}>
        {building}
        {failed}
        {s.runs === false && <Note>Save the Insights settings to choose who finds and places workstreams; it runs as the administrator who saved them last.</Note>}
        {!s.building && !failed && <Note>The list appears on its own once about 20 chats have topic lines, or find it now.</Note>}
      </Section>
    );
  const streams = [...listed].sort((a, b) => b[state.sort] - a[state.sort] || b.sessions - a.sessions);
  const sessions = streams.reduce((n, x) => n + x.sessions, 0);
  const total = streams.reduce((n, x) => n + x.tokens, 0);
  const max = Math.max(0.0001, ...streams.flatMap((x) => [share(x.sessions, sessions), share(x.tokens, total)]));
  const big = outlier(data);
  return (
    <Section {...head}>
      {building}
      {failed}
      {big > 0 && (
        <p className="rounded-md border border-amber/40 bg-amber/10 px-3 py-2 text-sm text-amber">
          One chat used {percent(big)} of this period’s tokens ({tokens(data.largestChat)}), so token shares mostly show that one chat. Compare by chats instead.
        </p>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Chips
          label="Sort workstreams"
          options={[
            ["sessions", "By chats"],
            ["tokens", "By tokens"],
          ]}
          value={state.sort}
          onChange={on.setSort}
        />
        <Legend
          items={[
            { name: "Chats", color: "var(--chart-1)" },
            { name: "Tokens", color: "var(--chart-3)" },
          ]}
        />
      </div>
      <div className="grid gap-1.5" data-testid="workstreams">
        {streams.map((x) => {
          const open = state.open.has(x.name);
          const sh = share(x.sessions, sessions);
          const tk = share(x.tokens, total);
          return (
            <div key={x.name} className="rounded-md border">
              <button type="button" aria-expanded={open} onClick={() => on.toggle(x.name)} className="grid w-full gap-2 p-3 text-left hover:bg-accent md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)] md:items-center">
                <span className="flex min-w-0 items-start gap-2">
                  <ChevronDown className={cn("mt-0.5 size-4 shrink-0 text-muted-foreground", open && "rotate-180")} />
                  <span className="grid min-w-0">
                    <strong className="text-md font-medium">{x.name}</strong>
                    <small className="truncate text-xs text-muted-foreground" title={x.description}>
                      {x.description}
                    </small>
                  </span>
                </span>
                <span className="grid grid-cols-[minmax(0,1fr)_9rem] items-center gap-x-3 gap-y-1 text-xs">
                  <Meter value={sh} max={max} tone="blue" className="h-2.5" />
                  <span>
                    <strong className="font-medium">{percent(sh)}</strong> {num(x.sessions)} {x.sessions === 1 ? "chat" : "chats"}
                  </span>
                  <Meter value={tk} max={max} tone="gold" className="h-2.5" />
                  <span>
                    <strong className="font-medium">{percent(tk)}</strong> {tokens(x.tokens)} tokens
                  </span>
                </span>
              </button>
              {open && <Detail stream={x} all={state.all.has(x.name)} onShowAll={() => on.showAll(x.name)} />}
            </div>
          );
        })}
      </div>
      <Note>
        {s.foundAt ? `Found by ${s.model || "a model"} on ${when(s.foundAt)}. ` : ""}
        {w.waiting ? ` ${num(w.waiting)} ${w.waiting === 1 ? "chat is" : "chats are"} still being placed and count under Everything else for now.` : ""}
      </Note>
    </Section>
  );
}

/** Everything under the toolbar, from one summary. */
export function InsightsView({ data, state, on }: { data: Insights; state: InsightsState; on: Handlers }) {
  const days = state.days;
  if (!data.settings?.enabled)
    return (
      <EmptyState
        title="Usage insights are off"
        text={
          data.gateway
            ? "Turn them on to see what kind of work people do in Chat: research, learning, work or play, in which fields, and the workstreams. Chat keeps labels and a one-line topic for each chat, never the messages."
            : "Insights ask models through the gateway, and Chat has no gateway connected."
        }
        action={data.gateway ? <Button onClick={on.openSettings}>Turn on</Button> : undefined}
      />
    );
  const note = (
    <Note>
      From labels Chat keeps after a chat’s first reply and once a longer chat goes quiet{data.lastLabeledAt ? `, most recently ${when(data.lastLabeledAt)}` : ""}. Temporary chats are never read, and no chat, message or person is shown
      here.
    </Note>
  );
  if (data.tooFew)
    return (
      <>
        {note}
        <EmptyState
          title="Too few people to show"
          text={`In the last ${days} days this group has labelled chats from ${num(data.people)} ${data.people === 1 ? "person" : "people"}. A group shows from ${data.minPeople} people, so nobody can be singled out.`}
        />
      </>
    );
  if (!data.sessions)
    return (
      <>
        {note}
        <EmptyState title="No labelled chats yet" text={`Chats are labelled after their first reply. None from the last ${days} days are labelled yet.`} />
      </>
    );
  const p = data.previous || {};
  const by = Object.fromEntries((data.kinds || []).map((k) => [k.name, k.sessions]));
  const flagged = Object.values(data.flags || {}).reduce((a, b) => a + Number(b || 0), 0);
  const big = outlier(data);
  const studied = (by.research || 0) + (by.learning || 0);
  return (
    <>
      {note}
      <Tiles
        className="lg:grid-cols-5"
        tiles={[
          { label: "Chats", value: num(data.sessions), note: <ChangeNote change={change(data.sessions, p.sessions, days)} /> },
          { label: "People", value: num(data.people), note: <ChangeNote change={change(data.people, p.people, days)} /> },
          { label: "Tokens", value: tokens(data.tokens), note: big ? `One chat holds ${percent(big)}` : <ChangeNote change={change(data.tokens, p.tokens, days)} /> },
          { label: "Research and learning", value: percent(share(studied, data.sessions)), note: `${num(studied)} chats` },
          { label: "Flags", value: num(flagged), note: flagged ? "Counted only, see below" : "Nothing flagged" },
        ]}
      />
      <Stack data={data} />
      <Workstreams data={data} state={state} on={on} />
      <div className="grid gap-4 lg:grid-cols-2">
        <Counts title="Fields" group="field" list={data.fields} total={Number(data.sessions)} />
        <Counts title="Activities" group="activity" list={data.activities} total={Number(data.sessions)} />
        <Counts title="What people wanted" group="output" list={data.outputs} total={Number(data.sessions)} />
        <Flags data={data} />
      </div>
    </>
  );
}

/** The Insights settings: labelling, naming chats, and the two models. */
export function InsightsSettingsDialog({ settings, models, onClose, onSaved }: { settings?: InsightsSettings; models: string[]; onClose: () => void; onSaved: () => void }) {
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    await api.request("/api/admin/insights/settings", {
      method: "PUT",
      body: {
        enabled: form.get("insights-enabled") === "on",
        titles: form.get("insights-titles") === "on",
        model: String(form.get("insights-model") || "").trim() || null,
        reportModel: String(form.get("insights-report-model") || "").trim() || null,
      },
    });
    onSaved();
    toast("Insights saved");
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title="Usage insights">
      <form onSubmit={onSubmit} className="grid gap-4">
        <CheckField
          id="insights-enabled"
          label="Label chats"
          defaultChecked={!!settings?.enabled}
          description="After a chat’s first reply, and again when a longer chat goes quiet, ask a model what kind of work it was. Chat keeps the labels and a one-line topic, never the messages. A deleted chat still counts but loses its topic, and temporary chats are never read."
        />
        <CheckField
          id="insights-titles"
          label="Name new chats"
          defaultChecked={!!settings?.titles}
          description="After a chat’s first reply, give it a short title from the same model in place of the start of the first message. A title someone typed is never changed."
        />
        <TextField
          id="insights-model"
          label="Labelling model"
          maxLength={160}
          list="insights-models"
          spellCheck={false}
          autoComplete="off"
          placeholder="gemma4-31b-it"
          defaultValue={settings?.model || ""}
          help="Answers the questions about each chat and names it. It must return token probabilities (logprobs). Chat asks as each chat’s owner, so the gateway counts the tokens as theirs."
        />
        <TextField
          id="insights-report-model"
          label="Workstreams model"
          maxLength={160}
          list="insights-models"
          spellCheck={false}
          autoComplete="off"
          placeholder="The labelling model"
          defaultValue={settings?.reportModel || ""}
          help="Finds the workstream list from topic lines each week, or sooner when many chats fit none. A larger model names them better. Finding the list and placing chats in it run as the administrator who saved these settings last."
        />
        <datalist id="insights-models">
          {models.map((id) => (
            <option key={id} value={id} />
          ))}
        </datalist>
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Save
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

export function InsightsTab() {
  const queries = useQueryClient();
  const { data: models = [] } = useModels();
  const [days, setDays] = useKept("insights.days", 28);
  const [group, setGroup] = useKept("insights.group", "");
  const [sort, setSort] = useKept<"sessions" | "tokens">("insights.sort", "sessions");
  const [open, setOpen] = useState(new Set<string>());
  const [all, setAll] = useState(new Set<string>());
  const [settings, setSettings] = useState(false);
  const key = ["admin", "insights", days, group] as const;
  const insights = useQuery({
    queryKey: key,
    queryFn: () => api.request<Insights>(`/api/admin/insights?days=${days}${group ? `&group=${encodeURIComponent(group)}` : ""}`),
    placeholderData: (old) => old,
    // While the workstream list is being found, or chats wait for a place, look again every few seconds.
    refetchInterval: (q) => {
      const w = q.state.data?.workstreams;
      return w?.state?.building ? 4000 : w?.waiting ? 15000 : false;
    },
  });
  const data = insights.data;
  const on: Handlers = {
    setSort,
    toggle: (name) =>
      setOpen((old) => {
        const next = new Set(old);
        if (next.has(name)) next.delete(name);
        else next.add(name);
        return next;
      }),
    showAll: (name) => setAll((old) => new Set(old).add(name)),
    openSettings: () => setSettings(true),
    findAgain: async () => {
      try {
        const r = await api.request<{ state?: object }>("/api/admin/insights/workstreams", { method: "POST" });
        queries.setQueryData<Insights>(key, (old) => (old?.workstreams ? { ...old, workstreams: { ...old.workstreams, state: { ...r.state, building: true } } } : old));
        queries.invalidateQueries({ queryKey: key });
      } catch (e) {
        toast((e as Error).message);
      }
    },
  };
  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <Chips
            label="Period"
            options={INSIGHT_PERIODS.map((d) => [d, `${d} days`] as const)}
            value={days}
            onChange={(d) => {
              setDays(d);
              setOpen(new Set());
            }}
          />
          <GroupPicker
            id="admin-insights-group"
            groups={data?.groups}
            value={group}
            onChange={(g) => {
              setGroup(g);
              setOpen(new Set());
            }}
          />
        </div>
        <Button size="sm" variant="outline" onClick={() => setSettings(true)}>
          <Settings /> Settings
        </Button>
      </div>
      <ErrorBox error={insights.error} onRetry={() => insights.refetch()} />
      {data ? <InsightsView data={data} state={{ days, group, sort, open, all }} on={on} /> : !insights.error && <Loading label="Loading administration…" />}
      {settings && (
        <InsightsSettingsDialog
          settings={data?.settings}
          models={models.map((m) => m.id).filter((id) => id && id !== "auto")}
          onClose={() => setSettings(false)}
          onSaved={() => {
            setSettings(false);
            queries.invalidateQueries({ queryKey: ["admin", "insights"] });
          }}
        />
      )}
    </div>
  );
}
