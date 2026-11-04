"use client";

// Administration › Feedback: what people sent with the sidebar's Feedback button, and
// what they thought of replies, per model. Ratings show no conversation or person; sent
// feedback names its sender only when they said they may be contacted.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { FEEDBACK_KINDS } from "@/components/app-shell/feedback-dialog";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { Chips, Note, Pill, Section, TableWrap, td, th } from "@/components/manage/kit";
import { api } from "@/lib/api";
import { keys } from "@/lib/queries";
import { when } from "@/lib/format";
import { useKept } from "./state";

/** Why a reply was not good; the keys are what the server stores (feedback::REASONS). */
export const REASONS = [
  ["wrong", "Wrong or made up"],
  ["instructions", "Didn’t follow instructions"],
  ["too_long", "Too long"],
  ["too_short", "Too short"],
  ["harmful", "Harmful or offensive"],
  ["other", "Other"],
] as const;
export const reasonLabel = (key: string) => REASONS.find(([k]) => k === key)?.[1] || key;

export interface FeedbackSummary {
  days?: number;
  models: { model: string; up: number; down: number; upShare: number }[];
  reasons: { reason: string; count: number }[];
  comments: { model: string; rating: number; reasons?: string[]; comment: string; at: string }[];
}
const PERIODS = [
  [7, "7 days"],
  [30, "30 days"],
  [90, "90 days"],
] as const;

export function FeedbackView({ data: f, days }: { data: FeedbackSummary; days: number }) {
  if (!f.models.length) return <EmptyState title="No feedback yet" text={`Nobody rated a reply in the last ${days} days.`} />;
  return (
    <>
      <TableWrap>
        <table className="w-full min-w-[480px]">
          <thead className="border-b bg-muted/40">
            <tr>
              <th className={th}>Model</th>
              <th className={th}>Up</th>
              <th className={th}>Down</th>
              <th className={th}>Share up</th>
            </tr>
          </thead>
          <tbody>
            {f.models.map((m) => (
              <tr key={m.model} className="border-b border-line-soft last:border-0">
                <td className={td}>
                  <strong className="font-medium">{m.model}</strong>
                </td>
                <td className={td}>{Number(m.up).toLocaleString()}</td>
                <td className={td}>{Number(m.down).toLocaleString()}</td>
                <td className={td}>{Math.round(m.upShare * 100)}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableWrap>
      <div className="grid gap-4 lg:grid-cols-2">
        <Section title="Why replies got a thumbs down">
          {f.reasons.length ? (
            <ul className="grid gap-1.5">
              {f.reasons.map((r) => (
                <li key={r.reason} className="flex justify-between gap-3 text-sm">
                  <span>{reasonLabel(r.reason)}</span>
                  <strong className="font-medium tabular-nums">{Number(r.count).toLocaleString()}</strong>
                </li>
              ))}
            </ul>
          ) : (
            <Note>No reasons given.</Note>
          )}
        </Section>
        <Section title="Recent comments">
          {f.comments.length ? (
            <ul className="grid gap-3">
              {f.comments.map((c, i) => (
                <li key={i} className="grid gap-0.5 border-l-2 pl-3">
                  <p className="text-md whitespace-pre-wrap">{c.comment}</p>
                  <small className="text-xs text-faint">{[c.model, c.rating > 0 ? "Thumbs up" : "Thumbs down", ...(c.reasons || []).map(reasonLabel), when(c.at)].join(" · ")}</small>
                </li>
              ))}
            </ul>
          ) : (
            <Note>No comments.</Note>
          )}
        </Section>
      </div>
    </>
  );
}

export interface SentFeedback {
  id: string;
  kind: "problem" | "idea" | "other";
  message: string;
  page?: string | null;
  version: string;
  done: boolean;
  at: string;
  from?: { username: string; name: string } | null;
}
export interface SentFeedbackList {
  items: SentFeedback[];
  open: Record<SentFeedback["kind"], number>;
}
type ButtonMode = "off" | "sidebar" | "highlighted";
const MODES = [
  ["highlighted", "Highlighted"],
  ["sidebar", "In the sidebar"],
  ["off", "Off"],
] as const;
const STATES = [
  ["open", "Open"],
  ["done", "Done"],
  ["all", "All"],
] as const;
const KIND_FILTERS = [["all", "All kinds"], ...FEEDBACK_KINDS] as const;
const kindLabel = (kind: string) => FEEDBACK_KINDS.find(([k]) => k === kind)?.[1] || kind;

function ButtonCard() {
  const queries = useQueryClient();
  const button = useQuery({ queryKey: ["admin", "feedback", "button"], queryFn: () => api.request<{ mode: ButtonMode }>("/api/admin/feedback/button") });
  const [busy, setBusy] = useState(false);
  const save = async (mode: ButtonMode) => {
    setBusy(true);
    try {
      const saved = await api.request<{ mode: ButtonMode }>("/api/admin/feedback/button", { method: "PUT", body: { mode } });
      queries.setQueryData(["admin", "feedback", "button"], saved);
      queries.invalidateQueries({ queryKey: keys.capabilities });
      toast(mode === "off" ? "Feedback button turned off" : "Feedback button saved");
    } catch (e) {
      toast((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Section
      title="Feedback button"
      description="Where people send a problem or an idea from. Highlighted makes it stand out at the bottom of the sidebar, as while Chat is in development."
      actions={button.data && <Chips label="Feedback button" options={MODES} value={button.data.mode} onChange={(mode) => !busy && save(mode)} />}
    >
      <ErrorBox error={button.error} onRetry={() => button.refetch()} />
    </Section>
  );
}

export function SentFeedbackView({ data, onDone }: { data: SentFeedbackList; onDone: (item: SentFeedback, done: boolean) => void }) {
  if (!data.items.length) return <EmptyState title="Nothing here" text="Feedback people send from the sidebar shows here." />;
  return (
    <ul className="grid gap-3">
      {data.items.map((f) => (
        <li key={f.id} className="grid gap-2 rounded-lg border bg-card p-4">
          <div className="flex flex-wrap items-center gap-2">
            <Pill tone={f.kind === "problem" ? "amber" : f.kind === "idea" ? "accent" : ""}>{kindLabel(f.kind)}</Pill>
            {f.done && <Pill>Done</Pill>}
            <small className="text-xs text-faint">{[f.from ? `${f.from.name} (${f.from.username})` : "Anonymous", f.page, `v${f.version}`, when(f.at)].filter(Boolean).join(" · ")}</small>
            <Button size="sm" variant="outline" className="ml-auto" onClick={() => onDone(f, !f.done)}>
              {f.done ? "Reopen" : "Mark done"}
            </Button>
          </div>
          <p className="text-md whitespace-pre-wrap">{f.message}</p>
        </li>
      ))}
    </ul>
  );
}

function SentFeedbackSection() {
  const queries = useQueryClient();
  const [state, setState] = useKept<"open" | "done" | "all">("feedback.sent.state", "open");
  const [kind, setKind] = useKept<"all" | SentFeedback["kind"]>("feedback.sent.kind", "all");
  const params = new URLSearchParams();
  if (state !== "all") params.set("done", String(state === "done"));
  if (kind !== "all") params.set("kind", kind);
  const sent = useQuery({ queryKey: ["admin", "feedback", "sent", state, kind], queryFn: () => api.request<SentFeedbackList>(`/api/admin/feedback/sent?${params}`), placeholderData: (old) => old });
  const mark = async (item: SentFeedback, done: boolean) => {
    try {
      await api.request(`/api/admin/feedback/sent/${encodeURIComponent(item.id)}`, { method: "PATCH", body: { done } });
      queries.invalidateQueries({ queryKey: ["admin", "feedback", "sent"] });
    } catch (e) {
      toast((e as Error).message);
    }
  };
  const open = sent.data?.open;
  return (
    <div className="grid gap-4">
      <ButtonCard />
      <div className="flex flex-wrap items-center gap-3">
        <Chips label="Status" options={STATES} value={state} onChange={setState} />
        <Chips label="Kind" options={KIND_FILTERS} value={kind} onChange={setKind} />
      </div>
      {open && (
        <Note>
          Open: {open.problem.toLocaleString()} {open.problem === 1 ? "problem" : "problems"}, {open.idea.toLocaleString()} {open.idea === 1 ? "idea" : "ideas"}, {open.other.toLocaleString()} other. A sender is named only when they said you may contact them.
        </Note>
      )}
      <ErrorBox error={sent.error} onRetry={() => sent.refetch()} />
      {sent.data ? <SentFeedbackView data={sent.data} onDone={mark} /> : !sent.error && <Loading label="Loading feedback…" />}
    </div>
  );
}

const VIEWS = [
  ["sent", "Sent feedback"],
  ["ratings", "Reply ratings"],
] as const;

export function FeedbackTab() {
  const [view, setView] = useKept<"sent" | "ratings">("feedback.view", "sent");
  return (
    <div className="grid gap-4">
      <Chips label="Feedback" options={VIEWS} value={view} onChange={setView} />
      {view === "sent" ? <SentFeedbackSection /> : <RatingsSection />}
    </div>
  );
}

function RatingsSection() {
  const [days, setDays] = useKept("feedback.days", 30);
  const feedback = useQuery({ queryKey: ["admin", "feedback", days], queryFn: () => api.request<FeedbackSummary>(`/api/admin/feedback?days=${days}`), placeholderData: (old) => old });
  const f = feedback.data;
  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Chips label="Period" options={PERIODS} value={days} onChange={setDays} />
        {!!f?.models.length && (
          <Button size="sm" variant="outline" asChild>
            <a href={`/api/admin/feedback/models.csv?days=${days}`} download>
              <Download /> Download CSV
            </a>
          </Button>
        )}
      </div>
      <Note>Thumbs up and down people gave replies in the last {days} days. You see the model, the rating, the reasons and any comment, never the conversation or who gave it.</Note>
      <ErrorBox error={feedback.error} onRetry={() => feedback.refetch()} />
      {f ? <FeedbackView data={f} days={days} /> : !feedback.error && <Loading label="Loading administration…" />}
    </div>
  );
}
