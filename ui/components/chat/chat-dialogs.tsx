"use client";

// Every dialog the chat opens, in one place above the pages, so the sidebar's chat
// menu and the chat page share them.

import { useQuery } from "@tanstack/react-query";
import { ChevronRight, GitBranch, Grid2x2, Link2, LockKeyhole } from "lucide-react";
import Link from "next/link";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { useLibraryDialogs } from "@/components/library/saved-dialogs";
import { Columns, Confirm, Field, FormActions, FormDialog, FormError, Pill, RadioCards, useSubmit } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dropdown } from "@/components/ui/dropdown";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { copyText } from "@/lib/clipboard";
import { api } from "@/lib/api";
import { attachmentUrl, describe } from "@/lib/chat/attachments";
import { COMMENT_LIMIT, feedbackDetails, REASONS } from "@/lib/chat/feedback";
import type { Dialog as ChatDialog } from "@/lib/chat/store";
import type { Attachment, ShareInfo } from "@/lib/chat/types";
import { when } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useChat } from "./chat-provider";
import { NamedIcon } from "./icon";

function Locked({ on }: { on: boolean }) {
  return on ? <Pill>Locked</Pill> : null;
}

function RenameDialog({ dialog, close }: { dialog: Extract<ChatDialog, { kind: "rename" }>; close: () => void }) {
  const store = useChat();
  const [title, setTitle] = useState(dialog.title);
  const form = useSubmit(async () => {
    if (title.trim()) await store.rename(dialog.id, title.trim());
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title="Rename conversation">
      <form className="grid gap-4" onSubmit={form.onSubmit}>
        <Field id="chat-title" label="Conversation title">
          <Input id="chat-title" maxLength={80} value={title} required autoFocus onFocus={(e) => e.currentTarget.select()} onChange={(e) => setTitle(e.target.value)} />
        </Field>
        <FormError message={form.error} />
        <FormActions>
          <Button type="submit" disabled={form.busy}>
            Save title
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function DeleteDialog({ dialog, close }: { dialog: Extract<ChatDialog, { kind: "delete" }>; close: () => void }) {
  const store = useChat();
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && close()}
      title="Delete this conversation?"
      text="This permanently deletes this conversation from your account."
      cancel="Keep conversation"
      label="Delete conversation"
      action={() => store.remove(dialog.id)}
    />
  );
}

function EditDialog({ dialog, close }: { dialog: Extract<ChatDialog, { kind: "edit" }>; close: () => void }) {
  const store = useChat();
  const [text, setText] = useState(dialog.content);
  const form = useSubmit(() => store.forkAndSend(dialog.messageId, "edit", text));
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title="Edit and resend" description="Creates a new version in this conversation. Previous versions remain available." className="sm:max-w-xl">
      <form className="grid gap-4" onSubmit={form.onSubmit}>
        <Field id="edited-message" label="Message">
          <Textarea id="edited-message" rows={7} maxLength={12000} required={dialog.required} value={text} onChange={(e) => setText(e.target.value)} />
        </Field>
        <FormError message={form.error} />
        <FormActions>
          <Button type="submit" disabled={form.busy}>
            Send edited message
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function FeedbackDialog({ dialog, close }: { dialog: Extract<ChatDialog, { kind: "feedback" }>; close: () => void }) {
  const store = useChat();
  const m = store.active?.messages?.[dialog.index];
  const [reasons, setReasons] = useState<string[]>(m?.feedback?.reasons || []);
  const [comment, setComment] = useState(m?.feedback?.comment || "");
  const form = useSubmit(async () => {
    await store.saveRating(dialog.index, feedbackDetails(reasons, comment));
    close();
    toast("Thanks for the feedback");
  });
  return (
    <FormDialog
      open
      onOpenChange={(o) => !o && close()}
      title="What went wrong?"
      description="Thanks. If you like, say what went wrong. Only you and administrators see this, and administrators don’t see the conversation or who sent it."
    >
      <form className="grid gap-4" onSubmit={form.onSubmit}>
        <fieldset>
          <legend className="mb-1.5 text-sm font-medium">What was wrong?</legend>
          <div className="flex flex-wrap gap-1.5">
            {REASONS.map(([key, label]) => (
              <label key={key} className={cn("inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-1 text-sm", reasons.includes(key) && "border-foreground bg-secondary")}>
                <Checkbox
                  name="reason"
                  value={key}
                  checked={reasons.includes(key)}
                  onCheckedChange={(on) => setReasons((list) => (on ? [...list, key] : list.filter((r) => r !== key)))}
                />
                {label}
              </label>
            ))}
          </div>
        </fieldset>
        <Field
          id="feedback-comment"
          label={
            <>
              Comment <small className="font-normal text-faint">(optional)</small>
            </>
          }
        >
          <Textarea id="feedback-comment" rows={3} maxLength={COMMENT_LIMIT} placeholder="What would have been better?" value={comment} onChange={(e) => setComment(e.target.value)} />
        </Field>
        <FormError message={form.error} />
        <FormActions cancel="Skip">
          <Button type="submit" disabled={form.busy}>
            Send feedback
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function ParametersDialog({ close }: { close: () => void }) {
  const store = useChat();
  const [systemPrompt, setSystemPrompt] = useState(store.systemPrompt);
  const [temperature, setTemperature] = useState(String(store.temperature));
  const [topP, setTopP] = useState(String(store.topP));
  const [maxTokens, setMaxTokens] = useState(store.maxTokens ? String(store.maxTokens) : "");
  const form = useSubmit(() => store.saveParameters({ systemPrompt, temperature: Number(temperature), topP: Number(topP), maxTokens: Number(maxTokens) }));
  const locked = (key: string) => store.isLocked(key);
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title="Generation parameters" description="Applies to your next message. Locked controls are managed by your administrator." className="sm:max-w-lg">
      {store.presets.length > 0 && (
        <Button variant="outline" size="sm" className="justify-self-start" onClick={() => store.openDialog({ kind: "presets" })}>
          <Grid2x2 /> Use a saved preset
        </Button>
      )}
      <form className="grid gap-4" onSubmit={form.onSubmit}>
        <Field
          id="system-prompt"
          label={
            <>
              System instructions <Locked on={locked("systemPrompt")} />
            </>
          }
        >
          <Textarea id="system-prompt" rows={4} maxLength={8192} disabled={locked("systemPrompt")} value={systemPrompt} onChange={(e) => setSystemPrompt(e.target.value)} />
        </Field>
        <Columns>
          <Field
            id="temperature"
            label={
              <>
                Temperature <Locked on={locked("temperature")} />
              </>
            }
          >
            <Input id="temperature" type="number" min={0} max={2} step={0.1} required disabled={locked("temperature")} value={temperature} onChange={(e) => setTemperature(e.target.value)} />
          </Field>
          <Field
            id="top-p"
            label={
              <>
                Top P <Locked on={locked("topP")} />
              </>
            }
          >
            <Input id="top-p" type="number" min={0.01} max={1} step={0.01} required disabled={locked("topP")} value={topP} onChange={(e) => setTopP(e.target.value)} />
          </Field>
        </Columns>
        <Field
          id="max-tokens"
          label={
            <>
              Maximum output tokens <Locked on={locked("maxTokens")} />
            </>
          }
          help="Leave empty to let the model write as much as it needs."
        >
          <Input id="max-tokens" type="number" min={0} max={262144} step={1} placeholder="The model’s own limit" disabled={locked("maxTokens")} value={maxTokens} onChange={(e) => setMaxTokens(e.target.value)} aria-describedby="max-tokens-help" />
        </Field>
        <FormError message={form.error} />
        <FormActions>
          <Button type="submit" disabled={form.busy}>
            Save parameters
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function PresetsDialog({ close }: { close: () => void }) {
  const store = useChat();
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title="Saved presets">
      <div className="grid gap-1">
        {store.presets.map((p) => (
          <button key={p.id} type="button" className="flex items-center gap-3 rounded-md border px-3 py-2 text-left hover:bg-accent" onClick={() => store.applyPreset(p)}>
            <Grid2x2 className="size-4 text-muted-foreground" />
            <span className="min-w-0 flex-1">
              <h3 className="truncate text-md font-medium">{p.name}</h3>
              <p className="truncate text-sm text-muted-foreground">{store.modelName(p.model)}</p>
            </span>
            <ChevronRight className="size-4 text-muted-foreground" />
          </button>
        ))}
      </div>
    </FormDialog>
  );
}

function AlternativesDialog({ dialog, close }: { dialog: Extract<ChatDialog, { kind: "alternatives" }>; close: () => void }) {
  const store = useChat();
  const current = store.active?.id;
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title="Conversation alternatives">
      <div className="grid max-h-[60vh] gap-1 overflow-y-auto">
        {dialog.conversations.length ? (
          dialog.conversations.map((c) => (
            <Link
              key={c.id}
              href={`/chat/${encodeURIComponent(c.id)}`}
              onClick={close}
              aria-current={c.id === current ? "page" : undefined}
              className={cn("flex items-center gap-3 rounded-md border px-3 py-2 hover:bg-accent", c.id === current && "border-foreground bg-secondary")}
            >
              <GitBranch className="size-4 text-muted-foreground" />
              <span className="min-w-0 flex-1">
                <h3 className="truncate text-md font-medium">{c.title}</h3>
                <p className="truncate text-sm text-muted-foreground">
                  {store.modelName(c.model)}
                  {c.id === current ? " · Current conversation" : ""}
                </p>
              </span>
              <ChevronRight className="size-4 text-muted-foreground" />
            </Link>
          ))
        ) : (
          <p className="text-sm text-muted-foreground">No separate branches yet. Use Branch conversation on a response to create one.</p>
        )}
      </div>
    </FormDialog>
  );
}

export const SHORTCUTS: [string, string][] = [
  ["Search conversations", "⌘ / Ctrl K"],
  ["New conversation", "⌘ / Ctrl Shift O"],
  ["Chat details", "⌘ / Ctrl ."],
  ["Send a message", "Enter"],
  ["New line", "Shift Enter"],
];
function ShortcutsDialog({ close }: { close: () => void }) {
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title="Keyboard shortcuts">
      <div className="grid divide-y rounded-md border">
        {SHORTCUTS.map(([label, keys]) => (
          <div key={label} className="flex justify-between px-3 py-2 text-md">
            <span>{label}</span>
            <kbd className="font-mono text-sm text-muted-foreground">{keys}</kbd>
          </div>
        ))}
      </div>
    </FormDialog>
  );
}

function TextDialog({ title, text, close }: { title: string; text: string; close: () => void }) {
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title={<span className="block truncate">{title}</span>} wide>
      <pre className="max-h-[60vh] overflow-auto rounded-md bg-code px-3 py-2 font-mono text-xs whitespace-pre-wrap">{text}</pre>
      <FormActions cancel={false}>
        <Button onClick={close}>Close</Button>
      </FormActions>
    </FormDialog>
  );
}

const CHOICES: [string, string, string][] = [
  ["none", "Only you", "Nobody else can open it."],
  ["group", "A group", "Members find it on the group’s page."],
  ["signed_in", "Anyone signed in with the link", "Every account here can open it from the link."],
];
export const shareLink = (id: string) => `${typeof location === "undefined" ? "" : location.origin}/chat/${encodeURIComponent(id)}`;

/**
 * Sharing a chat: read-only and live, with one group or with anyone
 * signed in who has the link. The link is the chat's own address; people it is not
 * shared with get a not-found page. A change is saved as soon as it is made.
 */
export function ShareDialog({ id, close }: { id: string; close: () => void }) {
  const store = useChat();
  const loaded = useQuery({ queryKey: ["chat", "share", id], queryFn: () => api.request<{ share: ShareInfo; groups?: { id: string; name: string }[] }>(`/api/conversations/${encodeURIComponent(id)}/share`) });
  const [share, setShare] = useState<ShareInfo | null>(null);
  useEffect(() => {
    if (loaded.data) setShare(loaded.data.share);
  }, [loaded.data]);
  useEffect(() => {
    if (loaded.error) {
      toast((loaded.error as Error).message);
      close();
    }
  }, [loaded.error, close]);
  if (!loaded.data || !share) return null;
  const groups = loaded.data.groups || [];
  const scope = share.scope || "none";
  // The group it is shared with stays a choice even after its owner leaves that group.
  const options = share.groupId && !groups.some((g) => g.id === share.groupId) ? [...groups, { id: share.groupId, name: share.groupName || "Current group" }] : groups;
  async function save(next: string, groupId?: string) {
    // Picking "A group" first shares with the group it had, or the first one offered.
    const body = { scope: next, ...(next === "group" ? { groupId: groupId || share!.groupId || options[0]?.id } : {}) };
    try {
      const r = await api.request<{ share: ShareInfo }>(`/api/conversations/${encodeURIComponent(id)}/share`, { method: "PUT", body });
      setShare(r.share);
      toast(r.share.scope === "none" ? "Only you can open it now" : r.share.scope === "group" ? `Shared with ${r.share.groupName}` : "Anyone signed in with the link can open it");
      store.shareChanged(id, r.share.scope);
    } catch (e) {
      toast((e as Error).message);
    }
  }
  return (
    <FormDialog open onOpenChange={(o) => !o && close()} title="Share this chat" className="sm:max-w-lg">
      <div className="grid gap-2">
        <RadioCards
          label="Who can open it"
          name="share-scope"
          value={scope}
          onChange={(v) => save(v)}
          options={CHOICES.map(([value, label, note]) => {
            const off = value === "group" && !options.length;
            return [value, label, off ? "You are not in any groups yet." : note, off] as const;
          })}
        />
        {options.length > 0 && scope === "group" && (
          <Dropdown aria-label="Group" value={share.groupId ?? undefined} onChange={(e) => save("group", e.target.value)}>
            {options.map((g) => (
              <option key={g.id} value={g.id}>
                {g.name}
              </option>
            ))}
          </Dropdown>
        )}
      </div>
      {scope !== "none" && (
        <div className="flex gap-2">
          <Input readOnly value={shareLink(id)} aria-label="Link to this chat" onFocus={(e) => e.currentTarget.select()} />
          <Button
            size="sm"
            className="h-9"
            onClick={async () => {
              await copyText(shareLink(id), "Link copied");
            }}
          >
            <Link2 /> Copy link
          </Button>
        </div>
      )}
      <p className="flex items-start gap-2 text-sm text-muted-foreground">
        <LockKeyhole className="mt-0.5 size-4 shrink-0" />
        People see the chat as it grows, read-only, and can fork it into their own copy. They see its messages, attachments and images, and the steps tools took with what they returned, but not your instructions for the chat. They can’t use your workspace, browser or connections.
      </p>
      <FormActions cancel={false}>
        <Button onClick={close}>Done</Button>
      </FormActions>
    </FormDialog>
  );
}

/**
 * The person's uploaded files: attach one to the
 * next message, or delete one a saved conversation does not use. Settings opens it too.
 */
export function YourFilesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const store = useChat();
  const files = useQuery({ queryKey: ["chat", "attachments"], queryFn: () => api.request<{ attachments?: Attachment[]; limits?: { maxFileBytes?: number } }>("/api/attachments"), enabled: open });
  const [deleting, setDeleting] = useState<string | null>(null);
  useEffect(() => {
    if (files.data?.limits) store.attachments.setLimits(files.data.limits);
  }, [files.data, store]);
  const list = files.data?.attachments || [];
  const selected = store.attachments.current();
  return (
    <>
      <FormDialog
        open={open}
        onOpenChange={onOpenChange}
        title="Your files"
        description="Files are private to your account. Attach any file to a conversation: Chat reads text, code and documents (PDF, Word, PowerPoint, Excel), shows images to models that can see them, and puts every file in the conversation’s workspace when it has one."
        className="sm:max-w-lg"
      >
        {files.isPending ? (
          <Loading compact label="Loading files…" />
        ) : files.error ? (
          <ErrorBox compact error={files.error} />
        ) : list.length ? (
          <div className="grid max-h-[50vh] divide-y overflow-y-auto rounded-md border">
            {list.map((a) => {
              const d = describe(a);
              return (
                <div key={a.id} className="flex flex-wrap items-center gap-3 px-3 py-2">
                  <NamedIcon name={d.glyph} className="size-4 text-muted-foreground" />
                  <a href={attachmentUrl(a)} target="_blank" rel="noopener noreferrer" className="min-w-0 flex-1">
                    <h3 className="truncate text-md font-medium">{a.name}</h3>
                    <p className="text-sm text-muted-foreground">
                      {d.detail}
                      {a.createdAt ? ` · ${when(a.createdAt)}` : ""}
                    </p>
                  </a>
                  <Button
                    size="xs"
                    variant="outline"
                    disabled={selected.length >= 4 || selected.some((s) => s.id === a.id) || store.streaming}
                    onClick={() => {
                      if (store.attachments.attachExisting(a)) {
                        onOpenChange(false);
                        toast("File attached to your next message");
                      }
                    }}
                  >
                    Attach
                  </Button>
                  <Button size="xs" variant="ghost" onClick={() => setDeleting(a.id)}>
                    Delete
                  </Button>
                </div>
              );
            })}
          </div>
        ) : (
          <EmptyState title="No uploaded files" text="Use the paperclip in Chat to attach a file." />
        )}
        <FormActions cancel={false}>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </FormActions>
      </FormDialog>
      <Confirm
        open={!!deleting}
        onOpenChange={(o) => !o && setDeleting(null)}
        title="Delete this file?"
        text="Files referenced by saved conversations cannot be deleted."
        cancel="Keep file"
        label="Delete file"
        action={async () => {
          const id = deleting!;
          await api.request(`/api/attachments/${encodeURIComponent(id)}`, { method: "DELETE" });
          store.attachments.remove(id);
          await files.refetch();
          toast("File deleted");
        }}
      />
    </>
  );
}

export function ChatDialogs() {
  const store = useChat();
  const library = useLibraryDialogs();
  const dialog = store.dialog;
  const close = () => store.openDialog(null);
  // Saving from a chat opens the Library's own dialog.
  useEffect(() => {
    if (dialog?.kind !== "save") return;
    const { item } = dialog;
    store.openDialog(null);
    library.saveFrom({ ...item, kind: item.kind as never });
  }, [dialog, store, library]);
  return (
    <>
      {library.element}
      {dialog?.kind === "rename" && <RenameDialog key={dialog.id} dialog={dialog} close={close} />}
      {dialog?.kind === "delete" && <DeleteDialog dialog={dialog} close={close} />}
      {dialog?.kind === "share" && <ShareDialog id={dialog.id} close={close} />}
      {dialog?.kind === "edit" && <EditDialog dialog={dialog} close={close} />}
      {dialog?.kind === "feedback" && <FeedbackDialog dialog={dialog} close={close} />}
      {dialog?.kind === "parameters" && <ParametersDialog close={close} />}
      {dialog?.kind === "presets" && <PresetsDialog close={close} />}
      {dialog?.kind === "alternatives" && <AlternativesDialog dialog={dialog} close={close} />}
      {dialog?.kind === "shortcuts" && <ShortcutsDialog close={close} />}
      <YourFilesDialog open={dialog?.kind === "files"} onOpenChange={(o) => !o && close()} />
      {dialog?.kind === "pasted" && <TextDialog title={dialog.name} text={store.attachments.pastedText(dialog.id) || ""} close={close} />}
    </>
  );
}
