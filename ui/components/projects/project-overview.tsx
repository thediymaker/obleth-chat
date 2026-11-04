"use client";

// A project's Overview tab: what is saved in it, your chats, its instructions, the
// collections its chats search and who can see it.

import { useQueryClient } from "@tanstack/react-query";
import { BookOpen, Bookmark, ChevronRight, MessageSquare, Pin, Plus, Upload } from "lucide-react";
import Link from "next/link";
import { useRef, useState } from "react";
import { toast } from "sonner";
import { Eyebrow } from "@/components/app-shell/page";
import { kindLabel, kindOf, refreshSaved, savedPath } from "@/components/library/api";
import { useLibraryDialogs } from "@/components/library/saved-dialogs";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { count, initials, when } from "@/lib/format";
import { keys, useFeature } from "@/lib/queries";
import { useUser } from "@/lib/session";
import { cn } from "@/lib/utils";
import type { SavedSummary } from "./api";
import { ACCEPT, DEFAULT_MAX_FILE, refreshKnowledge, size, uploadDocument, useCollections } from "@/components/knowledge/api";
import { CollectionsPicker, NewProjectCollection } from "./collections-picker";
import { messageOf } from "./form-field";
import { useProjectFrame } from "./project-frame";
import { useRouter } from "next/navigation";

export function Section({ title, aside, children, className }: { title: React.ReactNode; aside?: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <section className={cn("grid gap-3 rounded-lg border bg-card p-4", className)}>
      <div className="flex min-h-6 items-center justify-between gap-2">
        <Eyebrow>{title}</Eyebrow>
        {aside}
      </div>
      {children}
    </section>
  );
}
const Note = ({ children }: { children: React.ReactNode }) => <p className="text-sm text-muted-foreground">{children}</p>;

function SavedCard({ item, onOpen, onPin }: { item: SavedSummary; onOpen: () => void; onPin?: () => void }) {
  const [Icon] = kindOf(item.kind);
  return (
    <article className="flex min-w-0 flex-col gap-2 rounded-md border bg-background p-3">
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Icon className="size-3.5" />
        <span className="grow truncate">{kindLabel(item)}</span>
        {onPin && (
          <Button
            variant="ghost"
            size="icon-xs"
            aria-pressed={item.pinned}
            aria-label={`${item.pinned ? "Unpin" : "Pin"} ${item.title}`}
            title={item.pinned ? "Unpin" : "Pin to the top"}
            onClick={onPin}
            className={cn(item.pinned ? "text-foreground" : "text-faint")}
          >
            <Pin className={cn(item.pinned && "fill-current")} />
          </Button>
        )}
      </div>
      <button type="button" onClick={onOpen} className="line-clamp-2 text-left text-md font-medium hover:underline">
        {item.title}
      </button>
      {item.tags.length > 0 && (
        <div className="flex flex-wrap gap-1.5 text-xs text-faint">
          {item.tags.slice(0, 3).map((t) => (
            <span key={t}>#{t}</span>
          ))}
        </div>
      )}
    </article>
  );
}

export function ProjectOverview() {
  const { detail: d, edit } = useProjectFrame();
  const p = d.project;
  const mine = p.mine;
  const user = useUser();
  const router = useRouter();
  const client = useQueryClient();
  const knowledge = useFeature("knowledge");
  const library = useLibraryDialogs();
  const [picking, setPicking] = useState(false);
  const [making, setMaking] = useState(false);
  // Documents go only into the person's own collections.
  const { data: collections } = useCollections(mine && knowledge);
  const owned = new Set((collections?.collections ?? []).filter((c) => c.mine).map((c) => c.id));
  const maxFileBytes = collections?.maxFileBytes ?? DEFAULT_MAX_FILE;

  async function pin(item: SavedSummary) {
    try {
      await api.request(savedPath(item.id), { method: "PATCH", body: { pinned: !item.pinned } });
      refreshSaved(client);
    } catch (e) {
      toast(messageOf(e));
    }
  }

  const saved = knowledge && (
    <Section
      title="Saved in this project"
      aside={
        <span className="flex items-center gap-1">
          {p.saved > 6 && (
            <Button
              variant="ghost"
              size="xs"
              onClick={() => router.push(`/library?project=${encodeURIComponent(p.id)}`)}
            >
              All {p.saved} <ChevronRight />
            </Button>
          )}
          <Button variant="ghost" size="xs" onClick={() => library.create("note", { projectId: p.id })}>
            <Plus /> Add note
          </Button>
        </span>
      }
    >
      {d.saved.length ? (
        <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
          {d.saved.slice(0, 6).map((item) => (
            <SavedCard key={item.id} item={item} onOpen={() => library.open(item.id)} onPin={item.ownerId === user.id ? () => pin(item) : undefined} />
          ))}
        </div>
      ) : (
        <Note>Things saved from this project’s chats land here, and its chats can find them. Pin the ones that matter most.</Note>
      )}
    </Section>
  );

  const chats = (
    <Section title="Your chats" aside={p.scope === "group" && <span className="text-xs text-faint">Only you see yours</span>}>
      {d.chats.length ? (
        <div className="-mx-2 grid">
          {d.chats.map((c) => (
            <Link key={c.id} href={`/chat/${encodeURIComponent(c.id)}`} className="flex items-center gap-2.5 rounded-md px-2 py-2 text-md hover:bg-accent">
              <MessageSquare className="size-4 shrink-0 text-muted-foreground" />
              <span className="grow truncate">{c.title}</span>
              <span className="shrink-0 text-xs text-faint">{when(c.updatedAt)}</span>
            </Link>
          ))}
        </div>
      ) : (
        <Note>No chats yet. Start one here, or move a chat in from its Context panel.</Note>
      )}
    </Section>
  );

  const instructions = (
    <Section
      title="Instructions for every chat"
      aside={
        mine && (
          <Button variant="ghost" size="xs" onClick={edit}>
            {p.instructions ? "Edit" : "Add instructions"}
          </Button>
        )
      }
    >
      {p.instructions ? (
        <p className="max-h-72 overflow-y-auto text-md whitespace-pre-wrap text-text-2">{p.instructions}</p>
      ) : (
        <Note>{mine ? "Tell the model what this project is about and how to answer. Every chat here gets these instructions." : "No instructions."}</Note>
      )}
    </Section>
  );

  const starts = (
    <Section title="Chats here start with">
      <div className="grid gap-1">
        {d.collections.map((c) => (
          <div key={c.id} className="flex items-center gap-2 text-md">
            <BookOpen className="size-4 shrink-0 text-muted-foreground" />
            <Link href={`/knowledge/${encodeURIComponent(c.id)}`} className="grow truncate hover:underline">
              {c.name}
            </Link>
            <span className="text-xs text-faint">{count(c.documents, "doc")}</span>
            {mine && knowledge && owned.has(c.id) && <AddDocuments collection={c} projectId={p.id} maxFileBytes={maxFileBytes} />}
          </div>
        ))}
        {knowledge && (
          <div className="flex items-center gap-2 text-md">
            <Bookmark className="size-4 text-muted-foreground" />
            <span className="grow">Saved in this project</span>
            <span className="text-xs text-faint">{count(p.saved, "item")}</span>
          </div>
        )}
        {!d.collections.length && !knowledge && <Note>Just the instructions.</Note>}
      </div>
      {mine && knowledge && (
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" onClick={() => setPicking(true)}>
            <BookOpen /> Choose collections
          </Button>
          <Button variant="outline" size="sm" onClick={() => setMaking(true)}>
            <Plus /> New collection
          </Button>
        </div>
      )}
    </Section>
  );

  const people = (
    <Section title="People">
      <div className="flex items-center gap-2.5 text-md">
        <span className="flex size-6 items-center justify-center rounded-full bg-secondary text-2xs font-semibold">{initials(p.ownerName || "?")}</span>
        <span className="grow">{mine ? "You" : p.ownerName}</span>
        <span className="text-xs text-faint">owner</span>
      </div>
      {p.scope === "group" ? (
        <Note>
          Shared through the group{" "}
          <Link className="text-foreground underline-offset-2 hover:underline" href={`/groups/${encodeURIComponent(p.groupId || "")}`}>
            {p.groupName || ""}
          </Link>
          . Members see the instructions, collections and saved items, and can start their own chats here. Only the owner changes the project.
        </Note>
      ) : (
        <Note>Only you can see this project.{mine ? " Share it with a group from Edit." : ""}</Note>
      )}
    </Section>
  );

  return (
    <div className="grid grid-cols-1 items-start gap-4 lg:grid-cols-[minmax(0,1fr)_340px]">
      <div className="grid gap-4">
        {saved}
        {chats}
      </div>
      <div className="grid gap-4">
        {instructions}
        {starts}
        {people}
      </div>
      {library.element}
      {mine && knowledge && <CollectionsPicker open={picking} onOpenChange={setPicking} detail={d} />}
      {mine && knowledge && <NewProjectCollection open={making} onOpenChange={setMaking} detail={d} />}
    </div>
  );
}

/** Uploads files straight into one of the project's collections. */
function AddDocuments({ collection, projectId, maxFileBytes }: { collection: { id: string; name: string }; projectId: string; maxFileBytes: number }) {
  const client = useQueryClient();
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  async function add(files: File[]) {
    setBusy(true);
    let added = 0;
    for (const file of files) {
      if (file.size > maxFileBytes) {
        toast(`${file.name} is larger than ${size(maxFileBytes)}.`);
        continue;
      }
      try {
        await uploadDocument(collection.id, file);
        added++;
      } catch (e) {
        toast(`${file.name}: ${messageOf(e)}`);
      }
    }
    setBusy(false);
    refreshKnowledge(client, collection.id);
    await client.invalidateQueries({ queryKey: keys.project(projectId) });
    if (added) toast(`Added ${count(added, "document")} to ${collection.name}. Chats can search ${added === 1 ? "it" : "them"} once indexed.`);
  }
  return (
    <>
      <Button variant="outline" size="xs" aria-label={`Add documents to ${collection.name}`} disabled={busy} onClick={() => input.current?.click()}>
        <Upload /> {busy ? "Adding…" : "Add"}
      </Button>
      <input
        ref={input}
        type="file"
        multiple
        accept={ACCEPT}
        className="hidden"
        onChange={(e) => {
          const files = Array.from(e.target.files ?? []);
          e.target.value = "";
          if (files.length) add(files);
        }}
      />
    </>
  );
}
