"use client";

// The message box: files picked, pasted or dropped; what the model can use; the
// model and its effort; send and stop, with how much of the model's window the chat
// uses; and the line under it saying what is happening.

import { ArrowUp, BookOpen, Clock, Paperclip, Plus, Square, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { BrowserToggle } from "@/components/browser/browser-toggle";
import { CodeChip } from "@/components/code/code-chip";
import { KnowledgePicker, KnowledgePill } from "@/components/knowledge/chat-knowledge";
import { ProjectComposerChip } from "@/components/projects/chat-project";
import { useChatDraft } from "@/lib/chat/chat-draft";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { WorkspaceToggle } from "@/components/workspace/workspace-toggle";
import { describe } from "@/lib/chat/attachments";
import { pastesAsFile, PROMPT_LIMIT, promptCount } from "@/lib/chat/composer";
import { matchItems, menuTrigger, replaceTrigger } from "@/lib/chat/composer-menu";
import { contextUse } from "@/lib/chat/context";
import { temporaryNoteText } from "@/lib/chat/temporary";
import { cn } from "@/lib/utils";
import { useChat } from "./chat-provider";
import { ComposerMenu, type MenuEntry, useCommands, useMentions } from "./composer-menu";
import { NamedIcon } from "./icon";
import { ModelPicker } from "./model-picker";
import { ToolsButton } from "@/components/connections/chat-tools";
import { compact } from "@/lib/format";

const composerFiles = (list?: FileList | null) => [...(list || [])].filter((file) => file.size > 0);

function AttachmentChips() {
  const store = useChat();
  const { selected, uploading } = useSyncExternalStore(store.attachments.subscribe, store.attachments.snapshot, store.attachments.snapshot);
  if (!selected.length && !uploading) return null;
  return (
    <div className="flex flex-wrap gap-1.5">
      {selected.map((a) => {
        const d = describe(a),
          text = store.attachments.pastedText(a.id);
        return (
          <div key={a.id} className="flex max-w-full items-center gap-2 rounded-md border bg-background px-2 py-1.5 sm:max-w-72">
            <NamedIcon name={d.glyph} className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1">
              {text !== undefined ? (
                <button type="button" className="block max-w-full truncate text-left text-sm font-medium hover:underline" title="Show the pasted text" onClick={() => store.openDialog({ kind: "pasted", id: a.id, name: a.name })}>
                  {a.name}
                </button>
              ) : (
                <strong className="block truncate text-sm font-medium">{a.name}</strong>
              )}
              <small className="block truncate text-xs text-muted-foreground">
                {d.detail}
                {text !== undefined ? ` · ${text.split("\n").length.toLocaleString()} lines` : ""}
              </small>
              {d.note && <small className="block text-xs text-muted-foreground">{d.note}</small>}
            </span>
            <Button type="button" variant="ghost" size="icon-xs" aria-label={`Remove ${a.name}`} disabled={store.streaming} onClick={() => store.attachments.remove(a.id)}>
              <X />
            </Button>
          </div>
        );
      })}
      {uploading && (
        <span className="self-center text-sm text-muted-foreground" role="status">
          Uploading files…
        </span>
      )}
    </div>
  );
}

function SendButton() {
  const store = useChat();
  const use = contextUse(store.active, { windowSize: Number(store.policyModel()?.metadata?.contextWindow) || 0, systemPrompt: store.systemPrompt, open: !!store.route.id });
  const op = store.currentOperation();
  const round = 2 * Math.PI * 17,
    // The ring takes the accent while a reply is being written; near the end of the window it warns instead.
    level = use.share > 0.9 ? "text-destructive-foreground" : use.share > 0.75 ? "text-amber" : store.streaming ? "text-signal" : "text-foreground";
  const amount = use.unknown ? "—" : use.windowSize ? `${compact(use.used)} / ${compact(use.windowSize)}` : use.used ? compact(use.used) : "—";
  const stopping = store.streaming;
  const disabled = stopping ? !!op?.stopRequested : store.switchingVersion || !store.available || store.attachments.busy();
  return (
    <HoverCard openDelay={300} closeDelay={100}>
      <HoverCardTrigger asChild>
        <span className="relative inline-flex size-10 shrink-0 items-center justify-center">
          <svg className={cn("pointer-events-none absolute inset-0 size-10 -rotate-90", level)} viewBox="0 0 40 40" aria-hidden="true">
            <circle cx="20" cy="20" r="17" fill="none" stroke="var(--line-strong)" strokeWidth="2" />
            {use.share > 0 && <circle cx="20" cy="20" r="17" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeDasharray={`${Math.max(1.5, use.share * round).toFixed(1)} ${round.toFixed(1)}`} />}
          </svg>
          <Button
            type={stopping ? "button" : "submit"}
            size="icon-sm"
            className="rounded-full"
            disabled={disabled}
            aria-label={stopping ? "Stop response" : "Send message"}
            aria-describedby="context-card"
            onClick={stopping ? () => store.run(() => store.stopCurrent()) : undefined}
          >
            {stopping ? <Square className="fill-current" /> : <ArrowUp />}
          </Button>
        </span>
      </HoverCardTrigger>
      <HoverCardContent id="context-card" role="tooltip" align="end" className="w-64 text-sm">
        <div className="flex justify-between">
          <strong>Context</strong>
          <span>{amount}</span>
        </div>
        {use.windowSize > 0 && !use.unknown && (
          <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-secondary" role="meter" aria-label="Context used" aria-valuemin={0} aria-valuemax={use.windowSize} aria-valuenow={Math.min(use.used, use.windowSize)}>
            <div className="h-full bg-foreground" style={{ width: `${use.share * 100}%` }} />
          </div>
        )}
        {use.used || use.unknown ? (
          <div className="mt-2 grid gap-1 text-muted-foreground">
            <div className="flex justify-between">
              <span>Messages</span>
              <span>about {compact(use.messages)}</span>
            </div>
            {!use.unknown && (
              <div className="flex justify-between">
                <span>Tools, instructions and pages</span>
                <span>about {compact(use.rest)}</span>
              </div>
            )}
            <div className="flex justify-between">
              <span>Last reply</span>
              <span>
                {compact(use.output)} out{use.steps > 1 ? ` · ${use.steps} steps` : ""}
              </span>
            </div>
            {use.unknown ? <p>The size shows after the next reply.</p> : !use.windowSize && <p>This model does not report its window.</p>}
          </div>
        ) : (
          <p className="mt-1 text-muted-foreground">Shows after the first reply.</p>
        )}
        {store.route.id && (
          <button type="button" className="mt-2 text-sm underline-offset-2 hover:underline" onClick={() => store.toggleDetails()}>
            Chat details →
          </button>
        )}
      </HoverCardContent>
    </HoverCard>
  );
}

export function composerHint(store: ReturnType<typeof useChat>) {
  const op = store.currentOperation();
  return store.switchingVersion
    ? "Switching response version…"
    : store.streaming
      ? op?.stopRequested
        ? "Stopping the response…"
        : op?.phase === "recovering"
          ? "Reconnecting to the saved response…"
          : op?.phase === "sending"
            ? "Sending your message…"
            : "Generating a response…"
    : "AI can make mistakes. Check important information.";
}

const IDLE_PLACEHOLDER = "Ask a question, write some code, follow an idea…";

export function Composer({ home = false, placeholder = IDLE_PLACEHOLDER }: { home?: boolean; placeholder?: string }) {
  const store = useChat();
  const draft = useChatDraft();
  const box = useRef<HTMLTextAreaElement>(null);
  const files = useRef<HTMLInputElement>(null);
  const [text, setText] = useState(store.draft);
  const [dropping, setDropping] = useState(false);
  const [picker, setPicker] = useState(false);
  // Where the caret is, which row of the open list is marked, and the text the list
  // was closed on (it stays closed until the text changes).
  const [caret, setCaret] = useState(store.draft.length);
  const [marked, setMarked] = useState(0);
  const [closedOn, setClosedOn] = useState<string | null>(null);
  const [focused, setFocused] = useState(false);
  const revision = store.draftRevision;
  // A draft set elsewhere (a starter, a quote, a failed send) shows in the box.
  useEffect(() => {
    setText(store.draft);
    setCaret(store.draft.length);
    const el = box.current;
    if (el && document.activeElement !== el && store.draft) {
      el.focus();
      el.setSelectionRange(store.draft.length, store.draft.length);
    }
  }, [revision, store]);
  // The box grows with what is in it, up to a point.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  }, [text]);
  const available = store.available;
  const conversationId = store.route.id || null;
  const knowledgeOn = store.feature("knowledge");
  const submit = () => {
    const value = box.current?.value ?? text;
    store.run(() => store.send(value));
  };
  const temporaryShown = home && !draft.project;
  const attachDisabled = store.streaming || store.attachments.full();

  const trigger = focused && available && closedOn !== text ? menuTrigger(text, caret) : null;
  const commands = useCommands({ home, attach: () => files.current?.click(), knowledge: knowledgeOn ? () => setPicker(true) : null });
  const mentions = useMentions(conversationId, trigger?.kind === "mention");
  const entries = trigger ? matchItems(trigger.kind === "command" ? commands : mentions, trigger.query) : [];
  const menu = trigger && entries.length ? trigger : null;
  const active = Math.min(marked, entries.length - 1);
  const write = (value: string, at = value.length) => {
    setText(value);
    store.setDraft(value);
    setCaret(at);
    requestAnimationFrame(() => {
      box.current?.focus();
      box.current?.setSelectionRange(at, at);
    });
  };
  const pick = (item: MenuEntry) => {
    if (!menu) return;
    // A command takes its own text out of the box; a mention leaves what it points at.
    const said = item.run();
    const next = replaceTrigger(text, menu, typeof said === "string" ? said : "");
    setMarked(0);
    write(next.text, next.caret);
  };
  return (
    // min-w-0: in Home's grid the box may be narrower than its toolbar's full width, which then shrinks the model button.
    <div className="relative min-w-0">
      {menu && (
        <ComposerMenu id="composer-menu" label={menu.kind === "command" ? "Actions" : "Point at something"} items={entries} active={active} query={menu.query} below={home} onActive={setMarked} onPick={pick} />
      )}
      {/* 12px corners, a strong line and a soft shadow. */}
      <form
        id="chat-form"
        className={cn(
          "rounded-xl border border-line-strong bg-card shadow-[0_8px_32px_var(--shadow-soft)] transition-[border-color,box-shadow] focus-within:border-focus-border focus-within:shadow-[0_0_0_1px_var(--focus-border),0_8px_32px_var(--shadow-soft)]",
          dropping && "border-dashed border-blue bg-blue/5",
        )}
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
        onDragOver={(e) => {
          if (!e.dataTransfer?.types?.includes("Files")) return;
          e.preventDefault();
          setDropping(true);
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropping(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          setDropping(false);
          // A folder can't be uploaded as it is; a zip of it can, and keeps its layout.
          if ([...(e.dataTransfer?.items || [])].some((item) => item.webkitGetAsEntry?.()?.isDirectory)) {
            store.run(() => {
              throw new Error("To attach a folder, zip it first and attach the zip.");
            });
            return;
          }
          const list = composerFiles(e.dataTransfer?.files);
          if (list.length) store.attachments.add(list);
        }}
      >
        <div className="flex flex-wrap gap-1.5 px-3.5 pt-3 empty:hidden">
          <ProjectComposerChip onClear={() => store.code.resetDraft()} />
          <CodeChip />
          {home && knowledgeOn && <KnowledgePill conversationId={null} addable={false} disabled={store.streaming} />}
          <AttachmentChips />
        </div>
        <textarea
          id="prompt"
          ref={box}
          aria-label="Message"
          rows={home ? 3 : 2}
          placeholder={available ? placeholder : "Connect your gateway to start a chat…"}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={!!menu}
          aria-controls={menu ? "composer-menu" : undefined}
          aria-activedescendant={menu ? `composer-menu-${active}` : undefined}
          maxLength={PROMPT_LIMIT}
          disabled={!available}
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            store.setDraft(e.target.value);
            setCaret(e.target.selectionStart ?? e.target.value.length);
            setMarked(0);
          }}
          onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          onKeyDown={(e) => {
            if (menu && !e.nativeEvent.isComposing) {
              const step = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
              if (step) {
                e.preventDefault();
                setMarked((active + step + entries.length) % entries.length);
                return;
              }
              if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") {
                e.preventDefault();
                pick(entries[active]!);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                setClosedOn(text);
                return;
              }
            }
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit();
            }
          }}
          onPaste={(e) => {
            // Text copied from documents can carry a picture of itself; keep it as text.
            const pasted = e.clipboardData?.getData("text/plain") || "";
            if (pasted) {
              // Long text is attached as a file, so the box never cuts it off and the
              // question stays in view.
              if (pastesAsFile(pasted, e.currentTarget)) {
                e.preventDefault();
                store.attachments.addText(pasted);
              }
              return;
            }
            const list = composerFiles(e.clipboardData?.files);
            if (!list.length) return;
            e.preventDefault();
            store.attachments.add(list);
          }}
          className={cn(
            "max-h-[220px] w-full resize-none bg-transparent text-base leading-[1.65] outline-none placeholder:text-muted-foreground disabled:opacity-60",
            home ? "min-h-[104px] px-[18px] pt-[18px] pb-2" : "min-h-[68px] px-4 pt-3.5 pb-1",
          )}
        />
        <div className="flex items-center gap-1.5 pt-1.5 pr-2.5 pb-2.5 pl-3">
          <input
            ref={files}
            id="message-files"
            className="sr-only"
            type="file"
            multiple
            tabIndex={-1}
            onChange={(e) => {
              const list = [...(e.target.files || [])];
              e.target.value = "";
              store.attachments.add(list);
            }}
          />
          {knowledgeOn ? (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button type="button" variant="ghost" size="icon-sm" className="rounded-full" disabled={store.streaming} title="Attach files or add knowledge" aria-label="Attach or add">
                  <Plus />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" aria-label="Add to this message">
                <DropdownMenuItem disabled={store.attachments.full()} onSelect={() => files.current?.click()}>
                  <Paperclip /> Attach files
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={() => setPicker(true)}>
                  <BookOpen /> Knowledge collections
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : (
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="rounded-full"
              disabled={attachDisabled}
              title="Attach files of any kind (you can also paste or drop them)"
              aria-label="Attach files"
              onClick={() => files.current?.click()}
            >
              <Paperclip />
            </Button>
          )}
          {/* Never wraps: on a phone the model button shrinks instead (model-picker). */}
          <div className="flex shrink-0 flex-nowrap items-center gap-0.5" role="group" aria-label="What the model can use">
            <WorkspaceToggle />
            <BrowserToggle />
            <ToolsButton chatId={conversationId} disabled={store.streaming} />
          </div>
          <span className="ml-auto" />
          <span className="text-xs text-muted-foreground" title="Longer text is attached as a file when you paste it">
            {promptCount(text.length)}
          </span>
          {temporaryShown && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              aria-pressed={store.temporaryDraft}
              className={cn("h-8 gap-1.5 rounded-md border px-2.5 text-sm font-normal text-muted-foreground hover:border-line-strong hover:text-foreground [&_svg]:size-3.5", store.temporaryDraft && "border-line-strong bg-secondary text-foreground")}
              title={store.temporaryDraft ? "Turn off to keep this chat" : "A chat that isn’t kept"}
              onClick={() => {
                store.toggleTemporary();
                requestAnimationFrame(() => box.current?.focus());
              }}
            >
              <Clock />
              <span className="hidden sm:inline">Temporary chat</span>
            </Button>
          )}
          <ModelPicker />
          <SendButton />
        </div>
      </form>
      <p className="mt-2 text-center text-xs leading-normal text-faint">
        {composerHint(store)}
        {available && !store.streaming && !store.switchingVersion && (
          <span className="max-[760px]:hidden">
            {" "}
            · <kbd className="font-mono">/</kbd> for actions · <kbd className="font-mono">@</kbd> to point at a file, a job or a tool
          </span>
        )}
      </p>
      {knowledgeOn && <KnowledgePicker open={picker} onOpenChange={setPicker} conversationId={conversationId} />}
    </div>
  );
}

/** The marker shown while a chat is, or is about to be, temporary. */
export function TemporaryNote({ started = true }: { started?: boolean }) {
  return (
    <p role="note" className="flex items-start gap-2 rounded-lg border border-dashed border-line-strong px-3 py-2 text-sm text-text-2">
      <Clock className="mt-px size-[15px] shrink-0 text-muted-foreground" />
      <span>
        <strong className="font-medium text-foreground">Temporary chat.</strong>{" "}
        {temporaryNoteText(started)}
      </span>
    </p>
  );
}
