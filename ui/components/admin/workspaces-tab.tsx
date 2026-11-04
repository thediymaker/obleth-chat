"use client";

// Administration › Workspaces: the workspace and browser pods open now, with what they
// use and a Stop button; the period's pod hours, busiest moment against the slots, and
// starts that failed or were turned away; and the settings in effect.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { INPUT, Legend, OUTPUT, TimeBars } from "@/components/manage/bar-chart";
import { Chips, Note, Pill, Quota, Section, TableWrap, Tiles, displayName, td, th, useConfirm, type Tone } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { useKept } from "./state";
import { WORKSPACE_PERIODS, bytes, cores, hours, pointLabel, pointTip, since, until, type OpenPod, type PodCounts, type WorkspacesOverview } from "./workspaces";

const count = (n: number | null | undefined) => Number(n || 0).toLocaleString();
const plural = (n: number | null | undefined, one: string, many: string) => `${count(n)} ${Number(n) === 1 ? one : many}`;
const STATUS: Record<string, [string, Tone]> = { running: ["Running", "accent"], starting: ["Starting", ""], error: ["Failed", "danger"] };
type Metric = "hours" | "peak";

function Now({ data }: { data: WorkspacesOverview }) {
  const n = data.now || {};
  const ws = n.workspaces || {};
  const br = n.browsers || {};
  const used = Number(n.inUse) || 0;
  const slots = Number(data.slots) || 0;
  const extra = (s: PodCounts, fallback: string) => [s.starting && `${count(s.starting)} starting`, s.error && `${count(s.error)} failed`].filter(Boolean).join(" · ") || fallback;
  return (
    <Tiles
      tiles={[
        {
          label: "Pods in use",
          value: (
            <>
              {count(used)} <small className="text-sm font-normal text-muted-foreground">of {count(slots)}</small>
            </>
          ),
          note: <Quota value={Math.min(used, slots)} max={Math.max(1, slots)} label="Pods in use" className="mt-1.5" />,
        },
        { label: "Workspaces", value: count(ws.running), note: extra(ws, "Running now") },
        { label: "Browsers", value: data.browsers ? count(br.running) : "Off", note: data.browsers ? extra(br, "Running now") : "The browser is turned off" },
        { label: "Previews", value: count(n.previews), note: "Web servers people are previewing" },
        { label: "CPU now", value: data.metrics ? cores(n.cpuMillis) : "—", note: data.metrics ? "Across open pods" : "Chat can’t read pod metrics" },
        { label: "Memory now", value: data.metrics ? bytes(n.memoryBytes) : "—", note: data.metrics ? "Across open pods" : "See the note below" },
      ]}
    />
  );
}

function Open({ data, onStop }: { data: WorkspacesOverview; onStop: (o: OpenPod) => void }) {
  const idle = data.idleMinutes || {};
  const note = `Workspaces stop after ${count(idle.workspace)} minutes without use${data.browsers ? `, browsers after ${count(idle.browser)}` : ""}. Stopping keeps a person’s files and browser profile; their next use starts the pod again.`;
  const open = data.open || [];
  return (
    <Section title="Open now" description={note}>
      {open.length ? (
        <TableWrap>
          <table className="w-full min-w-[960px]">
            <thead className="border-b bg-muted/40">
              <tr>
                {["Person", "Pod", "Status", "Started", "Last used", "Stops", "CPU", "Memory", "Disk"].map((h) => (
                  <th key={h} className={th}>
                    {h}
                  </th>
                ))}
                <th className={th}>
                  <span className="sr-only">Stop</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {open.map((o) => {
                const [label, tone] = STATUS[o.status] || [o.status, ""];
                const what = o.kind === "browser" ? `${plural(o.chats, "chat", "chats")} with a tab` : `${plural(o.chats, "chat", "chats")}${o.previews ? ` · ${plural(o.previews, "preview", "previews")}` : ""}`;
                return (
                  <tr key={`${o.kind}-${o.ownerId}`} className="border-b border-line-soft last:border-0">
                    <td className={td}>
                      <strong className="block font-medium">{displayName(o)}</strong>
                      <small className="text-xs text-muted-foreground">{o.username}</small>
                    </td>
                    <td className={td}>
                      {o.kind === "browser" ? "Browser" : "Workspace"}
                      {o.status === "error" ? (
                        <small className="block max-w-56 truncate text-xs text-destructive-foreground" title={o.errorMessage || ""}>
                          {o.errorMessage || "Did not start"}
                        </small>
                      ) : (
                        <small className="block text-xs text-muted-foreground">{what}</small>
                      )}
                    </td>
                    <td className={td}>
                      <Pill tone={tone}>{label}</Pill>
                    </td>
                    <td className={`${td} text-sm`}>{since(o.startedAt)}</td>
                    <td className={`${td} text-sm`}>{since(o.lastUsedAt)}</td>
                    <td className={`${td} text-sm`}>{o.status === "running" ? until(o.stopsAt) : "—"}</td>
                    <td className={td}>{cores(o.cpuMillis)}</td>
                    <td className={td}>{o.memoryBytes == null ? "—" : bytes(o.memoryBytes)}</td>
                    <td className={td}>{o.diskBytes == null ? "—" : bytes(o.diskBytes)}</td>
                    <td className={`${td} text-right`}>
                      <Button size="xs" variant="outline" onClick={() => onStop(o)} aria-label={`Stop ${displayName(o)}’s ${o.kind}`}>
                        Stop
                      </Button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </TableWrap>
      ) : (
        <Note>No workspace or browser is open right now.</Note>
      )}
    </Section>
  );
}

function History({ data, days, metric, setMetric }: { data: WorkspacesOverview; days: number; metric: Metric; setMetric: (m: Metric) => void }) {
  const t = data.history?.totals || ({} as NonNullable<NonNullable<WorkspacesOverview["history"]>["totals"]>);
  const slots = Number(data.slots) || 0;
  const series = (data.history?.series || []).map((p) => ({ ...p, label: pointLabel(p.at, days), ws: Number(p.workspaceHours) || 0, br: Number(p.browserHours) || 0, top: Number(p.peak) || 0 }));
  return (
    <Section
      title={days === 1 ? "Today" : `The last ${days} days`}
      description="From each pod’s start and stop times. Times are UTC."
      actions={
        <Chips
          label="Show"
          options={[
            ["hours", "Pod hours"],
            ["peak", "Most at once"],
          ]}
          value={metric}
          onChange={setMetric}
        />
      }
    >
      <Tiles
        tiles={[
          { label: "Pod hours", value: hours((Number(t.workspaceHours) || 0) + (Number(t.browserHours) || 0)), note: `${hours(t.workspaceHours)} workspaces · ${hours(t.browserHours)} browsers` },
          { label: "Busiest", value: t.peak ? `${count(t.peak)} of ${count(slots)}` : "0", note: t.peakAt ? `Pods at once, ${when(t.peakAt)}` : "Pods at once" },
          { label: "People", value: count(t.people), note: "Used a workspace or browser" },
          { label: "Starts", value: count(t.starts), note: t.medianStartSeconds != null ? `Ready in ${count(t.medianStartSeconds)} s, typically` : "Pods started" },
          { label: "Failed starts", value: count(t.failed), note: "Pods that did not start" },
          { label: "Turned away", value: count(t.refused), note: t.refused ? "Every slot was in use" : "Nobody waited for a slot" },
        ]}
      />
      <Legend
        items={
          metric === "peak"
            ? [
                { name: "Pods at once", color: INPUT },
                { name: `Slots (${count(slots)})`, color: "var(--destructive)", dashed: true },
              ]
            : [
                { name: "Workspaces", color: INPUT },
                { name: "Browsers", color: OUTPUT },
              ]
        }
      />
      <TimeBars
        data={series}
        height={170}
        series={
          metric === "peak"
            ? [{ key: "top", name: "Pods at once", color: INPUT }]
            : [
                { key: "ws", name: "Workspaces", color: INPUT },
                { key: "br", name: "Browsers", color: OUTPUT },
              ]
        }
        limit={metric === "peak" && slots ? { value: slots } : undefined}
        format={(v) => (metric === "peak" ? String(Math.round(v)) : hours(v))}
        label={`${metric === "peak" ? "Most pods at once" : "Pod hours"} per ${days === 1 ? "hour" : "day"}`}
        tip={(p) => pointTip(p, days)}
      />
      {data.recordedSince && data.start && new Date(data.recordedSince) > new Date(data.start) && <Note>Chat started keeping this history {when(data.recordedSince)}, so earlier days are empty.</Note>}
    </Section>
  );
}

function SettingsInEffect({ data }: { data: WorkspacesOverview }) {
  const l = data.limits || {};
  const rows: [string, string][] = [
    ["Who can use them", data.everyone ? "Everyone" : "Administrators only"],
    ["Pods at once", count(data.slots)],
    ["Each workspace", `${l.cpu || "—"} CPU · ${l.memory || "—"} memory · ${l.disk || "—"} of files`],
    ...(data.browsers ? ([["Each browser", `${l.browserCpu || "—"} CPU · ${l.browserMemory || "—"} memory`]] as [string, string][]) : []),
  ];
  return (
    <Section title="Settings in effect">
      <ul className="grid gap-1.5">
        {rows.map(([name, value]) => (
          <li key={name} className="flex flex-wrap items-baseline justify-between gap-3 text-sm">
            <span>{name}</span>
            <strong className="font-medium">{value}</strong>
          </li>
        ))}
      </ul>
      <Note>Set in Chat’s deployment.</Note>
      {!data.metrics && <Note>CPU and memory need permission to read pod metrics in the workspace namespace (metrics.k8s.io, pods, get and list).</Note>}
    </Section>
  );
}

/** Everything under the toolbar, from one overview. */
export function WorkspacesView({ data, days, metric, setMetric, onStop }: { data: WorkspacesOverview; days: number; metric: Metric; setMetric: (m: Metric) => void; onStop: (o: OpenPod) => void }) {
  if (!data.enabled) return <EmptyState title="Workspaces are off" text={data.off || "Chat runs no workspace or browser pods."} />;
  return (
    <>
      <Now data={data} />
      <Open data={data} onStop={onStop} />
      <History data={data} days={days} metric={metric} setMetric={setMetric} />
      <SettingsInEffect data={data} />
    </>
  );
}

export function WorkspacesTab() {
  const queries = useQueryClient();
  const confirm = useConfirm();
  const [days, setDays] = useKept("workspaces.days", 7);
  const [metric, setMetric] = useKept<Metric>("workspaces.metric", "hours");
  const key = ["admin", "workspaces", days] as const;
  // What is open now changes, so the tab looks again every 30 seconds while shown.
  const overview = useQuery({
    queryKey: key,
    queryFn: () => api.request<WorkspacesOverview>(`/api/admin/workspaces?days=${days}`),
    refetchInterval: 30_000,
    placeholderData: (old) => old,
  });
  const stop = (o: OpenPod) =>
    confirm.ask({
      title: `Stop ${displayName(o)}’s ${o.kind}?`,
      text:
        o.kind === "browser"
          ? "The browser pod stops now. Their browser profile stays, and it starts again the next time they use it."
          : "The workspace pod stops now, along with any previews. Their files stay, and it starts again the next time they use it.",
      label: "Stop",
      action: async () => {
        await api.request("/api/admin/workspaces/stop", { method: "POST", body: { kind: o.kind, ownerId: o.ownerId } });
        await queries.invalidateQueries({ queryKey: ["admin", "workspaces"] });
        toast("Stopped");
      },
    });
  return (
    <div className="grid gap-4">
      {confirm.element}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Chips label="Period" options={WORKSPACE_PERIODS} value={days} onChange={setDays} />
        <Button size="sm" variant="outline" onClick={() => overview.refetch()}>
          <RefreshCw /> Refresh
        </Button>
      </div>
      <ErrorBox error={overview.error} onRetry={() => overview.refetch()} />
      {overview.data ? (
        <WorkspacesView data={overview.data} days={days} metric={metric} setMetric={setMetric} onStop={stop} />
      ) : (
        !overview.error && <Loading label="Loading administration…" />
      )}
    </div>
  );
}
