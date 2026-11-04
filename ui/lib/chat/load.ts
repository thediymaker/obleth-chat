import type { Message, ModelInfo, Preferences } from "./types";
import type { ApiClient } from "@/lib/api";

// How long a reply may wait for its first words before Chat says what is holding it.
export const WAIT_NOTICE_MS = 3000;
// A gateway that can't say how busy its models are is asked again after this long.
const UNAVAILABLE_MS = 5 * 60 * 1000;

export interface ModelLoad {
  state?: "free" | "busy" | "full" | "offline" | string;
  slots: number;
  inFlight: number;
  waiting?: number;
  you?: { inFlight: number; limit?: number };
}
export interface LoadData {
  available: boolean | null;
  models: Record<string, ModelLoad>;
}

// How many run on how many slots. Running can pass slots for a while: when servers are
// removed, the requests they hold finish while the slots already count only the rest.
const running = (l: ModelLoad) => `${l.inFlight} running on ${l.slots} slots`;
/** A model's load in words, for its tooltip and the waiting notice. Free says nothing. */
export const loadLabel = (l?: ModelLoad) =>
  !l
    ? ""
    : l.state === "full"
      ? `Full: ${running(l)}${l.waiting ? `, ${l.waiting} waiting` : ""}`
      : l.state === "busy"
        ? `Busy: ${running(l)}`
        : l.state === "offline"
          ? "No servers running right now"
          : "";

export interface LoadNotice {
  text: string;
  /** Still waiting (a live status), rather than given up on. */
  waiting: boolean;
  /** Another model to send it to, or "" for none. */
  next: string;
  label: string;
}

/**
 * Live load per model, from the gateway through /api/models/load. Quiet by design: a
 * model's own icon turns amber while busy and red while full, and nothing changes while it is free.
 */
export function createLoad({
  api,
  state,
  modelName,
}: {
  api: Pick<ApiClient, "request">;
  state: () => { models: ModelInfo[]; preferences?: Preferences | null };
  modelName: (id: string) => string;
}) {
  let data: LoadData = { available: null, models: {} },
    fetchedAt = 0,
    pending: Promise<LoadData> | null = null;
  const listeners = new Set<() => void>();
  const of = (id: string) => data.models?.[id];
  /** Fetches the load unless the copy in hand is younger than `maxAge`. */
  function refresh(maxAge = 5000): Promise<LoadData> {
    const age = Date.now() - fetchedAt;
    if (age < (data.available === false ? UNAVAILABLE_MS : maxAge)) return Promise.resolve(data);
    pending ||= api
      .request<{ available?: boolean; models?: Record<string, ModelLoad> }>("/api/models/load")
      .then((r) => {
        data = { available: !!r?.available, models: r?.models || {} };
        listeners.forEach((l) => l());
        return data;
      })
      .catch(() => data)
      .finally(() => {
        fetchedAt = Date.now();
        pending = null;
      });
    return pending;
  }
  /** The tone that tints a model's icon while it is busy, full or has no servers. */
  function tint(id: string): "" | "busy" | "full" | "offline" {
    const s = of(id)?.state;
    return s === "busy" || s === "full" || s === "offline" ? s : "";
  }
  /** A model's load in words while it isn't free, for its tooltip and screen readers. */
  const label = (id: string) => loadLabel(of(id));
  /** What holds a reply that hasn't started: the person's own limit in its model, or a full model. */
  function holdup(model: string): { own?: boolean; text: string } | null {
    const l = of(model),
      you = l?.you;
    if (you?.limit && you.inFlight >= you.limit)
      return { own: true, text: `You have ${you.inFlight} of ${you.limit} requests running on ${modelName(model)}, so this one starts when one of them finishes.` };
    if (l?.state === "full") return { text: `${modelName(model)} is full right now: ${running(l)}${l.waiting ? `, ${l.waiting} waiting` : ""}. Your message is in line.` };
    if (l?.state === "offline") return { text: `${modelName(model)} has no servers running right now. Your message waits until one starts.` };
    return null;
  }
  /** Another model to send the message to: a favorite or the default that isn't full, else Auto. */
  function alternative(model: string, needsVision: boolean): string {
    const { models, preferences } = state();
    const usable = (id?: string) =>
      !!id &&
      id !== model &&
      models.some((m) => m.id === id && (id === "auto" || !needsVision || m.metadata?.supportsVision)) &&
      (id === "auto" || ["free", "busy"].includes(of(id)?.state || ""));
    return [...(preferences?.favorites || []), preferences?.defaultModel, "auto"].find(usable) || "";
  }
  /**
   * The notice under a reply that waits for a slot, or that the gateway stopped waiting
   * for: what holds it, and a model to switch to. Null when nothing is known to hold it.
   */
  function notice(m: Message, { waitingSince = 0, needsVision = false } = {}): LoadNotice | null {
    const model = m.settings?.model;
    if (!model || model === "auto" || !data.available) return null;
    const gaveUp = m.status === "error" && /stopped waiting/.test(m.errorMessage || "");
    const waiting = !gaveUp && !!waitingSince && Date.now() - waitingSince >= WAIT_NOTICE_MS;
    const held = waiting ? holdup(model) : null;
    if (!gaveUp && !held) return null;
    const next = held?.own ? "" : alternative(model, needsVision);
    return {
      text: gaveUp ? `${modelName(model)} stayed full.` : held!.text,
      waiting,
      next,
      label: next === "auto" ? "Let Auto choose" : `Switch to ${modelName(next)}`,
    };
  }
  /** Whether load is worth asking for while a reply waits. */
  const offered = () => data.available !== false;
  return {
    refresh,
    tint,
    label,
    notice,
    offered,
    current: () => data,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
export type Load = ReturnType<typeof createLoad>;
