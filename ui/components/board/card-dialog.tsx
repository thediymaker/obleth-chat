"use client";

// A card, open: its title, kind and notes to edit, its steps, the question it asks,
// what it is linked to and its history, with the card's actions at the foot.

import { ArrowDown, ArrowUp, Check, Hand, MessageSquare, Plus, X } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { ErrorBox, Eyebrow, Loading } from "@/components/app-shell/page";
import { DangerButton, Field, FormError, Pill } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { ago, when } from "@/lib/format";
import { useUser } from "@/lib/session";
import { cn } from "@/lib/utils";
import { type Card, type CardStatus, type HistoryEvent, type Step, COLUMN_NAMES, LIMITS, alive, useCard } from "./api";
import { Answer, type CardActions, StepMark, WhoMark } from "./card-parts";

export function CardDialog({ projectId, cardId, onClose, actions, onDelete }: { projectId: string; cardId: string | null; onClose: () => void; actions: CardActions; onDelete: (card: Card) => void }) {
  const { data, error, isPending, refetch } = useCard(projectId, cardId);
  return (
    <Dialog open={!!cardId} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[92dvh] overflow-y-auto sm:max-w-3xl" aria-describedby={undefined}>
        {data ? (
          <OpenCard key={data.card.id} card={data.card} history={data.history} actions={actions} onDelete={() => onDelete(data.card)} onClose={onClose} />
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>Card</DialogTitle>
            </DialogHeader>
            {isPending ? <Loading compact /> : <ErrorBox error={error} onRetry={() => refetch()} compact />}
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** Who did something, as the history says it. */
function actorName(e: HistoryEvent, me: string) {
  if (e.actor === "assistant") return "The assistant";
  return e.actorId === me ? "You" : e.actorName || "Someone";
}

/** One line of a card's history. */
export function historyText(e: HistoryEvent, me: string) {
  const who = actorName(e, me);
  const column = (s: CardStatus | null) => (s ? COLUMN_NAMES[s] : "");
  switch (e.action) {
    case "made":
      return e.to === "suggested" ? `${who} suggested it.` : `${who} made it in ${column(e.to)}.`;
    case "picked":
      return e.actor === "assistant" || (e.to === "doing" && !e.from) ? `${who} picked it up.` : `${who} picked it up${e.from ? ` from ${column(e.from)}` : ""}.`;
    case "asked":
      return `${who} asked: “${e.detail}”`;
    case "answered":
      return `${who} answered “${e.detail}”.`;
    case "done":
      return `${who} marked it done.`;
    default:
      if (e.from === "suggested" && e.to === "idea") return `${who} kept the suggestion.`;
      if (e.to === "dismissed") return `${who} dismissed it.`;
      return `${who} moved it from ${column(e.from)} to ${column(e.to)}.`;
  }
}

/** The line under the title: who made it, where it came from, who is on it. */
function provenance(card: Card, me: string) {
  const maker = card.madeBy === "assistant" ? "The assistant" : card.maker?.id === me ? "You" : card.maker?.name || "Someone";
  const from = card.sourceChat?.mine && card.sourceChat.title ? ` in the chat “${card.sourceChat.title}”` : card.sourceChat ? " in a chat" : "";
  return `${maker} made this card${from} ${ago(card.createdAt)}.`;
}

function OpenCard({ card, history, actions, onDelete, onClose }: { card: Card; history: HistoryEvent[]; actions: CardActions; onDelete: () => void; onClose: () => void }) {
  const user = useUser();
  const live = alive(card);
  const worker = card.worker === "assistant" ? "The assistant" : card.worker === "person" ? (card.workerUser?.id === user.id ? "You" : card.workerUser?.name || "Someone") : "";
  const column = card.status === "suggested" ? "idea" : card.status;
  return (
    <>
      <DialogHeader className="gap-2 pr-8">
        <div className="flex flex-wrap items-center gap-2 text-2xs font-medium tracking-wider text-muted-foreground uppercase">
          {card.kind && <span>{card.kind}</span>}
          {live ? (
            <span className="inline-flex h-5 items-center gap-1.5 rounded-full bg-signal/10 px-2 text-xs font-medium tracking-normal text-signal normal-case">
              <span className="relative inline-flex size-1.5" aria-hidden>
                <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />
                <span className="absolute inset-0 rounded-full bg-signal" />
              </span>
              {card.chat?.waiting ? "Waiting in its chat" : "In progress"}
            </span>
          ) : (
            <Pill className="tracking-normal normal-case">{card.status === "suggested" ? "Suggested" : COLUMN_NAMES[card.status]}</Pill>
          )}
        </div>
        <DialogTitle className="text-xl leading-snug">{card.title}</DialogTitle>
        <DialogDescription>
          {provenance(card, user.id)}
          {worker && card.status === "doing" && ` ${worker} ${worker === "You" ? "are" : "is"} on it.`}
        </DialogDescription>
      </DialogHeader>

      <div className="grid gap-6 md:grid-cols-[minmax(0,1fr)_240px]">
        <div className="grid content-start gap-6">
          {card.status === "waiting" && card.question && (
            <section className="grid gap-2 rounded-lg border p-3">
              <Eyebrow>Needs you</Eyebrow>
              <Answer card={card} actions={actions} />
            </section>
          )}
          {card.answer && card.status !== "waiting" && (
            <p className="text-sm text-muted-foreground">
              Asked “{card.question}” and got “{card.answer}”{card.answeredAt ? ` ${ago(card.answeredAt)}` : ""}.
            </p>
          )}
          <Details card={card} actions={actions} />
          <Steps card={card} actions={actions} live={live} />
          {card.progress && (
            <section className="grid gap-1">
              <Eyebrow>Progress</Eyebrow>
              <p className={cn("text-md", live ? "text-signal" : "text-text-2")}>{card.progress}</p>
            </section>
          )}
        </div>
        <aside aria-label="Linked and history" className="grid content-start gap-5 text-md">
          <section className="grid gap-2">
            <Eyebrow>Linked</Eyebrow>
            <LinkRow label="Chat">
              {card.chat ? (
                card.chat.mine ? (
                  <Link href={`/chat/${encodeURIComponent(card.chat.id)}`} className="hover:underline" onClick={onClose}>
                    {card.chat.title || "Its chat"}
                  </Link>
                ) : (
                  <span className="text-muted-foreground">{card.workerUser?.name ? `${card.workerUser.name}’s chat` : "Someone else’s chat"}</span>
                )
              ) : (
                <span className="text-muted-foreground">None yet</span>
              )}
            </LinkRow>
            {card.sourceChat && card.sourceChat.id !== card.chat?.id && (
              <LinkRow label="Made in">
                {card.sourceChat.mine ? (
                  <Link href={`/chat/${encodeURIComponent(card.sourceChat.id)}`} className="hover:underline" onClick={onClose}>
                    {card.sourceChat.title || "A chat"}
                  </Link>
                ) : (
                  <span className="text-muted-foreground">Someone else’s chat</span>
                )}
              </LinkRow>
            )}
            {card.links.map((l, i) => (
              <LinkRow key={i} label={l.kind}>
                <span className="font-mono text-sm">{l.label || l.ref}</span>
              </LinkRow>
            ))}
            {card.worker && (
              <LinkRow label="On it">
                <span className="inline-flex items-center gap-1.5">
                  <WhoMark assistant={card.worker === "assistant"} name={card.workerUser?.name || ""} label={worker} />
                  {worker}
                </span>
              </LinkRow>
            )}
          </section>
          <section className="grid gap-2">
            <Eyebrow>History</Eyebrow>
            <ol className="grid gap-2">
              {[...history].reverse().map((e, i) => (
                <li key={i} className="grid gap-0.5 text-sm">
                  <span className="text-text-2">{historyText(e, user.id)}</span>
                  <time dateTime={e.at} title={when(e.at)} className="font-mono text-xs text-muted-foreground">
                    {ago(e.at)}
                  </time>
                </li>
              ))}
            </ol>
          </section>
        </aside>
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t pt-4">
        {card.chat?.mine && (
          <Button variant="outline" onClick={() => actions.openChat(card)}>
            <MessageSquare /> Open its chat
          </Button>
        )}
        {card.status !== "done" && (
          <>
            {!(card.status === "doing" && card.worker === "assistant" && card.chat?.mine) && <Button onClick={() => actions.start(card)}>Start with the assistant</Button>}
            {!(card.status === "doing" && card.worker === "person" && card.workerUser?.id === user.id) && (
              <Button variant="outline" onClick={() => actions.take(card)}>
                <Hand /> I’m on it
              </Button>
            )}
          </>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline">Move to…</Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {(["idea", "next", "waiting", "done"] as CardStatus[])
              .filter((t) => t !== column)
              .map((t) => (
                <DropdownMenuItem key={t} onSelect={() => actions.move(card, t)}>
                  {COLUMN_NAMES[t]}
                </DropdownMenuItem>
              ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <span className="grow" />
        {card.status !== "done" && (
          <Button variant="outline" onClick={() => actions.change(card, { status: "done" }, "Marked done", { status: card.status === "suggested" ? "idea" : card.status })}>
            <Check /> Mark done
          </Button>
        )}
        <DangerButton size="default" onClick={onDelete}>
          Delete
        </DangerButton>
      </div>
    </>
  );
}

function LinkRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex min-w-0 items-baseline gap-3">
      <span className="w-16 shrink-0 text-xs text-muted-foreground capitalize">{label}</span>
      <span className="min-w-0 break-words">{children}</span>
    </div>
  );
}

/** Title, kind and notes, saved together. A change made elsewhere shows unless this person is editing. */
function Details({ card, actions }: { card: Card; actions: CardActions }) {
  const [saved, setSaved] = useState({ title: card.title, kind: card.kind, notes: card.notes });
  const [title, setTitle] = useState(card.title);
  const [kind, setKind] = useState(card.kind);
  const [notes, setNotes] = useState(card.notes);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const dirty = title !== saved.title || kind !== saved.kind || notes !== saved.notes;
  if (card.title !== saved.title || card.kind !== saved.kind || card.notes !== saved.notes) {
    setSaved({ title: card.title, kind: card.kind, notes: card.notes });
    if (!dirty) {
      setTitle(card.title);
      setKind(card.kind);
      setNotes(card.notes);
    }
  }
  return (
    <form
      className="grid gap-3"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!title.trim()) return setError("Give the card a title");
        setBusy(true);
        setError("");
        await actions.change(card, { title, kind, notes }, "Card saved");
        setBusy(false);
      }}
    >
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_160px]">
        <Field id="card-edit-title" label="Title">
          <Input id="card-edit-title" value={title} maxLength={LIMITS.title} onChange={(e) => setTitle(e.target.value)} />
        </Field>
        <Field id="card-edit-kind" label="Kind">
          <Input id="card-edit-kind" value={kind} maxLength={LIMITS.kind} placeholder="Writing" onChange={(e) => setKind(e.target.value)} />
        </Field>
      </div>
      <Field id="card-edit-notes" label="Notes">
        <Textarea id="card-edit-notes" value={notes} rows={4} maxLength={LIMITS.notes} placeholder="What it is for, and what done looks like. The assistant reads this when it works the card." onChange={(e) => setNotes(e.target.value)} />
      </Field>
      <FormError message={error} />
      {dirty && (
        <div className="flex justify-end gap-2">
          <Button
            type="button"
            variant="ghost"
            onClick={() => {
              setTitle(saved.title);
              setKind(saved.kind);
              setNotes(saved.notes);
            }}
          >
            Cancel
          </Button>
          <Button type="submit" disabled={busy}>
            Save
          </Button>
        </div>
      )}
    </form>
  );
}

const NEXT_STATE: Record<Step["state"], Step["state"]> = { todo: "now", now: "done", done: "todo" };
const STATE_WORDS: Record<Step["state"], string> = { todo: "to do", now: "being worked on", done: "done" };

/** The card's steps: each change saves at once. */
function Steps({ card, actions, live }: { card: Card; actions: CardActions; live: boolean }) {
  const [text, setText] = useState("");
  const steps = card.steps;
  const save = (next: Step[], done?: string) => actions.change(card, { steps: next }, done);
  const full = steps.length >= LIMITS.steps;
  return (
    <section className="grid gap-2" aria-labelledby="card-steps">
      <Eyebrow>
        <span id="card-steps">Steps</span>
      </Eyebrow>
      {steps.length > 0 && (
        <ol className="grid gap-1">
          {steps.map((s, i) => (
            <li key={i} className="group flex items-start gap-2 text-md">
              <button
                type="button"
                className="mt-0.5 rounded-full"
                aria-label={`Step ${i + 1}, ${STATE_WORDS[s.state]}: mark it ${STATE_WORDS[NEXT_STATE[s.state]]}`}
                title={`Mark it ${STATE_WORDS[NEXT_STATE[s.state]]}`}
                onClick={() => save(steps.map((x, j) => (j === i ? { ...x, state: NEXT_STATE[x.state] } : x)))}
              >
                <StepMark state={s.state} live={live} />
              </button>
              <span className={cn("min-w-0 grow", s.state === "todo" ? "text-muted-foreground" : s.state === "done" ? "text-text-2" : "text-foreground")}>{s.text}</span>
              <span className="flex shrink-0 gap-0.5 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100">
                {i > 0 && (
                  <Button variant="ghost" size="icon-xs" aria-label={`Move step ${i + 1} up`} onClick={() => save(swap(steps, i, i - 1))}>
                    <ArrowUp />
                  </Button>
                )}
                {i < steps.length - 1 && (
                  <Button variant="ghost" size="icon-xs" aria-label={`Move step ${i + 1} down`} onClick={() => save(swap(steps, i, i + 1))}>
                    <ArrowDown />
                  </Button>
                )}
                <Button variant="ghost" size="icon-xs" aria-label={`Remove step ${i + 1}`} onClick={() => save(steps.filter((_, j) => j !== i))}>
                  <X />
                </Button>
              </span>
            </li>
          ))}
        </ol>
      )}
      {!full && (
        <form
          className="flex gap-1.5"
          onSubmit={async (e) => {
            e.preventDefault();
            if (!text.trim()) return;
            if (await save([...steps, { text: text.trim(), state: "todo" }])) setText("");
          }}
        >
          <Input aria-label="Add a step" placeholder="Add a step" value={text} maxLength={LIMITS.step} onChange={(e) => setText(e.target.value)} className="h-8" />
          <Button type="submit" size="sm" variant="outline" disabled={!text.trim()}>
            <Plus /> Add
          </Button>
        </form>
      )}
      {full && <p className="text-xs text-muted-foreground">A card has up to 12 steps.</p>}
    </section>
  );
}

function swap(steps: Step[], a: number, b: number) {
  const next = [...steps];
  [next[a], next[b]] = [next[b]!, next[a]!];
  return next;
}
