"use client";

// A project's board (api/src/board/mod.rs): its cards, the queries the Board tab
// shares, and the changes it makes.

import { useQuery, type QueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";

export type CardStatus = "suggested" | "idea" | "next" | "doing" | "waiting" | "done" | "dismissed";
export type StepState = "todo" | "now" | "done";
export interface Step {
  text: string;
  state: StepState;
}
export interface CardLink {
  kind: string;
  label: string;
  ref: string;
}
export interface Person {
  id: string;
  name: string;
}
export interface Card {
  id: string;
  projectId: string;
  title: string;
  notes: string;
  kind: string;
  status: CardStatus;
  position: number;
  madeBy: "person" | "assistant";
  maker: Person | null;
  worker: "person" | "assistant" | null;
  workerUser: Person | null;
  /** The chat working the card. Its title shows only to its owner; whether it is writing shows to everyone. */
  chat: { id: string; mine: boolean; title: string | null; working: boolean; waiting: boolean } | null;
  sourceChat: { id: string; mine: boolean; title: string | null } | null;
  steps: Step[];
  progress: string;
  question: string;
  options: string[];
  answer: string;
  answeredAt: string | null;
  links: CardLink[];
  createdAt: string;
  updatedAt: string;
  startedAt: string | null;
  doneAt: string | null;
}
export interface Board {
  /** The open columns in order, then the latest done cards. */
  cards: Card[];
  doneCount: number;
}
export interface HistoryEvent {
  action: "made" | "moved" | "picked" | "asked" | "answered" | "done";
  actor: "person" | "assistant";
  actorId: string | null;
  actorName: string | null;
  from: CardStatus | null;
  to: CardStatus | null;
  detail: string;
  at: string;
}
/** What a card can be changed by; each field is left as it is when absent. */
export interface CardChange {
  title?: string;
  notes?: string;
  kind?: string;
  status?: CardStatus;
  position?: number;
  steps?: Step[];
  progress?: string;
  worker?: "me" | "assistant" | "none";
  answer?: string;
  question?: string;
  options?: string[];
  chatId?: string;
}

/** The limits the API keeps to (api/src/board/mod.rs). */
export const LIMITS = { title: 200, notes: 4000, kind: 40, steps: 12, step: 200 } as const;
/** Done cards the board shows before "n more". */
export const DONE_SHOWN = 5;

export const COLUMN_NAMES: Record<CardStatus, string> = {
  suggested: "Ideas",
  idea: "Ideas",
  next: "Up next",
  doing: "Now",
  waiting: "Needs you",
  done: "Done",
  dismissed: "Dismissed",
};

export const boardKey = (projectId: string) => ["project-board", projectId] as const;
export const cardKey = (projectId: string, cardId: string) => ["project-card", projectId, cardId] as const;
const boardPath = (projectId: string) => `/api/projects/${encodeURIComponent(projectId)}/cards`;
export const cardPath = (projectId: string, cardId: string) => `${boardPath(projectId)}/${encodeURIComponent(cardId)}`;

/** A card is alive while the assistant is on it and its chat is writing a reply. */
export const alive = (card: Card) => card.status === "doing" && card.worker === "assistant" && !!card.chat?.working;

/** Read closely while something on the board is alive, now and then otherwise. */
export const boardRefetch = (board: Board | undefined) => (board?.cards.some((c) => c.chat?.working) ? 5_000 : 20_000);

export function useBoard(projectId: string) {
  return useQuery({
    queryKey: boardKey(projectId),
    queryFn: () => api.request<Board>(boardPath(projectId)),
    refetchInterval: (query) => boardRefetch(query.state.data),
  });
}

export function useCard(projectId: string, cardId: string | null) {
  return useQuery({
    queryKey: cardKey(projectId, cardId || ""),
    queryFn: () => api.request<{ card: Card; history: HistoryEvent[] }>(cardPath(projectId, cardId!)),
    enabled: !!cardId,
    refetchInterval: (query) => (query.state.data?.card.chat?.working ? 5_000 : 20_000),
  });
}

/** Every done card, newest first, `limit` at a time. */
export function useDoneCards(projectId: string, limit: number, enabled: boolean) {
  return useQuery({
    queryKey: [...boardKey(projectId), "done", limit],
    queryFn: () => api.request<{ items: Card[]; more: boolean }>(`${boardPath(projectId)}?status=done&limit=${limit}`),
    enabled,
  });
}

export function refreshBoard(client: QueryClient, projectId: string) {
  client.invalidateQueries({ queryKey: boardKey(projectId) });
  client.invalidateQueries({ queryKey: ["project-card", projectId] });
}

export async function createCard(projectId: string, body: { title: string; kind?: string; notes?: string; status?: "idea" | "next" }) {
  return (await api.request<{ card: Card }>(boardPath(projectId), { method: "POST", body })).card;
}

export async function changeCard(projectId: string, cardId: string, body: CardChange) {
  return (await api.request<{ card: Card }>(cardPath(projectId, cardId), { method: "PATCH", body })).card;
}

export function deleteCard(projectId: string, cardId: string) {
  return api.request(cardPath(projectId, cardId), { method: "DELETE" });
}

/** The board's cards by column: each open column in order, done newest first. */
export function columns(cards: Card[]) {
  const by = (status: CardStatus) => cards.filter((c) => c.status === status).sort((a, b) => a.position - b.position);
  return {
    now: by("doing"),
    // Suggestions sit at the top of Ideas until they are kept or dismissed.
    ideas: [...by("suggested"), ...by("idea")],
    next: by("next"),
    waiting: by("waiting"),
    done: cards.filter((c) => c.status === "done").sort((a, b) => String(b.doneAt || "").localeCompare(String(a.doneAt || ""))),
  };
}

/** Who is on a card, for its label: "The assistant", "You" or a name. */
export function workerName(card: Card, me: string) {
  if (card.worker === "assistant") return "The assistant";
  if (card.worker === "person") return card.workerUser?.id === me ? "You" : card.workerUser?.name || "Someone";
  return "";
}

/** "3 of 5 steps done", or "". */
export function stepsLine(steps: Step[]) {
  if (!steps.length) return "";
  const done = steps.filter((s) => s.state === "done").length;
  return `${done} of ${steps.length} ${steps.length === 1 ? "step" : "steps"} done`;
}
