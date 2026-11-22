"use client";

// The project log: one entry a day of the person's own work in a project. Endpoints in
// api/src/project_log/mod.rs.

import { useQuery, type QueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";

export type RefKind = "chat" | "run" | "commit";
export interface LogRef {
  kind: RefKind | string;
  id: string;
  label: string;
  /** A chat that is no longer there. */
  gone?: boolean;
}
export interface LogLine {
  id: string;
  kind: "step" | "decision" | "open";
  /** HH:MM in the day's time zone. */
  time: string | null;
  at: string | null;
  text: string;
  refs: LogRef[];
  edited: boolean;
}
export interface LogNote {
  id: string;
  text: string;
  time: string;
  at: string;
  /** Written on the day it is about. */
  sameDay: boolean;
}
export type DayState = "writing" | "written" | "waiting";
export interface LogDay {
  day: string;
  timeZone: string;
  headline: string;
  state: DayState;
  failed: boolean;
  /** There is newer work than the entry includes. */
  newer: boolean;
  writtenAt: string | null;
  model: string | null;
  lines: LogLine[];
  open: LogLine[];
  notes: LogNote[];
}
export interface DaySummary {
  day: string;
  headline: string;
  state: DayState;
  failed: boolean;
  decisions: number;
  notes: number;
}
export interface DayList {
  items: DaySummary[];
  more: boolean;
  enabled: boolean;
  timeZone: string | null;
}
export interface Week {
  from: string;
  to: string;
  timeZone: string;
  daysWithWork: number;
  chats: number;
  runs: number;
  decisions: number;
  notes: number;
}

const enc = encodeURIComponent;
export const logPath = (projectId: string) => `/api/projects/${enc(projectId)}/log`;
export const dayPath = (projectId: string, day: string) => `${logPath(projectId)}/days/${enc(day)}`;
export const logKeys = {
  all: (projectId: string) => ["project-log", projectId] as const,
  days: (projectId: string, limit: number) => ["project-log", projectId, "days", limit] as const,
  day: (projectId: string, day: string) => ["project-log", projectId, "day", day] as const,
  week: (projectId: string, day: string) => ["project-log", projectId, "week", day] as const,
};

/** Asks again every few seconds while a day is being written. */
const whileWriting = (writing: boolean) => (writing ? 4000 : false);

export function useLogDays(projectId: string, limit: number) {
  return useQuery({
    queryKey: logKeys.days(projectId, limit),
    queryFn: () => api.request<DayList>(`${logPath(projectId)}?limit=${limit}`),
    refetchInterval: (q) => whileWriting(!!q.state.data?.items.some((d) => d.state === "writing")),
  });
}

export function useLogDay(projectId: string, day: string | null) {
  return useQuery({
    queryKey: logKeys.day(projectId, day || ""),
    queryFn: async () => (await api.request<{ day: LogDay }>(dayPath(projectId, day!))).day,
    enabled: !!day,
    refetchInterval: (q) => whileWriting(q.state.data?.state === "writing"),
  });
}

export function useLogWeek(projectId: string, day: string | null) {
  return useQuery({
    queryKey: logKeys.week(projectId, day || ""),
    queryFn: async () => (await api.request<{ week: Week }>(`${logPath(projectId)}/week${day ? `?day=${enc(day)}` : ""}`)).week,
  });
}

export const refreshLog = (client: QueryClient, projectId: string) => client.invalidateQueries({ queryKey: logKeys.all(projectId) });

export const writeAgain = (projectId: string, day: string) => api.request<{ day: LogDay }>(`${dayPath(projectId, day)}/write`, { method: "POST" });
export const addNote = (projectId: string, day: string, text: string) => api.request<{ day: LogDay }>(`${dayPath(projectId, day)}/notes`, { method: "POST", body: { text } });
export const editNote = (projectId: string, day: string, note: string, text: string) =>
  api.request<{ day: LogDay }>(`${dayPath(projectId, day)}/notes/${enc(note)}`, { method: "PATCH", body: { text } });
export const deleteNote = (projectId: string, day: string, note: string) => api.request(`${dayPath(projectId, day)}/notes/${enc(note)}`, { method: "DELETE" });
export const editLine = (projectId: string, day: string, line: string, text: string) =>
  api.request<{ day: LogDay }>(`${dayPath(projectId, day)}/lines/${enc(line)}`, { method: "PATCH", body: { text } });
export const exportLog = (projectId: string, from: string, to: string) =>
  api.request<{ markdown: string; filename: string; days: number }>(`${logPath(projectId)}/export?from=${enc(from)}&to=${enc(to)}`);

/** The person's own switch and the time zone their days are cut in. */
export interface LogChoice {
  available: boolean;
  enabled: boolean;
  timeZone: string | null;
}
export const saveLogChoice = (body: { enabled?: boolean; timeZone?: string }) => api.request<LogChoice>("/api/log/settings", { method: "PUT", body });

/** The browser's time zone, such as Europe/Berlin, or "" when it does not say. */
export function browserZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch {
    return "";
  }
}

/** A day as YYYY-MM-DD, from a date's own calendar fields. */
export function isoDay(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}
/** A YYYY-MM-DD day as a local date at noon, clear of time-zone edges. */
export const dateOf = (day: string) => new Date(`${day}T12:00:00`);
export const addDays = (day: string, n: number) => {
  const d = dateOf(day);
  d.setDate(d.getDate() + n);
  return isoDay(d);
};
/** The Monday of the week that holds `day`. */
export const mondayOf = (day: string) => addDays(day, -((dateOf(day).getDay() + 6) % 7));
/** "Tuesday 17 November" */
export const longDay = (day: string) => dateOf(day).toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long" });
/** "Tue 17" */
export const shortDay = (day: string) => dateOf(day).toLocaleDateString("en-GB", { weekday: "short", day: "numeric" });
/** "November 2026" */
export const monthOf = (day: string) => dateOf(day).toLocaleDateString("en-GB", { month: "long", year: "numeric" });

/** Where a reference opens: the chat, the research run, or the commit in the code's history. */
export function refHref(projectId: string, ref: LogRef): string | null {
  const project = `/projects/${enc(projectId)}`;
  if (ref.kind === "chat") return ref.gone ? null : `/chat/${enc(ref.id)}`;
  if (ref.kind === "run") return `${project}/research?run=${enc(ref.id)}`;
  if (ref.kind === "commit") return `${project}/code/history/${enc(ref.id)}`;
  return null;
}
/** How a reference chip reads: "chat: Decode sweep", "run 3", "a41c9e2". */
export function refText(ref: LogRef): string {
  if (ref.kind === "chat") return `chat: ${ref.label}`;
  if (ref.kind === "run") return ref.label.toLowerCase();
  return ref.label;
}

/** The requests "Write from the log" starts a chat with, for the days from `from` to `to`. */
export function logPrompts(from: string, to: string, monday: string) {
  const range = (a: string, b: string) => `from ${a} to ${b}`;
  return {
    weekly: `Write a short weekly update for my advisor from my project log ${range(monday, to)}. Read it with the project_log tool first. Say what I did, what I found, what I decided and what is still open, in plain sentences, and name the days.`,
    methods: `Draft a methods section from my project log ${range(from, to)}. Read it with the project_log tool first. Describe what was run, on what and with which settings, only as the log states it, and mark anything I should check.`,
    aiUse: `Write an AI-use statement for a journal from my project log ${range(from, to)}. Read it with the project_log tool first. Say what the AI models did in this project and what I did myself, plainly and without overstating either.`,
  };
}
