"use client";

// The scratch pad: an icon in the top bar (⇧⌘X, or "Open the scratch pad" in ⌘K)
// that opens a box for jotting something down from any page, and the person's recent
// jottings under it. What is typed can be kept on the pad, saved as a note, made a
// card on a project's board, or taken into a new chat (Ask, or Home's Research).
// A popover under the icon on a desktop, a sheet from the bottom on a phone.

import { useQueryClient } from "@tanstack/react-query";
import { Bookmark, ChevronDown, FlaskConical, MessageSquare, MoreHorizontal, Pencil, SquareKanban, StickyNote, Trash2 } from "lucide-react";
import { usePathname, useRouter } from "next/navigation";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { projectDot } from "@/components/app-shell/sidebar";
import { refreshBoard, createCard } from "@/components/board/api";
import { useChatStore } from "@/components/chat/chat-provider";
import { refreshSaved } from "@/components/library/api";
import { FormError, messageOf, useConfirm } from "@/components/manage/kit";
import { useConversationProject } from "@/components/projects/api";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { api } from "@/lib/api";
import { chatDraft } from "@/lib/chat/chat-draft";
import { ago, when } from "@/lib/format";
import { type ProjectSummary, useFeature, useProjects } from "@/lib/queries";
import { MAX_TEXT, SHORTCUT, cardOf, pageOf, preview, readDraft, writeDraft } from "@/lib/scratch";
import { useUser } from "@/lib/session";
import { cn } from "@/lib/utils";
import { type Jotting, PAGE, addJotting, changeJotting, deleteJotting, refreshJottings, scratchKey, useJottings } from "./api";

const PHONE = "(max-width: 760px)";
/** Whether the window is phone-sized, as the top bar's own breakpoint. */
function usePhone() {
  return useSyncExternalStore(
    (change) => {
      const query = window.matchMedia(PHONE);
      query.addEventListener?.("change", change);
      return () => query.removeEventListener?.("change", change);
    },
    () => window.matchMedia(PHONE).matches,
    () => false,
  );
}

/** The top bar's icon and the pop-up it opens; `open` is the shell's, so the shortcut and ⌘K can open it too. */
export function ScratchPad({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const phone = usePhone();
  const icon = useRef<HTMLButtonElement>(null);
  // Escape while a jotting is being edited ends the edit; otherwise it closes the pop-up.
  const escape = useRef<() => boolean>(() => false);
  const onEscape = (e: KeyboardEvent) => {
    if (escape.current()) e.preventDefault();
  };
  // Closing puts focus back on the icon, also when the shortcut opened it; not when
  // an action went to a new chat, whose box takes it.
  const leaving = useRef(false);
  const backToIcon = (e: Event) => {
    e.preventDefault();
    if (!leaving.current) icon.current?.focus({ preventScroll: true });
    leaving.current = false;
  };
  const close = (away = false) => {
    leaving.current = away;
    onOpenChange(false);
  };
  const button = (
    <Button ref={icon} variant="ghost" size="icon-sm" aria-label="Scratch pad" aria-keyshortcuts="Meta+Shift+X Control+Shift+X" data-scratch-icon onClick={phone ? () => onOpenChange(!open) : undefined}>
      <StickyNote />
    </Button>
  );
  const tip = (
    <TooltipContent>
      Scratch pad <kbd className="ml-1 font-sans opacity-70">{SHORTCUT.label}</kbd>
    </TooltipContent>
  );

  if (phone)
    return (
      <>
        <Tooltip>
          <TooltipTrigger asChild>{button}</TooltipTrigger>
          {tip}
        </Tooltip>
        <Sheet open={open} onOpenChange={onOpenChange}>
          <SheetContent side="bottom" className="max-h-[90dvh] gap-0 rounded-t-xl p-0 pb-[env(safe-area-inset-bottom)]" onEscapeKeyDown={onEscape} onCloseAutoFocus={backToIcon}>
            <SheetTitle className="px-4 pt-4 pr-12 text-md font-medium">Scratch pad</SheetTitle>
            <SheetDescription className="sr-only">Jot something down, then keep it or turn it into a note, a card or a chat.</SheetDescription>
            {open && <Panel close={close} escape={escape} />}
          </SheetContent>
        </Sheet>
      </>
    );
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>{button}</PopoverTrigger>
        </TooltipTrigger>
        {tip}
      </Tooltip>
      <PopoverContent
        align="end"
        collisionPadding={8}
        aria-label="Scratch pad"
        className="flex max-h-[min(640px,calc(100dvh-80px))] w-[400px] max-w-[calc(100vw-16px)] flex-col p-0"
        onEscapeKeyDown={onEscape}
        onCloseAutoFocus={backToIcon}
      >
        <div className="flex items-center justify-between px-4 pt-3">
          <h2 className="text-md font-medium">Scratch pad</h2>
          <kbd className="font-sans text-xs text-faint">{SHORTCUT.label}</kbd>
        </div>
        <Panel close={close} escape={escape} />
      </PopoverContent>
    </Popover>
  );
}

type Busy = "" | "save" | "note" | "card" | `jotting:${string}`;

/** The box, its project, its actions and the recent jottings. */
function Panel({ close, escape }: { close: (away?: boolean) => void; escape: React.RefObject<() => boolean> }) {
  const user = useUser();
  const client = useQueryClient();
  const router = useRouter();
  const store = useChatStore();
  const notes = useFeature("knowledge");
  const projects = useProjects().data?.projects ?? [];
  const page = pageOf(usePathname());
  const chatProject = useConversationProject(page.chat).data?.id ?? null;
  // Where the box's text belongs: the page's project until the person picks another or none.
  const [picked, setPicked] = useState<string | null | undefined>(undefined);
  const wanted = picked === undefined ? page.project || chatProject : picked;
  const project = projects.find((p) => p.id === wanted) ?? null;
  const [choosing, setChoosing] = useState<"" | "any" | "card">("");

  const [text, setShown] = useState(() => readDraft(user.id));
  // Kept as it changes, not in an effect: an action that closes the pop-up unmounts
  // the box in the same render that clears it.
  const setText = (next: string) => {
    setShown(next);
    writeDraft(user.id, next);
  };
  const box = useRef<HTMLTextAreaElement>(null);
  // The caret goes after a kept draft.
  useEffect(() => {
    const el = box.current;
    if (el) el.setSelectionRange(el.value.length, el.value.length);
  }, []);
  const typed = text.trim();

  const [busy, setBusy] = useState<Busy>("");
  const [error, setError] = useState("");
  const [editing, setEditing] = useState<{ id: string; text: string; projectId: string | null } | null>(null);
  useEffect(() => {
    escape.current = () => {
      if (!editing) return false;
      setEditing(null);
      return true;
    };
  });
  const confirm = useConfirm();
  const [limit, setLimit] = useState(PAGE);
  const list = useJottings(limit, true);
  const jottings = list.data?.items ?? [];

  const nameOf = (id: string | null) => projects.find((p) => p.id === id)?.name ?? null;

  async function run(what: Busy, work: () => Promise<unknown>) {
    setBusy(what);
    setError("");
    try {
      await work();
      return true;
    } catch (e) {
      setError(messageOf(e));
      return false;
    } finally {
      setBusy("");
    }
  }
  const cleared = () => {
    setText("");
    box.current?.focus();
  };

  // What each action does, for the box and for a jotting alike.
  async function asNote(body: string, projectId: string | null) {
    const { item } = await api.request<{ item: { id: string } }>("/api/saved", { method: "POST", body: { kind: "note", body, projectId } });
    refreshSaved(client);
    const name = nameOf(projectId);
    toast(name ? `Saved as a note in ${name}` : "Saved as a note in your Library", { action: { label: "Open", onClick: () => router.push(`/library/${encodeURIComponent(item.id)}`) } });
  }
  async function asCard(body: string, projectId: string) {
    await createCard(projectId, cardOf(body));
    refreshBoard(client, projectId);
    toast(`Added to the board of ${nameOf(projectId) ?? "the project"}`, { action: { label: "Open", onClick: () => router.push(`/projects/${encodeURIComponent(projectId)}/board`) } });
  }
  /** A new chat with the text in its box, not sent; in the project if any, on Home's Research tab for research. */
  function startChat(body: string, projectId: string | null, research: boolean) {
    store.newChat();
    if (projectId) chatDraft.setProject(projectId);
    if (research) chatDraft.setIntent("research");
    store.starter(body, false);
    close(true);
    router.push("/");
    const name = nameOf(projectId);
    toast(`Your text is in a new ${research ? "Research " : ""}chat${name ? ` in ${name}` : ""}, ready to send`);
  }

  // The box.
  const save = () =>
    typed &&
    !busy &&
    run("save", async () => {
      const jotting = await addJotting(typed, project?.id ?? null);
      client.setQueryData<{ items: Jotting[]; more: boolean }>([...scratchKey, limit], (old) => (old ? { ...old, items: [jotting, ...old.items.filter((j) => j.id !== jotting.id)] } : old));
      refreshJottings(client);
      cleared();
      toast("Saved to your scratch pad");
    });
  const note = () =>
    run("note", async () => {
      await asNote(typed, project?.id ?? null);
      cleared();
    });
  const card = (projectId = project?.id) => {
    if (!projectId) return setChoosing("card");
    return run("card", async () => {
      await asCard(typed, projectId);
      cleared();
    });
  };
  const chat = (research: boolean) => {
    startChat(typed, project?.id ?? null, research);
    setText("");
  };
  const choose = (id: string | null) => {
    const forCard = choosing === "card";
    setPicked(id);
    setChoosing("");
    if (forCard && id) void card(id);
  };

  // A jotting's actions. A note or a card takes it off the pad; a chat keeps it.
  const away = (j: Jotting, work: () => Promise<unknown>) =>
    run(`jotting:${j.id}`, async () => {
      await work();
      await deleteJotting(j.id);
      client.setQueryData<{ items: Jotting[]; more: boolean }>([...scratchKey, limit], (old) => (old ? { ...old, items: old.items.filter((x) => x.id !== j.id) } : old));
      refreshJottings(client);
    });
  const remove = (j: Jotting) =>
    confirm.ask({
      title: "Delete this jotting?",
      text: `“${preview(j.text)}” is deleted for good.`,
      label: "Delete",
      cancel: "Keep it",
      action: async () => {
        await deleteJotting(j.id);
        refreshJottings(client);
        toast("Jotting deleted");
      },
    });
  const saveEdit = () =>
    editing &&
    run(`jotting:${editing.id}`, async () => {
      await changeJotting(editing.id, { text: editing.text.trim(), projectId: editing.projectId });
      setEditing(null);
      refreshJottings(client);
    });

  const disabled = !typed || !!busy;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 px-4 pt-2 pb-3">
        <label htmlFor="scratch-text" className="sr-only">
          Jot something down
        </label>
        <Textarea
          id="scratch-text"
          ref={box}
          autoFocus
          value={text}
          maxLength={MAX_TEXT}
          placeholder="Jot something down…"
          className="max-h-60 min-h-[76px] resize-none"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void save();
            }
          }}
        />
        <div className="flex min-w-0 items-center gap-2">
          <ProjectChip
            projects={projects}
            value={project?.id ?? null}
            onChange={choose}
            open={!!choosing}
            onOpenChange={(o) => setChoosing(o ? "any" : "")}
            forCard={choosing === "card"}
          />
          <Button size="sm" className="ml-auto shrink-0" disabled={disabled} onClick={() => void save()} title="Save to your scratch pad (⌘↵)">
            Save <kbd className="font-sans text-xs opacity-70 max-[760px]:hidden">⌘↵</kbd>
          </Button>
        </div>
        <div className="flex flex-wrap gap-1" role="group" aria-label="Other ways to keep it">
          {notes && (
            <Button variant="outline" size="xs" disabled={disabled} onClick={() => void note()}>
              <Bookmark /> Save as a note
            </Button>
          )}
          <Button variant="outline" size="xs" disabled={disabled} onClick={() => void card()}>
            <SquareKanban /> Make a card
          </Button>
          <Button variant="outline" size="xs" disabled={disabled} onClick={() => chat(false)}>
            <MessageSquare /> Ask
          </Button>
          <Button variant="outline" size="xs" disabled={disabled} onClick={() => chat(true)}>
            <FlaskConical /> Research this
          </Button>
        </div>
        <FormError message={error} />
      </div>
      <section aria-label="Your jottings" className="min-h-0 flex-1 overflow-y-auto border-t max-[760px]:max-h-[40dvh] min-[761px]:max-h-[284px]">
        {list.isPending ? (
          <p role="status" className="px-4 py-4 text-sm text-muted-foreground">
            Loading your jottings…
          </p>
        ) : list.error ? (
          <p role="alert" className="px-4 py-4 text-sm text-destructive-foreground">
            {messageOf(list.error)}
          </p>
        ) : !jottings.length ? (
          <p className="px-4 py-4 text-sm text-muted-foreground">What you save shows here, on every device you sign in on.</p>
        ) : (
          <ul className="divide-y divide-line-soft">
            {jottings.map((j) =>
              editing?.id === j.id ? (
                <li key={j.id} className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 px-4 py-2.5">
                  <label htmlFor={`scratch-edit-${j.id}`} className="sr-only">
                    Edit the jotting
                  </label>
                  <Textarea
                    id={`scratch-edit-${j.id}`}
                    autoFocus
                    value={editing.text}
                    maxLength={MAX_TEXT}
                    className="max-h-48 min-h-16 resize-none"
                    onChange={(e) => setEditing({ ...editing, text: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.nativeEvent.isComposing) {
                        e.preventDefault();
                        void saveEdit();
                      }
                    }}
                  />
                  <div className="flex min-w-0 items-center gap-2">
                    <ProjectChip projects={projects} value={editing.projectId} onChange={(id) => setEditing({ ...editing, projectId: id })} />
                    <Button variant="ghost" size="sm" className="ml-auto" onClick={() => setEditing(null)}>
                      Cancel
                    </Button>
                    <Button size="sm" disabled={!editing.text.trim() || !!busy} onClick={() => void saveEdit()}>
                      Save changes
                    </Button>
                  </div>
                </li>
              ) : (
                <li key={j.id} className="flex items-start gap-2 py-2.5 pr-2 pl-4" data-jotting={j.id}>
                  <div className="min-w-0 flex-1">
                    <p className="line-clamp-3 text-sm break-words whitespace-pre-wrap">{j.text}</p>
                    <p className="mt-1 flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
                      <time dateTime={j.createdAt} title={when(j.createdAt)} className="shrink-0">
                        {ago(j.createdAt)}
                      </time>
                      {j.projectName && (
                        <>
                          <span aria-hidden="true">·</span>
                          <span className={cn("size-1.5 shrink-0 rounded-full", projectDot(j.projectColor || "gray"))} aria-hidden="true" />
                          <span className="truncate">{j.projectName}</span>
                        </>
                      )}
                    </p>
                  </div>
                  <JottingMenu
                    jotting={j}
                    projects={projects}
                    notes={notes}
                    disabled={!!busy}
                    onNote={() => void away(j, () => asNote(j.text, j.projectId))}
                    onCard={(projectId) => void away(j, () => asCard(j.text, projectId))}
                    onChat={(research) => startChat(j.text, j.projectId, research)}
                    onEdit={() => setEditing({ id: j.id, text: j.text, projectId: j.projectId })}
                    onDelete={() => remove(j)}
                  />
                </li>
              ),
            )}
          </ul>
        )}
        {list.data?.more && (
          <div className="border-t border-line-soft px-4 py-2">
            <Button variant="ghost" size="sm" className="w-full" disabled={list.isFetching} onClick={() => setLimit((n) => n + PAGE)}>
              Show more
            </Button>
          </div>
        )}
      </section>
      {confirm.element}
    </div>
  );
}

/** Where the text belongs: "In <project>" or "Not in a project"; opens a list of the person's projects. */
function ProjectChip({
  projects,
  value,
  onChange,
  open,
  onOpenChange,
  forCard = false,
}: {
  projects: ProjectSummary[];
  value: string | null;
  onChange: (id: string | null) => void;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  forCard?: boolean;
}) {
  const current = projects.find((p) => p.id === value) ?? null;
  const label = current ? `In ${current.name}` : "Not in a project";
  return (
    <DropdownMenu open={open} onOpenChange={onOpenChange} modal={false}>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="xs" className="max-w-[60%] min-w-0 rounded-full px-2.5" title={label} aria-label={`${label}, choose a project`} data-project-chip>
          {current && <span className={cn("size-2 shrink-0 rounded-full", projectDot(current.color))} aria-hidden="true" />}
          <span className="truncate">{label}</span>
          <ChevronDown className="shrink-0" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-72 w-64 overflow-y-auto">
        {forCard && <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">A card goes on a project’s board. Which project?</DropdownMenuLabel>}
        <DropdownMenuRadioGroup value={value ?? "none"} onValueChange={(v) => onChange(v === "none" ? null : v)}>
          {!forCard && <DropdownMenuRadioItem value="none">Not in a project</DropdownMenuRadioItem>}
          {projects.map((p) => (
            <DropdownMenuRadioItem key={p.id} value={p.id}>
              <span className={cn("size-2 shrink-0 rounded-full", projectDot(p.color))} aria-hidden="true" />
              <span className="truncate">{p.name}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        {!projects.length && <p className="px-2 py-1.5 text-sm text-muted-foreground">You have no projects yet.</p>}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** A jotting's menu: the box's actions on its text, then Edit and Delete. */
function JottingMenu({
  jotting,
  projects,
  notes,
  disabled,
  onNote,
  onCard,
  onChat,
  onEdit,
  onDelete,
}: {
  jotting: Jotting;
  projects: ProjectSummary[];
  notes: boolean;
  disabled: boolean;
  onNote: () => void;
  onCard: (projectId: string) => void;
  onChat: (research: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" className="shrink-0" disabled={disabled} aria-label={`Actions for “${preview(jotting.text)}”`}>
          <MoreHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        {notes && (
          <DropdownMenuItem onSelect={onNote}>
            <Bookmark /> Save as a note
          </DropdownMenuItem>
        )}
        {jotting.projectId ? (
          <DropdownMenuItem onSelect={() => onCard(jotting.projectId!)}>
            <SquareKanban /> Make a card
          </DropdownMenuItem>
        ) : (
          <DropdownMenuSub>
            <DropdownMenuSubTrigger disabled={!projects.length}>
              <SquareKanban /> Make a card in…
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="max-h-72 w-56 overflow-y-auto">
              {projects.map((p) => (
                <DropdownMenuItem key={p.id} onSelect={() => onCard(p.id)}>
                  <span className={cn("size-2 shrink-0 rounded-full", projectDot(p.color))} aria-hidden="true" />
                  <span className="truncate">{p.name}</span>
                </DropdownMenuItem>
              ))}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        )}
        <DropdownMenuItem onSelect={() => onChat(false)}>
          <MessageSquare /> Ask
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onChat(true)}>
          <FlaskConical /> Research this
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onEdit}>
          <Pencil /> Edit
        </DropdownMenuItem>
        <DropdownMenuItem variant="destructive" onSelect={onDelete}>
          <Trash2 /> Delete
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
