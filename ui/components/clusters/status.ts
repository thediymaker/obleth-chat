"use client";

// What each of the person's clusters says right now, for the Clusters page and for
// the places that only count: Home's "Running now", the status line and the
// composer's @ list. Each read asks the cluster itself, so the counters ask less often.

import { useQueries } from "@tanstack/react-query";
import { useMemo } from "react";
import { type Connection, useConnections } from "@/components/connections/connections";
import { api } from "@/lib/api";
import { useFeature } from "@/lib/queries";
import { type ClusterStatus, blocked } from "./model";

export type Cluster = Connection & { needsCredential?: boolean };
export const isCluster = (c: Connection) => c.kind === "slurm" || c.kind === "kubernetes";
export const clusterStatusKey = (id: string) => ["cluster-status", id] as const;

export function clusterStatusQuery(c: Cluster, every = 30_000) {
  return {
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
    refetchInterval: every,
    staleTime: Math.min(every / 2, 30_000),
  };
}

/** One of the person's jobs on a cluster. */
export interface ClusterJob {
  cluster: string;
  clusterId: string;
  id: string;
  name: string;
  state: string;
  /** Where it runs or why it waits: "gpu · 12:04 of 2:00:00", "Priority". */
  note: string;
}

export interface ClusterQueue {
  running: number;
  pending: number;
  jobs: ClusterJob[];
}

const RUNNING = /^(RUNNING|COMPLETING|Running|Active)$/;
const PENDING = /^(PENDING|CONFIGURING|SUSPENDED|Pending|Suspended)$/;

/** The person's own jobs on one cluster that run or wait, and how many of each. */
export function clusterQueue(c: Pick<Connection, "id" | "name">, status: ClusterStatus | undefined): ClusterQueue {
  const s = status?.state === "connected" ? status.status : undefined;
  const raw = s?.jobs;
  const list = raw && !blocked(raw) ? (raw as { id?: string | number; name?: string; state?: string; partition?: string; elapsed?: string; timeLimit?: string; reason?: string; age?: string }[]) : [];
  const jobs = list.map((j) => {
    const state = String(j.state ?? "");
    const note = PENDING.test(state)
      ? [j.partition, j.reason && j.reason !== "None" ? j.reason : ""].filter(Boolean).join(" · ")
      : [j.partition, j.elapsed ? (j.timeLimit ? `${j.elapsed} of ${j.timeLimit}` : j.elapsed) : j.age].filter(Boolean).join(" · ");
    return { cluster: c.name, clusterId: c.id, id: String(j.id ?? j.name ?? ""), name: String(j.name ?? j.id ?? ""), state, note };
  });
  const live = jobs.filter((j) => RUNNING.test(j.state) || PENDING.test(j.state));
  return { running: live.filter((j) => RUNNING.test(j.state)).length, pending: live.filter((j) => PENDING.test(j.state)).length, jobs: live };
}

/** The clusters the person can use right now. */
export function useClusters(on = true): Cluster[] {
  const feature = useFeature("clusters");
  const { data } = useConnections();
  return useMemo(() => (feature && on ? ((data || []) as Cluster[]).filter((c) => isCluster(c) && c.active && !c.needsCredential) : []), [feature, on, data]);
}

/** Every usable cluster with its queue, read `every` milliseconds while `on`. */
export function useClusterQueues(on = true, every = 60_000) {
  const clusters = useClusters(on);
  const statuses = useQueries({ queries: clusters.map((c) => clusterStatusQuery(c, every)) });
  return clusters.map((c, i) => ({ cluster: c, state: statuses[i]?.data?.state, ...clusterQueue(c, statuses[i]?.data) }));
}
