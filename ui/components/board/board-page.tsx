"use client";

// A project's Board tab: the cards the people in the project and the assistant make
// and work through. The Now row on top shows what is being worked on; below it the
// columns Ideas, Up next (in order), Needs you and Done. Cards move by drag and by
// their menu; Up next reorders the same ways. The accent marks only a card whose
// chat is writing a reply right now.

import { useQueryClient } from "@tanstack/react-query";
import { Plus, SquareKanban } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { FormActions, FormDialog, FormError, SelectField, TextAreaField, TextField, messageOf, useConfirm, useSubmit } from "@/components/manage/kit";
import { useProjectFrame } from "@/components/projects/project-frame";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ago, count } from "@/lib/format";
import { useUser } from "@/lib/session";
import { cn } from "@/lib/utils";
import { type Card, type CardStatus, DONE_SHOWN, LIMITS, alive, columns, createCard, refreshBoard, stepsLine, useBoard, useDoneCards, workerName } from "./api";
import { Answer, type CardActions, CardMenu, StepList, WhoMark, useCardActions } from "./card-parts";
import { CardDialog } from "./card-dialog";

/** The card being dragged, read by the drop targets. */
const DRAG_TYPE = "application/x-obleth-card";

export function ProjectBoard() {
  const { detail } = useProjectFrame();
  const projectId = detail.project.id;
  const { data, error, isPending, refetch } = useBoard(projectId);
  const actions = useCardActions(projectId);
  const confirm = useConfirm();
  const [open, setOpen] = useState<string | null>(null);
  const [making, setMaking] = useState(false);
  const [dragging, setDragging] = useState<Card | null>(null);

  if (!data) return isPending ? <Loading /> : <ErrorBox error={error} onRetry={() => refetch()} />;
  const cols = columns(data.cards);
  const empty = data.cards.length === 0;
  const askDelete = (card: Card) =>
    confirm.ask({
      title: "Delete this card?",
      text: `“${card.title}” and its history are deleted for everyone in the project. Its chat stays.`,
      label: "Delete card",
      action: async () => {
        await actions.remove(card);
        if (open === card.id) setOpen(null);
      },
    });
  const ctx: BoardContext = { projectId, actions, open: setOpen, askDelete, dragging, setDragging };

  return (
    <div className="grid gap-6" data-board>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-2xl text-sm text-muted-foreground">What this project needs doing. You and the assistant both add cards and move them along; start one with the assistant and its chat keeps the card up to date.</p>
        <Button onClick={() => setMaking(true)}>
          <Plus /> New card
        </Button>
      </div>
      {empty && (
        <EmptyState
          icon={SquareKanban}
          title="Nothing on the board yet"
          text="Add an idea below, or a card for what should happen next. Chats in this project can add cards too, and keep the ones they work on up to date."
        />
      )}
      {!empty && <NowRow cards={cols.now} ctx={ctx} />}
      <div className="-mx-1 overflow-x-auto px-1 pb-2" role="group" aria-label="Board columns">
        <div className="grid min-w-[920px] grid-cols-4 items-start gap-3">
          <Column ctx={ctx} status="idea" name="Ideas" hint="Yours and the assistant’s. Nothing here runs." cards={cols.ideas}>
            <AddIdea projectId={projectId} />
          </Column>
          <Column ctx={ctx} status="next" name="Up next" hint="In order. Drag, or use a card’s menu, to reorder." cards={cols.next} numbered />
          <Column ctx={ctx} status="waiting" name="Needs you" hint="Waiting on an answer from you." cards={cols.waiting} />
          <DoneColumn ctx={ctx} cards={cols.done} total={data.doneCount} />
        </div>
      </div>
      <NewCardDialog projectId={projectId} open={making} onOpenChange={setMaking} />
      <CardDialog projectId={projectId} cardId={open} onClose={() => setOpen(null)} actions={actions} onDelete={askDelete} />
      {confirm.element}
    </div>
  );
}

interface BoardContext {
  projectId: string;
  actions: CardActions;
  open: (id: string) => void;
  askDelete: (card: Card) => void;
  dragging: Card | null;
  setDragging: (card: Card | null) => void;
}

/** Where a dragged card goes when it is dropped on a column: `before` names the card it lands above. */
function drop(ctx: BoardContext, status: CardStatus, list: Card[], before?: Card) {
  const card = ctx.dragging;
  ctx.setDragging(null);
  if (!card || card.id === before?.id) return;
  // Onto Now: the person takes it.
  if (status === "doing") {
    if (card.status !== "doing") void ctx.actions.take(card);
    return;
  }
  const others = list.filter((c) => c.id !== card.id && c.status === status);
  const index = before ? others.findIndex((c) => c.id === before.id) : -1;
  if (index >= 0) void ctx.actions.move(card, status, index);
  else if (card.status === status) void ctx.actions.move(card, status, others.length);
  else void ctx.actions.move(card, status);
}

function dropTarget(ctx: BoardContext, onDrop: () => void) {
  return {
    onDragOver: (e: React.DragEvent) => {
      if (ctx.dragging) e.preventDefault();
    },
    onDrop: (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      onDrop();
    },
  };
}

function NowRow({ cards, ctx }: { cards: Card[]; ctx: BoardContext }) {
  const user = useUser();
  const live = cards.some(alive);
  const mine = cards.filter((c) => c.worker === "person" && c.workerUser?.id === user.id).length;
  const assistant = cards.filter((c) => c.worker === "assistant").length;
  const summary = [assistant && `The assistant is on ${count(assistant, "card")}`, mine && `you are on ${count(mine, "card")}`].filter(Boolean).join(" and ");
  return (
    <section aria-labelledby="board-now" className="grid gap-3" {...dropTarget(ctx, () => drop(ctx, "doing", cards))}>
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h2 id="board-now" className="flex items-center gap-2 text-md font-semibold">
          {live && (
            <span className="relative inline-flex size-2 shrink-0" aria-hidden>
              <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />
              <span className="absolute inset-0 rounded-full bg-signal" />
            </span>
          )}
          Now <span className="font-mono text-xs font-normal text-muted-foreground">{cards.length}</span>
        </h2>
        <span className="text-sm text-muted-foreground">{cards.length ? `${summary ? `${summary[0]!.toUpperCase()}${summary.slice(1)}.` : "Being worked on."}` : "Nothing is being worked on. Start a card with the assistant, or take one yourself."}</span>
      </div>
      {cards.length > 0 && (
        <div className="grid grid-cols-[repeat(auto-fit,minmax(260px,1fr))] gap-3">
          {cards.map((card) => (
            <NowCard key={card.id} card={card} ctx={ctx} />
          ))}
        </div>
      )}
    </section>
  );
}

/** What a card in Now says at its foot. */
function nowStatus(card: Card, me: string) {
  if (alive(card)) return card.chat?.waiting ? "Waiting for a yes or no in its chat" : card.progress || "Working in its chat";
  if (card.progress) return card.progress;
  if (card.worker === "assistant" && !card.chat) return "Its chat was deleted";
  // The assistant's chat is between replies: where the steps stand.
  if (card.worker === "assistant") return stepsLine(card.steps) || "Between replies in its chat";
  const who = workerName(card, me);
  return who && card.startedAt ? `${who} started this ${ago(card.startedAt)}` : stepsLine(card.steps);
}

function NowCard({ card, ctx }: { card: Card; ctx: BoardContext }) {
  const user = useUser();
  const live = alive(card);
  const who = workerName(card, user.id);
  return (
    <article
      aria-label={card.title}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData(DRAG_TYPE, card.id);
        e.dataTransfer.effectAllowed = "move";
        ctx.setDragging(card);
      }}
      onDragEnd={() => ctx.setDragging(null)}
      className={cn("flex min-w-0 flex-col gap-3 rounded-xl border bg-card p-4", live && "border-signal bg-signal/5")}
      data-alive={live || undefined}
    >
      <div className="flex items-center gap-2 text-2xs font-medium tracking-wider text-muted-foreground uppercase">
        <span className="truncate">{card.kind || "Card"}</span>
        <span className="ml-auto" />
        {card.worker && <WhoMark assistant={card.worker === "assistant"} name={card.workerUser?.name || ""} label={`${who} ${who === "You" ? "are" : "is"} on this`} />}
        <CardMenu card={card} actions={ctx.actions} onOpen={() => ctx.open(card.id)} onDelete={() => ctx.askDelete(card)} />
      </div>
      <h3 className="text-base leading-snug font-semibold text-pretty">{card.title}</h3>
      <StepList steps={card.steps.slice(0, 6)} live={live} />
      {card.steps.length > 6 && <p className="text-xs text-muted-foreground">and {count(card.steps.length - 6, "more step")}</p>}
      <div className="mt-auto flex items-center gap-2 text-sm">
        <span className={cn("min-w-0 grow truncate", live ? "text-signal" : "text-muted-foreground")}>{nowStatus(card, user.id)}</span>
        <Button variant="outline" size="sm" onClick={() => ctx.open(card.id)}>
          Open
        </Button>
      </div>
    </article>
  );
}

function Column({ ctx, status, name, hint, cards, numbered = false, children }: { ctx: BoardContext; status: CardStatus; name: string; hint: string; cards: Card[]; numbered?: boolean; children?: React.ReactNode }) {
  const [over, setOver] = useState(false);
  const target = dropTarget(ctx, () => {
    setOver(false);
    drop(ctx, status, cards);
  });
  return (
    <section
      aria-label={name}
      className={cn("flex min-w-0 flex-col gap-2.5 rounded-lg p-1", over && "bg-raised")}
      {...target}
      onDragEnter={() => ctx.dragging && setOver(true)}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
      }}
    >
      <ColumnHead name={name} count={cards.length} hint={hint} />
      {children}
      {cards.map((card, i) => (
        <BoardCard
          key={card.id}
          card={card}
          ctx={ctx}
          number={numbered ? i + 1 : undefined}
          onDropBefore={() => {
            setOver(false);
            drop(ctx, status, cards, card);
          }}
          up={numbered && i > 0 ? () => ctx.actions.move(card, status, i - 1) : undefined}
          down={numbered && i < cards.length - 1 ? () => ctx.actions.move(card, status, i + 1) : undefined}
        />
      ))}
    </section>
  );
}

function ColumnHead({ name, count: n, hint }: { name: string; count: number; hint: string }) {
  return (
    <div className="grid gap-0.5 px-0.5 pb-0.5">
      <h2 className="flex items-baseline gap-2 text-md font-semibold">
        {name}
        <span className="font-mono text-xs font-normal text-muted-foreground">{n}</span>
      </h2>
      <span className="text-xs text-muted-foreground">{hint}</span>
    </div>
  );
}

function BoardCard({ card, ctx, number, onDropBefore, up, down }: { card: Card; ctx: BoardContext; number?: number; onDropBefore: () => void; up?: () => void; down?: () => void }) {
  const user = useUser();
  const suggested = card.status === "suggested";
  const done = card.status === "done";
  const maker = card.madeBy === "assistant" ? "the assistant" : card.maker?.id === user.id ? "you" : card.maker?.name || "someone";
  return (
    <article
      aria-label={card.title}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData(DRAG_TYPE, card.id);
        e.dataTransfer.effectAllowed = "move";
        ctx.setDragging(card);
      }}
      onDragEnd={() => ctx.setDragging(null)}
      {...dropTarget(ctx, onDropBefore)}
      className={cn("flex min-w-0 flex-col gap-2 rounded-lg border bg-card p-3", suggested && "border-dashed border-line-strong bg-transparent", ctx.dragging?.id === card.id && "opacity-50")}
    >
      <div className="flex items-center gap-2 text-2xs font-medium tracking-wider text-muted-foreground uppercase">
        {number !== undefined && <span className="font-mono text-text-2">{number}</span>}
        <span className="truncate">{suggested ? "Suggested by the assistant" : card.kind}</span>
        <span className="ml-auto" />
        {!suggested && <WhoMark assistant={card.madeBy === "assistant"} name={card.maker?.name || ""} label={`Made by ${maker}`} />}
        <CardMenu card={card} actions={ctx.actions} onOpen={() => ctx.open(card.id)} onDelete={() => ctx.askDelete(card)} up={up} down={down} />
      </div>
      <button type="button" onClick={() => ctx.open(card.id)} className={cn("text-left text-md leading-snug font-medium text-pretty hover:underline", done && "text-muted-foreground")}>
        {card.title}
      </button>
      {card.notes && !done && <p className="line-clamp-2 text-sm text-muted-foreground">{card.notes}</p>}
      {card.steps.length > 0 && !done && <p className="text-xs text-muted-foreground">{stepsLine(card.steps)}</p>}
      {card.status === "waiting" && <Answer card={card} actions={ctx.actions} />}
      {suggested && (
        <div className="flex flex-wrap gap-1.5">
          <Button size="sm" onClick={() => ctx.actions.change(card, { status: "idea" }, "Kept in Ideas")}>
            Keep
          </Button>
          <Button size="sm" variant="outline" onClick={() => ctx.actions.change(card, { status: "dismissed" }, "Suggestion dismissed", { status: "suggested" })}>
            Dismiss
          </Button>
        </div>
      )}
    </article>
  );
}

function DoneColumn({ ctx, cards, total }: { ctx: BoardContext; cards: Card[]; total: number }) {
  const [all, setAll] = useState(false);
  const [limit, setLimit] = useState(100);
  const more = useDoneCards(ctx.projectId, limit, all);
  const shown = all && more.data ? more.data.items : cards.slice(0, DONE_SHOWN);
  const target = dropTarget(ctx, () => drop(ctx, "done", cards));
  return (
    <section aria-label="Done" className="flex min-w-0 flex-col gap-2.5 rounded-lg p-1" {...target}>
      <ColumnHead name="Done" count={total} hint="Each keeps its chat and history." />
      {shown.map((card) => (
        <BoardCard key={card.id} card={card} ctx={ctx} onDropBefore={() => drop(ctx, "done", cards)} />
      ))}
      {!all && total > DONE_SHOWN && (
        <Button variant="ghost" size="sm" className="self-start" onClick={() => setAll(true)}>
          {total - DONE_SHOWN} more
        </Button>
      )}
      {all && more.isPending && <Loading compact />}
      {all && more.data?.more && (
        <Button variant="ghost" size="sm" className="self-start" onClick={() => setLimit((n) => n + 100)}>
          Show more
        </Button>
      )}
      {all && (
        <Button variant="ghost" size="sm" className="self-start" onClick={() => setAll(false)}>
          Show fewer
        </Button>
      )}
    </section>
  );
}

/** The input on top of Ideas: Enter adds the idea. */
function AddIdea({ projectId }: { projectId: string }) {
  const client = useQueryClient();
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <form
      onSubmit={async (e) => {
        e.preventDefault();
        if (!title.trim() || busy) return;
        setBusy(true);
        try {
          await createCard(projectId, { title: title.trim(), status: "idea" });
          setTitle("");
          refreshBoard(client, projectId);
        } catch (err) {
          toast(messageOf(err));
        } finally {
          setBusy(false);
        }
      }}
    >
      <Input aria-label="Add an idea" placeholder="Add an idea…" value={title} maxLength={LIMITS.title} readOnly={busy} onChange={(e) => setTitle(e.target.value)} />
    </form>
  );
}

function NewCardDialog({ projectId, open, onOpenChange }: { projectId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const client = useQueryClient();
  const { busy, error, onSubmit } = useSubmit(async (data) => {
    await createCard(projectId, {
      title: String(data.get("card-title") || "").trim(),
      kind: String(data.get("card-kind") || "").trim(),
      notes: String(data.get("card-notes") || "").trim(),
      status: data.get("card-column") === "next" ? "next" : "idea",
    });
    refreshBoard(client, projectId);
    onOpenChange(false);
    toast("Card added");
  });
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="New card" description="Something this project needs doing. The assistant sees it in this project’s chats.">
      <form className="grid gap-4" onSubmit={onSubmit}>
        <TextField id="card-title" label="Title" required maxLength={LIMITS.title} placeholder="Add a limitations paragraph" autoFocus />
        <div className="grid gap-4 sm:grid-cols-2">
          <TextField id="card-kind" label="Kind" maxLength={LIMITS.kind} placeholder="Writing" help="A word or two, in your project’s terms." />
          <SelectField
            id="card-column"
            label="Column"
            defaultValue="idea"
            options={[
              ["idea", "Ideas"],
              ["next", "Up next"],
            ]}
          />
        </div>
        <TextAreaField id="card-notes" label="Notes" rows={4} maxLength={LIMITS.notes} placeholder="What it is for, and what done looks like." />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Add card
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}
