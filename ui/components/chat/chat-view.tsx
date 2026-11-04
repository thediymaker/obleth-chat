"use client";

// The home screen with its composer, a conversation, and someone else's chat shared
// with the viewer.

import { ChevronRight, GitBranch, Link2, LockKeyhole, MessageSquare, MoreHorizontal, PanelLeftClose, PanelRight, Plus, RefreshCw, Server } from "lucide-react";
import Link from "next/link";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { Copy as BrandCopy } from "@/components/brand";
import { KnowledgePill } from "@/components/knowledge/chat-knowledge";
import { ProjectPill } from "@/components/projects/chat-project";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuShortcut, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { api } from "@/lib/api";
import { useBranding } from "@/lib/branding-context";
import type { Attachment } from "@/lib/chat/types";
import { useConversations } from "@/lib/queries";
import { ChatMenuItems } from "./chat-menu";
import { NEW_CHAT_EVENT, OPEN_SEARCH_EVENT, useChat } from "./chat-provider";
import { Composer, TemporaryNote } from "./composer";
import { NamedIcon } from "./icon";
import { MessageView, messageSignature } from "./message";
import { SelectionBar } from "./selection-bar";
import { SidePane } from "./side-pane";

function Alert({ message, onRetry }: { message: string; onRetry?: () => void }) {
  if (!message) return null;
  return (
    <div role="alert" className="mb-2.5 flex items-center gap-3 rounded-lg border border-amber/20 bg-amber/[0.04] px-4 py-3 text-md leading-[1.65] text-amber [overflow-wrap:anywhere]">
      <Server className="size-4 shrink-0" />
      <span className="grow">{message}</span>
      {onRetry && (
        <Button size="sm" variant="outline" onClick={onRetry}>
          <RefreshCw /> Retry
        </Button>
      )}
    </div>
  );
}

/** The scroll column of a chat page, kept at its end on opening and while a reply is written. */
function useScroller() {
  const store = useChat();
  // A callback ref: the column only appears once the conversation has loaded.
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  const pinUntil = useRef(0);
  useLayoutEffect(() => {
    store.attachScroller(el);
    return () => store.attachScroller(null);
  }, [store, el]);
  useLayoutEffect(() => {
    if (!el) return;
    if (store.wantsEnd || (store.streaming && store.follow?.on())) {
      el.scrollTo({ top: el.scrollHeight });
      if (store.wantsEnd) pinUntil.current = Date.now() + 2000;
      store.wantsEnd = false;
    }
  });
  // Late layout (fonts, math, diagrams, a growing composer) keeps the end in view
  // while following; scrolling up stops it.
  useEffect(() => {
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if ((store.streaming || Date.now() < pinUntil.current) && store.follow?.on() !== false) el.scrollTo({ top: el.scrollHeight });
    });
    Array.from(el.children).forEach((child) => observer.observe(child));
    const content = el.querySelector("#messages");
    if (content) observer.observe(content);
    return () => observer.disconnect();
  }, [store, el, store.active?.id]);
  return setEl;
}

function Messages() {
  const store = useChat();
  const started = performance.now();
  const messages = store.active?.messages || [];
  useLayoutEffect(() => {
    store.reportPaint(performance.now() - started);
  });
  return (
    <div id="messages">
      {messages.map((m, index) => (
        <MessageView
          key={m.id || `pending-${index}`}
          m={m}
          index={index}
          signature={messageSignature(store, m, index, messages.length)}
          // The newest messages also follow the rest of the page (load, offers,
          // follow-ups).
          version={index >= messages.length - 2 ? store.version : 0}
        />
      ))}
    </div>
  );
}

const STARTERS = (workspaces: boolean): [string, string, string, string, boolean][] => [
  ...(workspaces
    ? ([
        ["terminal", "Build an app", "Describe it, then watch it run in a live preview.", "Build me a site for ", true],
        ["branch", "Explore a repo", "Get a tour of a codebase, or a list of its issues.", "Explain this codebase to me: ", true],
      ] as [string, string, string, string, boolean][])
    : ([["code", "Build something", "Go from a question to working code.", "Help me plan a small coding project. Start by asking what I want to build.", false]] as [string, string, string, string, boolean][])),
  ["chat", "Think it through", "Find a fresh way into a tricky problem.", "Help me think through a problem. Ask me what I am working on.", false],
  ...(workspaces ? [] : ([["book", "Learn something", "Make a complex idea feel approachable.", "Help me learn something new. Ask what topic interests me.", false]] as [string, string, string, string, boolean][])),
];

/** Settings links here with ?attach=<file> (Your files' Attach) and ?preset=<id> (a preset's Use). */
function useHomeParams() {
  const store = useChat();
  const params = useSearchParams();
  const router = useRouter();
  const attach = params.get("attach"),
    preset = params.get("preset");
  useEffect(() => {
    if (!attach && !preset) return;
    if (preset && !store.presets.length) return; // Waits for the presets to load.
    if (preset) store.applyPreset(store.presets.find((p) => p.id === preset));
    if (attach)
      api
        .request<{ attachments?: Attachment[] }>("/api/attachments")
        .then((r) => {
          const file = (r.attachments || []).find((a) => a.id === attach);
          if (file && store.attachments.attachExisting(file)) toast("File attached to your next message");
        })
        .catch((e: Error) => toast(e.message));
    router.replace("/");
  }, [attach, preset, store, store.presets, router]);
}

export function ChatHome() {
  const store = useChat();
  const branding = useBranding();
  const { data: chats = [] } = useConversations();
  // The page re-renders with the chrome, so its actions stay the same element.
  usePageChrome("Chat", useMemo(() => <FocusToggle />, []));
  useHomeParams();
  const workspaces = store.workspace.enabled();
  return (
    // 820px (900px from 1600px wide), 72px down (110px from 1600px, 56px up to 1200px).
    <div
      data-chat-home
      className="mx-auto w-full max-w-[820px] px-7 pt-[72px] leading-[normal] pb-10 max-[1200px]:pt-14 max-[760px]:px-5 max-[760px]:pt-11 max-[760px]:pb-[26px] min-[1600px]:max-w-[900px] min-[1600px]:pt-[110px]"
    >
      <SelectionBar />
      <div className="mb-[30px]">
        {branding.home.eyebrow && (
          <div className="mb-5 flex items-center gap-2 text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase">
            {branding.icons.greeting && <NamedIcon name={branding.icons.greeting} className="size-4" />}
            <BrandCopy text={branding.home.eyebrow} />
          </div>
        )}
        {branding.home.title && (
          <h1 className="text-[34px] leading-[1.2] font-semibold tracking-[-1px] max-[760px]:text-[28px] min-[1600px]:text-[38px]">
            <BrandCopy text={branding.home.title} />
          </h1>
        )}
        {branding.home.description && (
          <p className="mt-2.5 text-base leading-[1.65] text-muted-foreground">
            <BrandCopy text={branding.home.description} />
          </p>
        )}
      </div>
      <div className="grid gap-2.5">
        <Alert message={store.catalogError || store.error} onRetry={() => store.reload()} />
        {store.temporaryDraftOn() && <TemporaryNote started={false} />}
        <Composer home />
      </div>
      <div className="mt-6 grid grid-cols-3 gap-3 max-[760px]:gap-2">
        {STARTERS(workspaces).map(([icon, title, copy, prompt, space]) => (
          <button
            key={title}
            type="button"
            data-starter
            onClick={() => store.starter(prompt, space)}
            className="flex flex-col items-start rounded-lg border bg-card px-4 py-[18px] text-left transition-colors hover:border-line-strong hover:bg-raised max-[760px]:px-3 max-[760px]:py-3.5"
          >
            <span className="mb-3.5 flex size-[30px] items-center justify-center rounded-md border bg-background text-text-2">
              <NamedIcon name={icon} className="size-[18px]" />
            </span>
            <h3 className="mb-1.5 text-base leading-[normal] font-medium">{title}</h3>
            <p className="text-sm leading-[1.6] text-muted-foreground">{copy}</p>
          </button>
        ))}
      </div>
      <section className="mt-12 max-[760px]:mt-[34px] min-[1600px]:mt-[60px]">
        <div className="mb-3.5 flex items-center justify-between gap-2">
          <h2 className="text-base leading-[normal] font-medium tracking-normal">{chats.length ? "Pick up where you left off" : "A fresh start, just for you."}</h2>
          {chats.length > 0 && (
            <Button variant="ghost" size="sm" onClick={() => window.dispatchEvent(new Event(OPEN_SEARCH_EVENT))}>
              All conversations <ChevronRight />
            </Button>
          )}
        </div>
        {chats.length ? (
          <div className="border-t">
            {chats.slice(0, 4).map((c) => (
              <Link key={c.id} href={`/chat/${encodeURIComponent(c.id)}`} data-resume className="group/resume flex w-full items-center gap-3 border-b border-line-soft px-0.5 py-3.5 text-left">
                <MessageSquare className="size-4 shrink-0 text-faint" />
                <div className="min-w-0 flex-1">
                  <div className="mb-[3px] truncate text-base leading-[normal] text-text-2 group-hover/resume:text-foreground">{c.title}</div>
                  <div className="truncate text-sm leading-[normal] text-faint">{store.modelName(c.model)}</div>
                </div>
                <ChevronRight className="size-3.5 text-faint" />
              </Link>
            ))}
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">Your conversations appear here after your first message.</p>
        )}
      </section>
      {branding.home.footer && (
        <div className="mt-7 flex items-center justify-center gap-[7px] text-xs text-faint">
          <BrandCopy text={branding.home.footer} />
        </div>
      )}
    </div>
  );
}

/** The top bar's Focus mode button: hides the main navigation, saved to the account. */
export function FocusToggle() {
  const store = useChat();
  return (
    <Button variant="ghost" size="sm" className="max-[760px]:hidden" aria-pressed={store.focus} onClick={() => store.run(() => store.savePreference({ focus: !store.focus }))}>
      <PanelLeftClose /> <span className="max-[1000px]:hidden">Focus mode</span>
    </Button>
  );
}

function HeaderButton({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon-sm" className="size-[30px] [&_svg]:size-4" aria-label={label} onClick={onClick}>
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function DetailsButton() {
  const store = useChat();
  const id = store.route.id;
  return (
    <Button variant="ghost" size="sm" className="max-[760px]:hidden" aria-pressed={store.showing(id, "details")} onClick={() => store.toggleDetails(id)} title="Chat details (⌘.)">
      <PanelRight /> <span className="max-[1000px]:hidden">Details</span>
    </Button>
  );
}

export function ChatPage({ id }: { id: string }) {
  const store = useChat();
  const scroller = useScroller();
  const c = store.active?.id === id ? store.active : null;
  const readOnly = !!store.shared || !c;
  usePageChrome(c?.title || "Chat", useMemo(() => (readOnly ? <FocusToggle /> : <DetailsButton />), [readOnly]), "Chat");
  // ⌘. opens and closes the chat's details.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "." && !store.shared) {
        e.preventDefault();
        store.toggleDetails(id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store, id]);
  if (store.loadingChat && !c)
    return (
      <div className="py-16 text-center text-sm text-muted-foreground" role="status">
        Opening your conversation…
      </div>
    );
  if (store.chatError && !c)
    return (
      <div className="mx-auto grid max-w-3xl gap-3 px-4 py-8">
        <Alert message={store.chatError} onRetry={() => store.reload()} />
        <Button variant="outline" className="justify-self-start" asChild>
          <Link href="/" onClick={() => window.dispatchEvent(new Event(NEW_CHAT_EVENT))}>
            <Plus /> Start a new chat
          </Link>
        </Button>
      </div>
    );
  if (!c)
    return (
      <div className="py-16 text-center text-sm text-muted-foreground" role="status">
        Opening your conversation…
      </div>
    );
  const sh = store.shared;
  const empty = !c.messages?.length;
  return (
    <div className="flex h-full min-h-0">
      <SelectionBar />
      {/* The header spans it; messages, the chat line and the
          composer share an 884px column (820px of text and 32px gutters). */}
      <div ref={scroller} className="@container flex min-w-0 flex-1 flex-col overflow-y-auto leading-[normal]">
        <div data-conversation-header className="flex h-[52px] shrink-0 items-center justify-between gap-4 border-b border-line-soft px-6 max-[760px]:h-12 max-[760px]:px-4">
          <div className="flex min-w-0 items-center gap-[9px]">
            <h2 className="truncate text-base leading-[normal] font-medium tracking-normal text-text-2">{c.title}</h2>
            {sh && <Badge variant="secondary">Shared by {sh.ownerName}</Badge>}
            {!sh && c.temporary && <Badge variant="secondary">Temporary</Badge>}
            {!sh && c.archived && <Badge variant="secondary">Archived</Badge>}
            {!sh && c.shareScope && c.shareScope !== "none" && (
              <button type="button" title="Change who can see it" onClick={() => store.openDialog({ kind: "share", id: c.id })}>
                <Badge>Shared</Badge>
              </button>
            )}
          </div>
          {sh ? (
            <Button size="sm" variant="outline" onClick={() => store.run(() => store.forkShared())}>
              <GitBranch /> Fork
            </Button>
          ) : (
            <div className="flex shrink-0 items-center gap-1">
              {!c.temporary && (
                <>
                  <HeaderButton label="Share chat" onClick={() => store.openDialog({ kind: "share", id: c.id })}>
                    <Link2 />
                  </HeaderButton>
                  <HeaderButton label="Conversation alternatives" onClick={() => store.run(() => store.alternatives())}>
                    <GitBranch />
                  </HeaderButton>
                </>
              )}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon-sm" className="size-[30px]" aria-label="Conversation options">
                    <MoreHorizontal />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <ChatMenuItems chat={c} />
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          )}
        </div>
        {!sh && (
          <div className="mx-auto flex w-full max-w-[884px] flex-wrap items-center gap-1.5 px-8 pt-3 empty:hidden max-[760px]:px-4">
            {!c.temporary && (
              <ProjectPill conversationId={c.id}>
                <DropdownMenuItem onSelect={() => store.toggleDetails(id)}>
                  <PanelRight /> Chat details <DropdownMenuShortcut>⌘.</DropdownMenuShortcut>
                </DropdownMenuItem>
              </ProjectPill>
            )}
            <KnowledgePill conversationId={c.id} disabled={store.streaming} />
          </div>
        )}
        <div className="mx-auto w-full max-w-[884px] flex-1 px-8 pt-7 pb-2 max-[760px]:px-4 max-[760px]:pt-5">
          <Messages />
          {empty && <div className="px-5 py-[45px] text-center text-md text-muted-foreground">{sh ? "Nothing here yet. New messages show up as they arrive." : "A new conversation, ready for your first idea."}</div>}
        </div>
        {sh ? (
          <div className="sticky bottom-3 z-[5] mx-auto mt-2 mb-3 flex w-[min(calc(100%-32px),820px)] items-center gap-2.5 rounded-lg border border-line-strong bg-card px-3.5 py-2.5 text-md text-text-2 shadow-[0_6px_20px_var(--shadow-soft)]">
            <LockKeyhole className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1">
              You are reading {sh.ownerName}’s chat, {sh.scope === "group" && sh.groupName ? `shared with ${sh.groupName}` : "shared by link"}. It updates as they keep going.
            </span>
            <Button size="sm" onClick={() => store.run(() => store.forkShared())}>
              <GitBranch /> Fork to continue
            </Button>
          </div>
        ) : (
          <div className="sticky bottom-0 z-[5] mx-auto grid w-full max-w-[884px] gap-2.5 bg-[linear-gradient(transparent,var(--background)_18px)] px-8 pt-4 pb-3 max-[760px]:px-4 max-[760px]:pt-3">
            <Alert message={store.catalogError || store.error} onRetry={() => store.reload()} />
            {c.temporary && <TemporaryNote />}
            <Composer />
          </div>
        )}
      </div>
      <SidePane id={id} />
    </div>
  );
}

/** /chat/[id]: the conversation in the URL. */
export function ChatRoute() {
  const { id } = useParams<{ id: string }>();
  return <ChatPage id={decodeURIComponent(id || "")} />;
}
