"use client";

// Compute › Clusters: each Slurm or Kubernetes cluster with its
// live state, as the viewer's own account sees it, refreshed every 30 seconds while
// the page is open. Adding, tokens, editing and removing use the connection dialogs.

import { useQueries, useQueryClient } from "@tanstack/react-query";
import { Info, Lock, MessageSquare, Plus, RefreshCw, Server } from "lucide-react";
import { useRouter } from "next/navigation";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Eyebrow, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { useChatStore } from "@/components/chat/chat-provider";
import { useConnectionManager } from "@/components/connections/connection-dialogs";
import { type Connection, KINDS, useConnections } from "@/components/connections/connections";
import { Meter, Pill, TableWrap, td, th } from "@/components/manage/kit";
import { ComputeOff, ComputeTabs } from "@/components/workspace/compute";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { useCapabilities } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { type ClusterStatus, type FileSession, type KubernetesStatus, type SlurmStatus, blocked, gib, number, quotaMeters, quotaName, tone } from "./model";

type Cluster = Connection & { needsCredential?: boolean };
const isCluster = (c: Connection) => c.kind === "slurm" || c.kind === "kubernetes";
export const clusterStatusKey = (id: string) => ["cluster-status", id] as const;

function State({ text }: { text?: string | null }) {
  const t = tone(text);
  return <span className={cn("font-medium", t === "ok" && "text-success", t === "amber" && "text-amber", t === "bad" && "text-destructive-foreground")}>{text || "–"}</span>;
}

function Stat({ label, value, note, meter }: { label: string; value: React.ReactNode; note?: string; meter?: { used: number; total: number } }) {
  return (
    <div className="grid content-start gap-1 rounded-md border bg-background p-3">
      <Eyebrow>{label}</Eyebrow>
      <strong className="text-base font-semibold">{value}</strong>
      {meter && meter.total > 0 && <Meter value={Math.min(meter.used, meter.total)} max={meter.total} label={`${label}: ${value}`} />}
      {note && <small className="text-xs text-muted-foreground">{note}</small>}
    </div>
  );
}

function Table({ heads, rows, empty }: { heads: string[]; rows: React.ReactNode[][]; empty: string }) {
  if (!rows.length) return <p className="text-sm text-muted-foreground">{empty}</p>;
  return (
    <TableWrap>
      <table className="w-full text-left">
        <thead>
          <tr className="border-b">
            {heads.map((h) => (
              <th key={h} className={th}>
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i} className="border-b last:border-b-0">
              {r.map((c, j) => (
                <td key={j} className={td}>
                  {c}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </TableWrap>
  );
}

function Part({ title, count, children }: { title: string; count?: number; children: React.ReactNode }) {
  return (
    <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2">
      <h3 className="text-md font-medium">
        {title}
        {count !== undefined && <span className="ml-1.5 text-muted-foreground">{count}</span>}
      </h3>
      {children}
    </section>
  );
}

const Sub = ({ children }: { children: React.ReactNode }) => <small className="block text-xs text-muted-foreground">{children}</small>;

/** The person's file session on the cluster: what runs it, and a way to end it. */
function FileSessionPart({ session, onEnd }: { session: FileSession | null; onEnd: () => void }) {
  return (
    <Part title="File session">
      {session ? (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-sm">
            {session.state === "live" ? (
              <>
                Job {session.jobId ?? "?"} on {session.host || "the cluster"}, last call {session.lastSeenAt ? when(session.lastSeenAt) : "just now"}.
              </>
            ) : (
              <>Job {session.jobId ?? "?"} is starting.</>
            )}{" "}
            It ends after 20 minutes without a call{session.endsAt ? `, or at ${when(session.endsAt)}` : ""}.
          </p>
          <Button size="sm" variant="outline" onClick={onEnd}>
            End session
          </Button>
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">None running. One starts as a job of yours when a chat first needs your files, and ends after 20 minutes without a call.</p>
      )}
    </Part>
  );
}

function Slurm({ s, session, onEnd }: { s: SlurmStatus; session?: FileSession | null; onEnd?: () => void }) {
  const t = s.totals || {};
  const parts = s.partitions || [];
  const jobs = s.jobs || [];
  const recent = s.recent || [];
  const gpus = (t.gpus || 0) > 0;
  const idle = (n?: number, all?: number) => (
    <>
      {number(n)} <small className="text-muted-foreground">of {number(all)}</small>
    </>
  );
  const nodeNote = [`${number(t.idleNodes)} idle`, t.unavailableNodes ? `${number(t.unavailableNodes)} unavailable` : ""].filter(Boolean).join(" · ");
  return (
    <>
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-5">
        <Stat label="CPUs in use" value={`${number(t.allocCpus)} / ${number(t.cpus)}`} meter={{ used: t.allocCpus || 0, total: t.cpus || 0 }} />
        {gpus && <Stat label="GPUs in use" value={`${number(t.allocGpus)} / ${number(t.gpus)}`} meter={{ used: t.allocGpus || 0, total: t.gpus || 0 }} />}
        <Stat label="Memory in use" value={`${gib(t.allocMemoryMb || 0)} / ${gib(t.memoryMb || 0)}`} meter={{ used: t.allocMemoryMb || 0, total: t.memoryMb || 0 }} />
        <Stat label="Nodes" value={number(t.nodes)} note={nodeNote} />
        <Stat label="Queue" value={`${number(s.queue?.running)} running`} note={`${number(s.queue?.pending)} pending`} />
      </div>
      <Part title={`Your jobs as ${s.user}`} count={jobs.length}>
        <Table
          heads={["Job", "State", "Partition", "Time", "Nodes or reason"]}
          rows={jobs.map((j) => [
            <>
              <strong className="font-medium">{j.id}</strong>
              <Sub>{j.name}</Sub>
            </>,
            <State text={j.state} />,
            j.partition,
            <>
              {j.elapsed} <small className="text-muted-foreground">of {j.timeLimit}</small>
            </>,
            j.nodes || j.reason || "–",
          ])}
          empty="No running or pending jobs of yours."
        />
      </Part>
      {session !== undefined && onEnd && <FileSessionPart session={session} onEnd={onEnd} />}
      <Part title="Recent jobs" count={recent.length}>
        <Table
          heads={["Job", "State", "Partition", "Ran", "Ended", "Exit code"]}
          rows={recent.map((j) => [
            <>
              <strong className="font-medium">{j.id}</strong>
              <Sub>{j.name}</Sub>
            </>,
            <State text={j.state} />,
            j.partition,
            j.elapsed,
            j.ended ? when(j.ended * 1000) : "–",
            j.exit ?? "–",
          ])}
          empty="No jobs of yours ended in the last 7 days."
        />
      </Part>
      <Part title="Partitions" count={parts.length}>
        <Table
          heads={["Partition", "State", "Idle nodes", "Idle CPUs", ...(gpus ? ["Idle GPUs"] : []), "Max time"]}
          rows={parts.map((p) => [<strong className="font-medium">{p.name}</strong>, <State text={p.state} />, idle(p.idleNodes, p.nodes), idle(p.idleCpus, p.cpus), ...(gpus ? [p.gpus ? idle(p.idleGpus, p.gpus) : "–"] : []), p.maxTime])}
          empty="No partitions."
        />
        {!!parts.length && <p className="text-sm text-muted-foreground">Idle means free to start a job now. Nodes that are down, drained or in maintenance never count as idle.</p>}
      </Part>
    </>
  );
}

function Kubernetes({ k }: { k: KubernetesStatus }) {
  const pods = Array.isArray(k.pods) ? k.pods : [];
  const running = pods.filter((p) => p.phase === "Running").length;
  const quota = Array.isArray(k.quota) ? k.quota : [];
  const section = <T,>(title: string, value: T[] | { error: string } | undefined, heads: string[], map: (v: T) => React.ReactNode[], empty: string) =>
    blocked(value) ? (
      <Part title={title}>
        <p className="text-sm text-muted-foreground">{value.error}</p>
      </Part>
    ) : (
      <Part title={title} count={(value || []).length}>
        <Table heads={heads} rows={(value || []).map(map)} empty={empty} />
      </Part>
    );
  const other = k.homeNamespace && k.homeNamespace !== k.namespace;
  return (
    <>
      {!k.allowed?.length && (
        <div className="flex items-start gap-2 rounded-md border border-amber/40 bg-amber/10 px-3 py-2 text-sm">
          <Lock className="mt-0.5 size-4 shrink-0 text-amber" />
          <span>
            This account has no permissions in {k.namespace}.{other ? ` Its token belongs to namespace ${k.homeNamespace}; replace the token and leave the namespace empty to use it.` : ""}
          </span>
        </div>
      )}
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-5">
        <Stat label="Namespace" value={k.namespace} note={other ? `token's own: ${k.homeNamespace}` : undefined} />
        <Stat label="Pods" value={blocked(k.pods) ? "Not visible" : `${running} running`} note={blocked(k.pods) ? undefined : `${pods.length} in all`} />
        {quotaMeters(quota).map((q) => (
          <Stat key={q.resource} label={quotaName(q.resource)} value={`${q.used} / ${q.hard}`} meter={{ used: Math.round((q.ratio || 0) * 1000), total: 1000 }} />
        ))}
        {!quota.length && <Stat label="Quota" value="None" note="No ResourceQuota in this namespace" />}
      </div>
      {section("Pods", k.pods, ["Pod", "State", "Ready", "Restarts", "Age"], (p) => [<strong className="font-medium">{p.name}</strong>, <State text={p.phase} />, p.ready, number(p.restarts), p.age], "No pods.")}
      {section("Deployments", k.deployments, ["Deployment", "Ready", "Age"], (d) => [<strong className="font-medium">{d.name}</strong>, d.ready, d.age], "No deployments.")}
      {section("Jobs", k.jobs, ["Job", "State", "Age"], (j) => [<strong className="font-medium">{j.name}</strong>, <State text={j.state} />, j.age], "No jobs.")}
      {!blocked(k.nodes) && section("Nodes", k.nodes, ["Node", "Ready", "Roles", "CPU", "Memory", "Version"], (n) => [<strong className="font-medium">{n.name}</strong>, <State text={String(n.ready)} />, n.roles || "–", n.cpu, n.memory, n.version], "No nodes.")}
      {!!k.allowed?.length && (
        <details className="text-sm">
          <summary className="cursor-pointer text-muted-foreground hover:text-foreground">What this account may do in {k.namespace}</summary>
          <ul className="mt-2 grid gap-1 pl-4">
            {k.allowed.map((r, i) => (
              <li key={i}>
                <code className="font-mono text-xs">{r.resources || "*"}</code> {r.verbs}
              </li>
            ))}
          </ul>
        </details>
      )}
    </>
  );
}

function ClusterCard({ c, s, loading, onRefresh, manager }: { c: Cluster; s?: ClusterStatus; loading: boolean; onRefresh: () => void; manager: ReturnType<typeof useConnectionManager> }) {
  const store = useChatStore();
  const router = useRouter();
  const kind = KINDS[c.kind];
  const Icon = kind?.icon || Server;
  const pill = !c.enabled ? (
    <Pill tone="amber">Off for everyone</Pill>
  ) : c.needsCredential ? (
    <Pill tone="amber">Needs your token</Pill>
  ) : s?.state === "connected" ? (
    <Pill tone="accent">Connected</Pill>
  ) : s?.state === "error" ? (
    <Pill tone="danger">Unreachable</Pill>
  ) : (
    <Pill>Checking…</Pill>
  );
  const who = c.credential?.identity || c.identity || c.credential?.user || c.user;
  const facts = [who && `as ${who}`, c.url, s?.checkedAt && `updated ${when(s.checkedAt)}`].filter(Boolean).join(" · ");
  let body: React.ReactNode = null;
  if (c.needsCredential)
    body = c.enabled && (
      <div className="flex flex-wrap items-center gap-3 rounded-md border border-dashed p-3">
        <p className="flex min-w-0 grow basis-60 items-start gap-2 text-sm">
          <Lock className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          Add your own token to see this cluster and use it in chats. Everything then runs as you.
        </p>
        <Button size="sm" onClick={() => manager.credential(c)}>
          Add your token
        </Button>
      </div>
    );
  else if (s?.state === "error")
    body = (
      <div role="alert" className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive-foreground">
        <Info className="mt-0.5 size-4 shrink-0" />
        <span>{s.error}</span>
      </div>
    );
  else if (s?.state === "connected" && s.status)
    body =
      c.kind === "slurm" ? (
        <Slurm
          s={s.status}
          session={c.files ? (s.fileSession ?? null) : undefined}
          onEnd={async () => {
            await api.request(`/api/connections/${encodeURIComponent(c.id)}/file-session`, { method: "DELETE" });
            onRefresh();
          }}
        />
      ) : (
        <Kubernetes k={s.status} />
      );
  else body = <Loading label="Checking the cluster…" />;
  return (
    <article className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4 rounded-lg border bg-card p-4">
      <header className="flex items-start gap-3">
        <Icon className="mt-1 size-5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <h2 className="flex flex-wrap items-center gap-1.5 text-lg font-semibold">
            {c.name} <Pill>{kind?.label.split(" ")[0] || c.kind}</Pill>
            {c.shared && <Pill>Shared</Pill>}
            {pill}
          </h2>
          <p className="mt-0.5 truncate text-sm text-muted-foreground" title={facts}>
            {facts}
          </p>
        </div>
        <div className="flex items-center gap-2">
          {loading && <span className="text-xs text-muted-foreground">Refreshing…</span>}
          <Button variant="ghost" size="icon-sm" aria-label={`Refresh ${c.name}`} title="Refresh" onClick={onRefresh}>
            <RefreshCw className={cn(loading && "animate-spin")} />
          </Button>
        </div>
      </header>
      {body}
      <footer className="flex flex-wrap gap-1.5 border-t pt-3">
        {c.active && (
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              store.newChat();
              store.starter(`Using the ${c.name} cluster, `, false);
              router.push("/");
            }}
          >
            <MessageSquare /> Ask in a new chat
          </Button>
        )}
        {c.usable !== false && (
          <Button size="sm" variant={c.active ? "outline" : "default"} aria-pressed={c.active} title={c.active ? "Offered to the model in your chats" : "Kept out of your chats"} onClick={() => manager.toggle(c)}>
            {c.active ? "In my chats" : "Not in my chats"}
          </Button>
        )}
        {c.shared && c.credential && (
          <>
            <Button size="sm" variant="ghost" onClick={() => manager.credential(c)}>
              Replace token
            </Button>
            <Button size="sm" variant="ghost" onClick={() => manager.removeCredential(c)}>
              Remove my token
            </Button>
          </>
        )}
        {c.canManage && (
          <>
            <Button size="sm" variant="ghost" onClick={() => manager.edit(c)}>
              Edit
            </Button>
            <Button size="sm" variant="ghost" onClick={() => manager.remove(c)}>
              Remove
            </Button>
          </>
        )}
      </footer>
    </article>
  );
}

export function ClustersPage() {
  usePageChrome("Compute");
  const caps = useCapabilities();
  const on = !!caps.data?.features?.connections;
  const queries = useQueryClient();
  const list = useConnections();
  const clusters = ((list.data || []) as Cluster[]).filter(isCluster);
  const manager = useConnectionManager({
    onChange: (item) => {
      if ("removed" in item) queries.removeQueries({ queryKey: clusterStatusKey(item.id) });
      else if (isCluster(item)) void queries.invalidateQueries({ queryKey: clusterStatusKey(item.id) });
    },
  });
  const statuses = useQueries({
    queries: clusters.map((c) => ({
      queryKey: clusterStatusKey(c.id),
      queryFn: async (): Promise<ClusterStatus> => {
        try {
          return await api.request<ClusterStatus>(`/api/connections/${encodeURIComponent(c.id)}/status`);
        } catch (e) {
          if ((e as Error).name === "AbortError") throw e;
          return { state: "error", error: (e as Error).message };
        }
      },
      enabled: !c.needsCredential,
      refetchInterval: 30_000,
      staleTime: 10_000,
    })),
  });
  const add = (
    <Button size="sm" onClick={() => manager.pick(["slurm", "kubernetes"])}>
      <Plus /> Add cluster
    </Button>
  );
  const head = (
    <PageHeader
      title="Compute"
      description="Slurm and Kubernetes clusters the model can work on in your chats, with your own account and permissions. Reading runs without asking; submitting, cancelling, applying and deleting ask you first."
      actions={on ? add : undefined}
    >
      <ComputeTabs />
    </PageHeader>
  );
  if (caps.data && !on)
    return (
      <Page>
        {head}
        <ComputeOff page="clusters" />
      </Page>
    );
  return (
    <Page wide>
      {head}
      {!list.data ? (
        list.error ? (
          <ErrorBox error={list.error} onRetry={() => list.refetch()} />
        ) : (
          <Loading label="Loading clusters…" />
        )
      ) : !clusters.length ? (
        <EmptyState icon={Server} title="No clusters yet" text="Add a Slurm cluster through slurmrestd or a Kubernetes cluster through its API server, with a token from your account." action={add} />
      ) : (
        <div className="grid grid-cols-[minmax(0,1fr)] gap-4">
          {clusters.map((c, i) => (
            <ClusterCard key={c.id} c={c} s={statuses[i]?.data} loading={!!statuses[i]?.isFetching} onRefresh={() => void statuses[i]?.refetch()} manager={manager} />
          ))}
        </div>
      )}
      {manager.element}
    </Page>
  );
}
