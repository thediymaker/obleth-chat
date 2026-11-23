"use client";

// Pieces the board and an open card share: who is on a card, its steps, its menu,
// and the actions a card offers.

import { useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Check, Ellipsis, ExternalLink, Hand, MessageSquare, Trash2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { toast } from "sonner";
import { BrandLogo } from "@/components/brand";
import { useChatStore } from "@/components/chat/chat-provider";
import { NamedIcon } from "@/components/chat/icon";
import { messageOf, PersonAvatar } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useBranding } from "@/lib/branding-context";
import { chatDraft } from "@/lib/chat/chat-draft";
import { cn } from "@/lib/utils";
import { type Card, type CardChange, type CardStatus, type Step, COLUMN_NAMES, changeCard, deleteCard, refreshBoard } from "./api";

/** The assistant's mark: the site's logo, or its assistant icon, in a ring. */
export function AssistantMark({ label, className }: { label: string; className?: string }) {
  const branding = useBranding();
  return (
    <span role="img" aria-label={label} title={label} className={cn("grid size-5 shrink-0 place-items-center rounded-full border", className)}>
      {branding.logo.light || branding.logo.dark ? <BrandLogo className="size-3" /> : <NamedIcon name={branding.icons.assistant || "chat"} className="size-3 text-text-2" />}
    </span>
  );
}

/** Who made a card, or who is on it: the assistant's mark or the person's initials. */
export function WhoMark({ assistant, name, label }: { assistant: boolean; name: string; label: string }) {
  if (assistant) return <AssistantMark label={label} />;
  return (
    <span role="img" aria-label={label} title={label} className="shrink-0">
      <PersonAvatar name={name} size="sm" className="size-5" />
    </span>
  );
}

/** A card's steps, each with a mark for done, now or still to do. `live` draws the step being worked in the accent. */
export function StepList({ steps, live = false, className }: { steps: Step[]; live?: boolean; className?: string }) {
  if (!steps.length) return null;
  return (
    <ul className={cn("grid gap-1.5 text-md", className)}>
      {steps.map((s, i) => (
        <li key={i} className={cn("flex items-start gap-2", s.state === "todo" ? "text-muted-foreground" : s.state === "now" ? "text-foreground" : "text-text-2")}>
          <StepMark state={s.state} live={live} />
          <span className="min-w-0">
            <span className="sr-only">{s.state === "done" ? "Done: " : s.state === "now" ? "Now: " : "To do: "}</span>
            {s.text}
          </span>
        </li>
      ))}
    </ul>
  );
}

export function StepMark({ state, live = false }: { state: Step["state"]; live?: boolean }) {
  return (
    <span aria-hidden className={cn("mt-0.5 grid size-3.5 shrink-0 place-items-center rounded-full border", state === "now" ? (live ? "border-signal" : "border-foreground") : "border-line-strong")}>
      {state === "done" && <Check className="size-2.5 text-text-2" strokeWidth={3.5} />}
      {state === "now" && <span className={cn("size-1.5 rounded-full", live ? "signal-breathe bg-signal" : "bg-foreground")} />}
    </span>
  );
}

/** Changing a card from the board or an open card: a failure is toasted, a success refreshes the board. */
export function useCardActions(projectId: string) {
  const client = useQueryClient();
  const store = useChatStore();
  const router = useRouter();
  async function change(card: Card, body: CardChange, done?: string, undo?: CardChange) {
    try {
      await changeCard(projectId, card.id, body);
      refreshBoard(client, projectId);
      if (done) toast(done, undo ? { action: { label: "Undo", onClick: () => void change(card, undo) } } : undefined);
      return true;
    } catch (e) {
      toast(messageOf(e));
      return false;
    }
  }
  return {
    change,
    /** Moves a card to a column, at `position` there when given. */
    move: (card: Card, status: CardStatus, position?: number) => change(card, position === undefined ? { status } : { status, position }, status === card.status ? undefined : `Moved to ${COLUMN_NAMES[status]}`),
    /** Puts the person on the card. */
    take: (card: Card) => change(card, { worker: "me" }, "You are on it"),
    /** A new chat in the project for this card; sending its first message links them and puts the assistant on it. */
    start(card: Card) {
      store.newChat();
      chatDraft.setProject(projectId);
      chatDraft.setCard({ id: card.id, title: card.title });
      store.starter(`Let’s work on the card “${card.title}”.`, false);
      router.push("/");
    },
    openChat: (card: Card) => card.chat?.mine && router.push(`/chat/${encodeURIComponent(card.chat.id)}`),
    async remove(card: Card) {
      await deleteCard(projectId, card.id);
      refreshBoard(client, projectId);
      toast("Card deleted");
    },
  };
}
export type CardActions = ReturnType<typeof useCardActions>;

/** Where a card can be moved from its menu. */
const TARGETS: CardStatus[] = ["idea", "next", "waiting", "done"];

/**
 * A card's menu, reachable by keyboard: open it, work it, move it up or down in its
 * column or to another column, or delete it (which asks first, through `onDelete`).
 */
export function CardMenu({
  card,
  actions,
  onOpen,
  onDelete,
  up,
  down,
}: {
  card: Card;
  actions: CardActions;
  onOpen: () => void;
  onDelete: () => void;
  up?: () => void;
  down?: () => void;
}) {
  const column = card.status === "suggested" ? "idea" : card.status;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-xs" aria-label={`Actions for ${card.title}`} title="Actions" className="text-muted-foreground">
          <Ellipsis />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuItem onSelect={onOpen}>
          <ExternalLink /> Open
        </DropdownMenuItem>
        {card.chat?.mine && (
          <DropdownMenuItem onSelect={() => actions.openChat(card)}>
            <MessageSquare /> Open its chat
          </DropdownMenuItem>
        )}
        {card.status !== "done" && (
          <>
            <DropdownMenuItem onSelect={() => actions.start(card)}>
              <AssistantGlyph /> Start with the assistant
            </DropdownMenuItem>
            {!(card.status === "doing" && card.worker === "person") && (
              <DropdownMenuItem onSelect={() => actions.take(card)}>
                <Hand /> I’m on it
              </DropdownMenuItem>
            )}
          </>
        )}
        {(up || down) && <DropdownMenuSeparator />}
        {up && (
          <DropdownMenuItem onSelect={up}>
            <ArrowUp /> Move up
          </DropdownMenuItem>
        )}
        {down && (
          <DropdownMenuItem onSelect={down}>
            <ArrowDown /> Move down
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">Move to</DropdownMenuLabel>
        {TARGETS.filter((t) => t !== column).map((t) => (
          <DropdownMenuItem key={t} onSelect={() => actions.move(card, t)}>
            {COLUMN_NAMES[t]}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive" onSelect={onDelete}>
          <Trash2 /> Delete…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function AssistantGlyph() {
  const branding = useBranding();
  return <NamedIcon name={branding.icons.assistant || "chat"} />;
}

/** A Needs you card's question, answered with one of its options or in words. */
export function Answer({ card, actions }: { card: Card; actions: CardActions }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  async function answer(value: string) {
    if (!value.trim()) return;
    setBusy(true);
    const ok = await actions.change(card, { answer: value.trim() }, card.chat ? "Answered; the card is back in Now" : "Answered; the card is back in Up next");
    setBusy(false);
    if (ok) setText("");
  }
  if (!card.question) return null;
  return (
    <div className="grid gap-2">
      <p className="text-sm text-foreground">{card.question}</p>
      {card.options.length > 0 ? (
        <div className="flex flex-wrap gap-1.5" role="group" aria-label={`Answers to: ${card.question}`}>
          {card.options.map((o, i) => (
            <Button key={o} size="sm" variant={i === 0 ? "default" : "outline"} disabled={busy} onClick={() => answer(o)}>
              {o}
            </Button>
          ))}
        </div>
      ) : (
        <form
          className="flex gap-1.5"
          onSubmit={(e) => {
            e.preventDefault();
            void answer(text);
          }}
        >
          <Input aria-label={`Your answer to: ${card.question}`} placeholder="Your answer" value={text} maxLength={500} onChange={(e) => setText(e.target.value)} className="h-8" />
          <Button size="sm" type="submit" disabled={busy || !text.trim()}>
            Answer
          </Button>
        </form>
      )}
    </div>
  );
}
