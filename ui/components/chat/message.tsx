"use client";

// One message of a conversation, with its attachments.

import { Bookmark, ChevronDown, ChevronLeft, ChevronRight, Clock, Copy, ExternalLink, GitBranch, Info, Orbit, Pencil, Plus, RefreshCw, ThumbsDown, ThumbsUp } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import { BrandLogo } from "@/components/brand";
import { BrowserCard } from "@/components/browser/browser-card";
import { MessageActivity } from "@/components/workspace/message-activity";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useBranding } from "@/lib/branding-context";
import { attachmentUrl, describe } from "@/lib/chat/attachments";
import { clock, effortLabel, NUDGE } from "@/lib/chat/composer";
import { isPendingMessage, responseVersion } from "@/lib/chat/conversation-state";
import { ratable } from "@/lib/chat/feedback";
import type { ChatStore } from "@/lib/chat/store";
import { appliesWork, duration, gist, isLive, latest } from "@/lib/chat/turn";
import type { Message as ChatMessage } from "@/lib/chat/types";
import { initials } from "@/lib/format";
import { useSession } from "@/lib/session";
import { cn } from "@/lib/utils";
import { useChatStore as useChat } from "./chat-provider";
import { NamedIcon } from "./icon";
import { citeHtml, KnowledgeSources } from "@/components/knowledge/chat-knowledge";
import { SavedNotes } from "@/components/library/saved-notes";
import { useChatProjectId } from "@/components/projects/chat-project";
import { codeTitle, Markdown } from "./markdown-view";
import { AppOffer, QuestionCard, ResearchOffer, WorkspaceOffer } from "./question-cards";
import { SteerForm, TurnAfter, TurnBefore } from "./turn-view";

function IconButton({ label, onClick, children, pressed, disabled, ...rest }: { label: string; onClick: () => void; children: React.ReactNode; pressed?: boolean; disabled?: boolean; [key: string]: unknown }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button type="button" variant="ghost" size="icon-xs" aria-label={label} aria-pressed={pressed} disabled={disabled} onClick={onClick} className={cn("size-[30px] text-muted-foreground [&_svg]:size-[15px]", pressed && "bg-secondary text-foreground")} {...rest}>
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** The thinking row, closed: while it thinks, its latest sentence; after, how long it took and where it ended up. */
export function thinkingSummary(m: ChatMessage) {
  const text = m.reasoningContent || "",
    now = m.status === "streaming" && !m.content;
  const ms = (m.activity || []).filter((e) => e.kind === "thought").reduce((t, e) => t + (e.durationMs || 0), 0);
  const last = text.trim().split(/\n\s*\n/).at(-1) || text;
  return { now, label: now ? "Thinking" : ms ? `Thought for ${duration(ms)}` : "Thought", line: now ? latest(text, 140) : gist(last, 140) };
}

function ThinkingPanel({ m, index }: { m: ChatMessage; index: number }) {
  const store = useChat();
  const content = useRef<HTMLDivElement>(null);
  const key = m.id || `pending-${index}`;
  const [, redraw] = useState(0);
  const [now, setNow] = useState(() => Date.now());
  const t = thinkingSummary(m);
  useEffect(() => {
    if (!t.now) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [t.now]);
  // Stays at the end of the thinking while it grows, unless the person scrolled up in it.
  const atEnd = useRef(true);
  useEffect(() => {
    const el = content.current;
    if (el && atEnd.current) el.scrollTop = el.scrollHeight;
  }, [m.reasoningContent]);
  if (!m.reasoningContent) return null;
  const since = Date.parse(m._began || m.createdAt || "");
  return (
    <details
      className="group/thinking mb-3 rounded-lg border"
      open={store.thinkingOpen.get(key) === true}
      onToggle={(e) => {
        store.thinkingOpen.set(key, (e.target as HTMLDetailsElement).open);
        redraw((n) => n + 1);
      }}
    >
      <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-sm [&::-webkit-details-marker]:hidden">
        <Orbit className={cn("size-4 shrink-0 text-muted-foreground", t.now && "animate-spin [animation-duration:3s]")} />
        <span className="shrink-0 font-medium">{t.label}</span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">{t.line}</span>
        {t.now && Number.isFinite(since) && <small className="shrink-0 font-mono text-xs text-faint">{clock(now - since)}</small>}
        <ChevronDown className="size-4 shrink-0 text-muted-foreground transition-transform group-open/thinking:rotate-180" />
      </summary>
      <div
        ref={content}
        onScroll={(e) => {
          const el = e.currentTarget;
          atEnd.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 40;
        }}
        className="max-h-80 overflow-y-auto overscroll-contain border-t px-3 py-2 text-text-2"
      >
        <Markdown text={m.reasoningContent} />
      </div>
    </details>
  );
}

function responseStatus(store: ChatStore, m: ChatMessage) {
  if (isPendingMessage(m))
    return store.currentOperation()?.stopRequested
      ? "Stopping…"
      : (m.activity || []).some((s) => s.status === "awaiting")
        ? "Waiting for your answer…"
        : m.reasoningContent && !m.content
          ? "Thinking…"
          : m.content
            ? "Responding…"
            : "Waiting for the model…";
  const status = m.status || "completed",
    model = m.settings?.resolvedModel || m.settings?.model;
  const usage = m.usage?.total_tokens ? `${Number(m.usage.total_tokens).toLocaleString()} tokens` : "";
  const label = ({ completed: "", interrupted: "Stopped", cancelled: "Stopped", error: "Response failed" } as Record<string, string>)[status] ?? status;
  const chosen = model && model !== "auto" ? `${m.settings?.auto ? "Auto · " : ""}${store.modelName(model)}` : "";
  return [label, chosen, usage].filter(Boolean).join(" · ");
}

/** What a finished reply ran with and what it cost, behind its Details button. */
export function detailCells(m: ChatMessage): [string, string, boolean?][] {
  if (m.role !== "assistant" || (!m.settings && !m.createdAt)) return [];
  const p = m.settings || {},
    usage = m.usage || {},
    count = (value: unknown) => (value != null ? Number(value).toLocaleString() : null);
  // Speed is output tokens over the time spent writing them; energy and CO₂ come from
  // the gateway when it measures them.
  const writing = Number(usage.generation_ms) || 0,
    written = Number(usage.completion_tokens) || 0,
    rate = writing >= 200 && written > 1 ? written / (writing / 1000) : 0;
  const figure = (value: unknown, unit: string, big: number, bigUnit: string) => {
    const n = Number(value);
    if (!(n > 0)) return null;
    const [v, u] = n >= big ? [n / big, bigUnit] : [n, unit];
    return `${v.toLocaleString(undefined, { maximumSignificantDigits: 3 })} ${u}`;
  };
  const cells: [string, unknown, boolean?][] = [
    ["Model", p.resolvedModel || p.model],
    ["Effort", p.reasoningEffort && effortLabel(p.reasoningEffort)],
    ["Temperature", p.temperature],
    ["Top P", p.topP],
    ["Output limit", p.maxTokens ? `${count(p.maxTokens)} tokens` : p.model && "The model’s own"],
    ["Input tokens", count(usage.prompt_tokens) && ((usage.steps || 0) > 1 ? `${count(usage.prompt_tokens)} over ${usage.steps} steps` : count(usage.prompt_tokens))],
    ["Output tokens", count(usage.completion_tokens)],
    ["Speed", rate ? `${rate >= 100 ? Math.round(rate).toLocaleString() : rate.toFixed(1)} tokens/s` : null],
    ["First token", usage.first_token_ms != null ? `${(Number(usage.first_token_ms) / 1000).toFixed(usage.first_token_ms < 10000 ? 2 : 1)} s` : null],
    ["Energy", figure(usage.energy_wh, "Wh", 1000, "kWh")],
    ["CO₂", figure(usage.co2_g, "g", 1000, "kg")],
    ["Finish", m.finishReason],
    ["Created", m.createdAt && new Date(m.createdAt).toLocaleString(), true],
    ["Request ID", m.requestId, true],
  ];
  return cells.filter(([, v]) => v != null && v !== "").map(([k, v, wide]) => [k, String(v), wide]);
}

function MessageDetails({ m, id }: { m: ChatMessage; id: string }) {
  const cells = detailCells(m);
  return (
    <div id={id} className="mt-2 rounded-lg border bg-card p-3">
      <div className="grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-4">
        {cells.map(([k, v, wide]) => (
          <div key={k} className={cn("min-w-0", wide && "col-span-2")}>
            <span className="block text-xs text-muted-foreground">{k}</span>
            <strong className="block truncate text-md font-medium" title={v}>
              {v}
            </strong>
          </div>
        ))}
      </div>
      {m.settings?.systemPrompt && (
        <div className="mt-3">
          <span className="text-xs text-muted-foreground">System instructions</span>
          <pre className="mt-1 max-h-48 overflow-auto rounded-md bg-code px-3 py-2 font-mono text-[12px] whitespace-pre-wrap">{m.settings.systemPrompt}</pre>
        </div>
      )}
    </div>
  );
}

function Versions({ m }: { m: ChatMessage }) {
  const store = useChat();
  const versions = m.versions || [];
  if (versions.length < 2 || store.shared) return null;
  const index = m.versionIndex || versions.findIndex((v) => v.id === m.id) + 1;
  const previous = responseVersion(m, -1),
    next = responseVersion(m, 1);
  const busy = store.streaming || store.switchingVersion;
  return (
    <div className="flex items-center gap-0.5 text-xs text-muted-foreground" role="group" aria-label={`${m.role === "user" ? "Message" : "Response"} versions`}>
      <IconButton label="Previous version" disabled={busy || !previous} onClick={() => store.selectMessageVersion(previous)}>
        <ChevronLeft />
      </IconButton>
      <span aria-label={`Version ${index} of ${versions.length}`}>
        {index} / {versions.length}
      </span>
      <IconButton label="Next version" disabled={busy || !next} onClick={() => store.selectMessageVersion(next)}>
        <ChevronRight />
      </IconButton>
    </div>
  );
}

function FollowUps({ m, index }: { m: ChatMessage; index: number }) {
  const store = useChat();
  if (store.shared || m.role !== "assistant" || m.status !== "completed" || index !== (store.active?.messages?.length || 0) - 1) return null;
  const questions = (m.followUps || []).map((q, i) => ({ q, i })).filter(({ q }) => typeof q === "string" && q.trim());
  const heading = <div className="mb-1.5 text-2xs font-medium tracking-wider text-muted-foreground uppercase">Suggested follow-ups</div>;
  if (questions.length)
    return (
      <section className="mt-4" aria-label="Suggested follow-up questions">
        {heading}
        <div className="grid gap-1">
          {questions.map(({ q, i }) => (
            <button
              key={i}
              type="button"
              disabled={store.streaming || store.switchingVersion}
              onClick={() => store.send(q)}
              className="flex items-center justify-between gap-3 rounded-md border px-3 py-2 text-left text-md hover:bg-accent disabled:opacity-50"
            >
              <span>{q}</span>
              <Plus className="size-4 shrink-0 text-muted-foreground" />
            </button>
          ))}
        </div>
      </section>
    );
  return m.followUpsStatus === "pending" ? (
    <section className="mt-4" role="status" aria-label="Finding follow-up ideas">
      {heading}
      <div className="grid gap-1" aria-hidden="true">
        {[0, 1, 2].map((i) => (
          <span key={i} className="h-9 animate-pulse rounded-md bg-skeleton" />
        ))}
      </div>
    </section>
  ) : null;
}

/** Under a reply that waits for a free slot, or that the gateway gave up on: why, and a model to switch to. */
function LoadNotice({ m, index }: { m: ChatMessage; index: number }) {
  const store = useChat();
  if (m.role !== "assistant" || store.shared) return null;
  const waiting = isPendingMessage(m) && !String(m.content || "").trim() && !m.reasoningContent && !(m.activity || []).length;
  const asked = store.active?.messages?.[index - 1];
  const notice = store.load.notice(m, { waitingSince: waiting ? Date.parse(m._began || m.createdAt || "") || 0 : 0, needsVision: (asked?.attachments || []).some((a) => a.kind === "image") });
  if (!notice) return null;
  return (
    <div className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-amber/40 bg-amber/5 px-3 py-2 text-md" role={notice.waiting ? "status" : undefined}>
      <Clock className="size-4 shrink-0 text-amber" />
      <p className="min-w-0 flex-1">{notice.text}</p>
      {notice.next && (
        <Button size="xs" variant="outline" onClick={() => store.run(() => store.switchWaiting(index, notice.next))}>
          {notice.label}
        </Button>
      )}
    </div>
  );
}

function MessageFiles({ m }: { m: ChatMessage }) {
  if (!m.attachments?.length) return null;
  return (
    <div className="mt-2 flex flex-wrap gap-2">
      {m.attachments.map((a) => {
        const d = describe(a),
          url = attachmentUrl(a);
        return (
          <a key={a.id} href={url} target="_blank" rel="noopener noreferrer" title={`Open ${a.name}`} className="flex max-w-full items-center gap-2 rounded-md border bg-card px-2.5 py-1.5 hover:bg-accent sm:max-w-72">
            {a.kind === "image" ? (
              // eslint-disable-next-line @next/next/no-img-element -- the person's own
              // upload
              <img src={url} alt={a.name} loading="lazy" className="size-10 shrink-0 rounded object-cover" />
            ) : (
              <NamedIcon name={d.glyph} className="size-4 shrink-0 text-muted-foreground" />
            )}
            <span className="min-w-0 flex-1">
              <strong className="block truncate text-sm font-medium">{a.name}</strong>
              <small className="block truncate text-xs text-muted-foreground">{d.detail}</small>
              {d.note && <small className="block text-xs text-muted-foreground">{d.note}</small>}
            </span>
            <ExternalLink className="size-3.5 shrink-0 text-faint" />
          </a>
        );
      })}
    </div>
  );
}

function AnswerContent({ m, index }: { m: ChatMessage; index: number }) {
  const store = useChat();
  // A reply that ends by asking questions has no answer text on purpose.
  const thinkingOnly =
    m.role === "assistant" &&
    m.status === "completed" &&
    m.reasoningContent &&
    !String(m.content || "").trim() &&
    !(m.activity || []).some((e) => ["question", "workspace", "research", "app"].includes(e.kind || ""));
  const canSave = store.feature("knowledge") && !store.active?.temporary;
  const project = useChatProjectId(store.shared ? null : (store.active?.id ?? null));
  return (
    <div className="min-w-0" data-answer>
      <Markdown
        text={m.content}
        cite={m.role === "assistant" ? (html) => citeHtml(html, m) : undefined}
        onSaveCode={canSave ? (item) => store.saveFromChat({ ...item, title: item.title || codeTitle(item.body) }, index, project) : undefined}
        onCopy={(text) => store.copy(text)}
      />
      {m.finishReason === "length" && String(m.content || "").trim() && (
        <p className="mt-2 rounded-md bg-muted px-3 py-2 text-sm text-text-2">
          {m.settings?.maxTokens
            ? "The response reached its output limit. Raise the limit in Parameters, or lower thinking effort to leave more room for the answer."
            : "The response filled the model’s context window. Start a new chat, or ask for a shorter answer."}
        </p>
      )}
      {m.errorMessage && m.status === "error" && <p className="mt-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive-foreground">{m.errorMessage}</p>}
      {thinkingOnly && <p className="mt-2 rounded-md bg-muted px-3 py-2 text-sm text-text-2">The model returned thinking without a final answer. Try increasing the output limit in Parameters or lowering thinking effort.</p>}
    </div>
  );
}

function Avatar({ text }: { text: string }) {
  return <span className="flex size-6 items-center justify-center rounded-full bg-secondary text-2xs font-semibold">{text}</span>;
}

/** Everything a message's look depends on, so an unchanged one is not drawn again. */
export function messageSignature(store: ChatStore, m: ChatMessage, index: number, count: number) {
  return JSON.stringify([
    m.id,
    m.status,
    m.content?.length,
    m.content?.slice(-40),
    m.reasoningContent?.length,
    m.finishReason,
    m.errorMessage,
    m.feedback,
    m.followUps,
    m.followUpsStatus,
    m.versionIndex,
    m.versions?.length,
    (m.activity || []).map((e) => [e.id, e.status, e.check, e.tick, e.agents?.length, e.items?.length]),
    m._draft?.lines,
    m._draft?.id,
    index === count - 1,
    store.streaming,
    store.switchingVersion,
    !!store.shared,
    m.settings?.resolvedModel,
    store.context,
  ]);
}

export const MessageView = memo(
  function MessageView({ m, index }: { m: ChatMessage; index: number; signature: string; version: number }) {
    const store = useChat();
    const branding = useBranding();
    const { user } = useSession();
    const [detailsOpen, setDetailsOpen] = useState(() => store.detailsOpen.has(m.id || `pending-${index}`));
    const key = m.id || `pending-${index}`;
    const shared = store.shared;
    const project = useChatProjectId(shared ? null : (store.active?.id ?? null));
    if (m.role === "user" && String(m.content || "").startsWith(NUDGE))
      return (
        <article className="flex items-center gap-2 py-2 text-sm text-muted-foreground" data-message={index}>
          <RefreshCw className="size-3.5" />
          <span>
            <strong className="font-medium text-foreground">Auto mode</strong> {m.content.slice(NUDGE.length)}
          </span>
        </article>
      );
    const assistant = m.role === "assistant";
    const work = assistant && appliesWork(m);
    const busy = store.streaming || store.switchingVersion;
    const name = user?.display_name || user?.username || "";
    const owner = shared?.ownerName || "?";
    // Settings' "Conversation context" shows or hides each answer's model and
    // generation details.
    const details = assistant && store.context && detailCells(m).length > 0;
    const isLast = index === (store.active?.messages?.length || 0) - 1;
    const rating = m.feedback?.rating || 0;
    const toggleDetails = () => {
      if (store.detailsOpen.has(key)) store.detailsOpen.delete(key);
      else store.detailsOpen.add(key);
      setDetailsOpen(store.detailsOpen.has(key));
    };
    return (
      // 32px apart; the name hangs in the gutter when the column is wide enough.
      <article className="group/message mb-8 max-[760px]:mb-[26px]" data-message={index} data-role={m.role}>
        <div className="mb-2 flex items-center gap-2 text-md font-semibold @min-[961px]:-ml-8 max-[760px]:ml-0">
          {m.role === "user" ? (
            <>
              <Avatar text={initials(shared ? owner : name)} />
              {shared ? owner : "You"}
            </>
          ) : (
            <>
              {branding.logo.light || branding.logo.dark ? <BrandLogo className="size-6" /> : branding.icons.assistant ? <NamedIcon name={branding.icons.assistant} className="size-5" /> : null}
              {branding.assistantName}
            </>
          )}
        </div>
        <div className={cn("min-w-0 text-base leading-[1.75] text-text-2 [overflow-wrap:anywhere]", m.role === "user" && "w-fit max-w-full rounded-lg border bg-card px-3.5 py-2.5 text-foreground")}>
          {assistant &&
            (work ? (
              <>
                <TurnBefore m={m} />
                {isLive(m) && !shared && <SteerForm m={m} />}
              </>
            ) : (
              <>
                <ThinkingPanel m={m} index={index} />
                <MessageActivity m={m} />
              </>
            ))}
          <AnswerContent m={m} index={index} />
          {assistant && (
            <>
              <LoadNotice m={m} index={index} />
              <KnowledgeSources message={m} />
              <SavedNotes message={m} />
              <QuestionCard m={m} index={index} />
              <WorkspaceOffer m={m} index={index} />
              <ResearchOffer m={m} index={index} />
              <AppOffer m={m} index={index} />
              <BrowserCard m={m} index={index} isLast={isLast} streaming={store.streaming} />
            </>
          )}
          {work && <TurnAfter m={m} />}
          <MessageFiles m={m} />
        </div>
        <div className="mt-2 -ml-1.5 flex min-h-[30px] flex-wrap items-center gap-0.5 max-[760px]:flex-nowrap">
          {assistant ? (
            <>
              <IconButton label="Copy response" onClick={() => store.copy(m.content)}>
                <Copy />
              </IconButton>
              {m.id && store.feature("knowledge") && m.status === "completed" && !shared && !store.active?.temporary && (
                <IconButton label="Save to Library" onClick={() => store.saveFromChat({ kind: "excerpt", body: m.content, title: store.active?.title }, index, project)}>
                  <Bookmark />
                </IconButton>
              )}
              {ratable(m, shared) && (
                <>
                  <IconButton label="Good response" pressed={rating === 1} onClick={() => store.run(() => store.rate(index, 1))}>
                    <ThumbsUp />
                  </IconButton>
                  <IconButton label="Bad response" pressed={rating === -1} onClick={() => store.run(() => store.rateDown(index))}>
                    <ThumbsDown />
                  </IconButton>
                </>
              )}
              {m.id && !busy && !shared && (
                <>
                  <IconButton label={m.status === "completed" ? "Regenerate response" : "Retry response"} onClick={() => store.run(() => store.forkAndSend(m.id!, "regenerate"))}>
                    <RefreshCw />
                  </IconButton>
                  {!store.active?.temporary && (
                    <IconButton label="Branch conversation" onClick={() => store.run(() => store.branch(index))}>
                      <GitBranch />
                    </IconButton>
                  )}
                </>
              )}
              {details && (
                <IconButton label="Response details" aria-controls={`details-${key}`} aria-expanded={detailsOpen} onClick={toggleDetails}>
                  <Info />
                </IconButton>
              )}
            </>
          ) : (
            m.id &&
            !busy &&
            !shared && (
              <IconButton label="Edit and resend message" onClick={() => store.openDialog({ kind: "edit", messageId: m.id!, content: m.content, required: !m.attachments?.length })}>
                <Pencil />
              </IconButton>
            )
          )}
          <Versions m={m} />
          {assistant && (
            <span
              className={cn("ml-1 text-xs text-muted-foreground", isPendingMessage(m) && "animate-pulse", m.status === "error" && "text-destructive-foreground")}
              role={isPendingMessage(m) ? "status" : undefined}
            >
              {responseStatus(store, m)}
            </span>
          )}
        </div>
        {details && detailsOpen && <MessageDetails m={m} id={`details-${key}`} />}
        <FollowUps m={m} index={index} />
      </article>
    );
  },
  (a, b) => a.m === b.m && a.index === b.index && a.signature === b.signature && a.version === b.version,
);
