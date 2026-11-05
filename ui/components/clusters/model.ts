// What GET /api/connections/:id/status says about a cluster (api/src/connections),
// and the small readers the Clusters page uses.

export type Blocked = { error: string };
export const blocked = (v: unknown): v is Blocked => !!v && !Array.isArray(v) && typeof (v as Blocked).error === "string";

export interface SlurmStatus {
  user?: string;
  totals?: { cpus?: number; allocCpus?: number; gpus?: number; allocGpus?: number; memoryMb?: number; allocMemoryMb?: number; nodes?: number; idleNodes?: number; unavailableNodes?: number };
  queue?: { running?: number; pending?: number };
  jobs?: { id: string | number; name?: string; state?: string; partition?: string; elapsed?: string; timeLimit?: string; nodes?: string; reason?: string }[];
  /** Jobs that ended in the last 7 days, the most recent first; `ended` is Unix seconds. */
  recent?: { id: string | number; name?: string; state?: string; partition?: string; elapsed?: string; ended?: number | null; exit?: number | null }[];
  partitions?: { name: string; state?: string; nodes?: number; idleNodes?: number; cpus?: number; idleCpus?: number; gpus?: number; idleGpus?: number; maxTime?: string }[];
}

export interface KubernetesStatus {
  namespace?: string;
  homeNamespace?: string;
  allowed?: { resources?: string; verbs?: string }[];
  pods?: { name: string; phase?: string; ready?: string; restarts?: number; age?: string }[] | Blocked;
  deployments?: { name: string; ready?: string; age?: string }[] | Blocked;
  jobs?: { name: string; state?: string; age?: string }[] | Blocked;
  nodes?: { name: string; ready?: boolean | string; roles?: string; cpu?: string; memory?: string; version?: string }[] | Blocked;
  quota?: { resource: string; used?: string; hard?: string; ratio?: number | null }[];
}

export interface ClusterStatus {
  state?: "connected" | "error" | string;
  error?: string;
  checkedAt?: string;
  status?: SlurmStatus & KubernetesStatus;
}

export const number = (n: number | null | undefined) => (n == null ? "–" : Number(n).toLocaleString());
export const gib = (mb: number) => (mb >= 1024 ? `${(mb / 1024).toFixed(mb >= 10240 ? 0 : 1)} GiB` : `${number(mb)} MiB`);

/** A job, pod or node state's color: ok, amber, bad or none. */
export function tone(state: string | null | undefined): "ok" | "amber" | "bad" | "" {
  const s = String(state ?? "");
  if (/RUNNING|Running|Complete|COMPLETED|IDLE|MIXED|UP|Succeeded|true/.test(s)) return "ok";
  if (/PENDING|Pending|ContainerCreating|ALLOCATED|COMPLETING/.test(s)) return "amber";
  if (/FAIL|Fail|Error|CrashLoop|DOWN|DRAIN|TIMEOUT|CANCELLED|OUT_OF_MEMORY|ImagePull|false/.test(s)) return "bad";
  return "";
}

const QUOTA: Record<string, string> = {
  "requests.cpu": "CPU requested",
  cpu: "CPU requested",
  "requests.memory": "Memory requested",
  memory: "Memory requested",
  pods: "Pod quota",
  "limits.cpu": "CPU limits",
  "limits.memory": "Memory limits",
  "requests.storage": "Storage",
  persistentvolumeclaims: "Volumes",
};
export const quotaName = (r: string) => QUOTA[r] || r;
export const quotaRank = (r: string) => {
  const i = Object.keys(QUOTA).indexOf(r);
  return i < 0 ? 99 : i;
};
/** The four quotas worth a meter, most useful first. */
export const quotaMeters = (quota: KubernetesStatus["quota"] = []) =>
  quota
    .filter((q) => q.ratio != null)
    .sort((a, b) => quotaRank(a.resource) - quotaRank(b.resource))
    .slice(0, 4);
