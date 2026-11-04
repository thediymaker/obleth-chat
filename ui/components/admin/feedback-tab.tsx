"use client";

// Administration › Feedback: what people thought of replies, per model. No conversation
// or person is shown: the server sends only ratings, reasons, comments, models and dates.

import { useQuery } from "@tanstack/react-query";
import { Download } from "lucide-react";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { Chips, Note, Section, TableWrap, td, th } from "@/components/manage/kit";
import { api } from "@/lib/api";
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

export function FeedbackTab() {
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
