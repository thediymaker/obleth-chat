"use client";

// Memory: what Chat has learned about you, as a graph of topics and memories, with
// a panel to read, change, pin and delete each one. Endpoints in api/src/memory.rs.

import { useQueryClient } from "@tanstack/react-query";
import { Brain, List, MoreHorizontal, Network, Pin, PinOff, Plus, Search, Sparkles, Trash2, X } from "lucide-react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { FormActions, FormDialog, FormError, Note, Pill, Section, SelectField, TextAreaField, TextField, useConfirm, useSubmit } from "@/components/manage/kit";
import { messageOf } from "@/components/projects/form-field";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { initials } from "@/lib/format";
import { useUser } from "@/lib/session";
import { cn } from "@/lib/utils";
import {
  addMemory,
  ago,
  changeMemory,
  deleteMemory,
  MEMORY_KEY,
  MEMORY_KINDS,
  refreshMemory,
  saveChoices,
  topicColor,
  topicNames,
  topicPath,
  TOPIC_KINDS,
  useMemory,
  type Memory,
  type MemoryKind,
  type MemoryState,
  type Topic,
  type TopicKind,
} from "./api";
import { MemoryGraph, type Selection } from "./memory-graph";

const KIND_OPTIONS = Object.entries(MEMORY_KINDS).map(([k, [label]]) => [k, label] as const);
const TOPIC_OPTIONS = Object.entries(TOPIC_KINDS).map(([k, [label]]) => [k, label] as const);
const chatHref = (id: string) => `/chat/${encodeURIComponent(id)}`;

export function MemoryPage() {
  usePageChrome("Memory");
  const user = useUser();
  const client = useQueryClient();
  const router = useRouter();
  const params = useSearchParams();
  const memory = useMemory();
  const confirm = useConfirm();
  const [view, setView] = useState<"graph" | "list">("graph");
  const [query, setQuery] = useState("");
  const [adding, setAdding] = useState(false);
  const linked = params.get("m");
  const [selected, setSelected] = useState<Selection>(linked ? { type: "memory", id: linked } : null);
  const data = memory.data;
  const name = user.display_name || user.username;

  const select = (s: Selection) => {
    setSelected(s);
    if (linked) router.replace("/memory", { scroll: false });
  };
  async function choose(next: { useMemory: boolean; learn: boolean }) {
    client.setQueryData<MemoryState>(MEMORY_KEY, (old) => (old ? { ...old, ...next } : old));
    try {
      await saveChoices(next);
    } catch (e) {
      toast(messageOf(e));
      refreshMemory(client);
    }
  }
  function forgetAll() {
    confirm.ask({
      title: "Delete all memories?",
      text: `Chat forgets all ${data?.memories.length ?? 0} memories and their topics. Your chats are not changed. This can't be undone.`,
      label: "Delete all",
      action: async () => {
        await api.request("/api/memory", { method: "DELETE" });
        setSelected(null);
        await refreshMemory(client);
        toast("All memories deleted");
      },
    });
  }

  if (memory.isPending) return <Page><Loading label="Loading memory…" /></Page>;
  if (memory.error || !data)
    return (
      <Page>
        <ErrorBox error={memory.error} onRetry={() => memory.refetch()} />
      </Page>
    );
  const paused = data.available && !data.useMemory && !data.learn;
  return (
    <Page>
      {confirm.element}
      <PageHeader
        title="Memory"
        description="What Chat has learned about you and your work, so answers fit you. Only you can see it."
        actions={
          <>
            {data.available && (
              <Button onClick={() => setAdding(true)}>
                <Plus /> Add memory
              </Button>
            )}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="icon" aria-label="More memory actions">
                  <MoreHorizontal />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem variant="destructive" disabled={!data.memories.length} onSelect={forgetAll}>
                  <Trash2 /> Delete all memories
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />
      {!data.available ? (
        <section className="mb-4 flex flex-col items-center gap-3 rounded-lg border bg-card px-6 py-12 text-center">
          <Brain className="size-6 text-muted-foreground" />
          <h2 className="text-lg font-semibold">Memory is not turned on for this server.</h2>
          <p className="max-w-md text-sm text-muted-foreground">
            An administrator can turn it on in Administration › System. {data.memories.length ? "Your memories from before are below; you can still delete them." : ""}
          </p>
        </section>
      ) : (
        <div className="mb-4 grid gap-3 rounded-lg border bg-card p-4 sm:grid-cols-2">
          <Choice
            id="memory-use"
            label="Use memory in chats"
            text="Chats get the memories that fit what you ask, and the ones you pin."
            checked={data.useMemory}
            onChange={(v) => choose({ useMemory: v, learn: data.learn })}
          />
          <Choice
            id="memory-learn"
            label="Learn from my chats"
            text="After a reply, Chat notes lasting things about you and your work. Temporary chats are never read."
            checked={data.learn}
            onChange={(v) => choose({ useMemory: data.useMemory, learn: v })}
          />
          {paused && <p className="text-sm text-amber sm:col-span-2">Memory is paused. Chat neither uses nor learns memories until you turn one of these on.</p>}
        </div>
      )}
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <div className="relative min-w-52 flex-1 sm:max-w-80">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Find a memory or topic" aria-label="Find a memory or topic" className="pl-8" />
        </div>
        <div role="group" aria-label="View" className="flex rounded-md border p-0.5">
          {(
            [
              ["graph", "Graph", Network],
              ["list", "List", List],
            ] as const
          ).map(([id, label, Icon]) => (
            <button
              key={id}
              type="button"
              aria-pressed={view === id}
              onClick={() => setView(id)}
              className={cn("flex items-center gap-1.5 rounded-sm px-2.5 py-1 text-sm text-muted-foreground", view === id && "bg-accent text-foreground")}
            >
              <Icon className="size-3.5" /> {label}
            </button>
          ))}
        </div>
        <span className="ml-auto text-sm text-muted-foreground">
          {data.memories.length} {data.memories.length === 1 ? "memory" : "memories"} · {data.topics.length} {data.topics.length === 1 ? "topic" : "topics"}
          {data.memories.length >= data.max && <span className="text-amber"> · full</span>}
        </span>
      </div>
      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_360px]">
        <div className="min-w-0">
          {!data.memories.length ? (
            <EmptyState
              icon={Sparkles}
              title="Nothing remembered yet"
              text={
                data.available
                  ? "As you chat, Chat will remember your role, your projects, the tools you use and how you like answers, and draw them here. You can also add a memory yourself."
                  : "Chat has no memories of you."
              }
              action={
                data.available && (
                  <Button variant="outline" onClick={() => setAdding(true)}>
                    <Plus /> Add memory
                  </Button>
                )
              }
            />
          ) : view === "graph" ? (
            <MemoryGraph className="h-[min(640px,70vh)]" memories={data.memories} topics={data.topics} selected={selected} onSelect={select} query={query} initials={initials(name) || "You"} />
          ) : (
            <MemoryList data={data} query={query} selected={selected} onSelect={select} />
          )}
        </div>
        <aside className="grid min-w-0 gap-4 lg:sticky lg:top-4">
          {selected?.type === "memory" && data.memories.some((m) => m.id === selected.id) ? (
            <MemoryDetail key={selected.id} memory={data.memories.find((m) => m.id === selected.id)!} data={data} onClose={() => select(null)} onSelect={select} />
          ) : selected?.type === "topic" && data.topics.some((t) => t.id === selected.id) ? (
            <TopicDetail key={selected.id} topic={data.topics.find((t) => t.id === selected.id)!} data={data} onClose={() => select(null)} onSelect={select} confirm={confirm.ask} />
          ) : (
            <Recent data={data} onSelect={select} />
          )}
        </aside>
      </div>
      {adding && <AddMemory data={data} onClose={() => setAdding(false)} onAdded={(id) => select({ type: "memory", id })} />}
    </Page>
  );
}

function Choice({ id, label, text, checked, onChange }: { id: string; label: string; text: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-start gap-3">
      <Switch id={id} checked={checked} onCheckedChange={onChange} className="mt-0.5" aria-describedby={`${id}-help`} />
      <div className="grid gap-0.5">
        <label htmlFor={id} className="text-md font-medium">
          {label}
        </label>
        <p id={`${id}-help`} className="text-sm text-muted-foreground">
          {text}
        </p>
      </div>
    </div>
  );
}

function TopicChip({ topic, onClick }: { topic: Topic; onClick?: () => void }) {
  const color = topicColor(topic.kind);
  const body = (
    <>
      <span className="size-2 rounded-full" style={{ background: color }} />
      {topic.name}
    </>
  );
  const cls = "inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs";
  return onClick ? (
    <button type="button" className={cn(cls, "hover:bg-accent")} onClick={onClick}>
      {body}
    </button>
  ) : (
    <span className={cls}>{body}</span>
  );
}

/** Where a memory came from, in a few words. */
function Origin({ memory }: { memory: Memory }) {
  if (memory.origin === "added") return <>Added by you {ago(memory.createdAt)}</>;
  if (!memory.source) return <>Learned {ago(memory.createdAt)} from a chat that was deleted</>;
  return (
    <>
      Learned {ago(memory.createdAt)} from{" "}
      <Link className="underline-offset-2 hover:underline" href={chatHref(memory.source.conversationId)}>
        “{memory.source.title || "a chat"}”
      </Link>
    </>
  );
}

function Recent({ data, onSelect }: { data: MemoryState; onSelect: (s: Selection) => void }) {
  const recent = useMemo(() => [...data.memories].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)).slice(0, 8), [data.memories]);
  const used = useMemo(
    () =>
      data.memories
        .filter((m) => m.uses > 0)
        .sort((a, b) => b.uses - a.uses)
        .slice(0, 5),
    [data.memories],
  );
  return (
    <>
      <Section title="Recently learned" description="Select a memory or topic in the graph to read it, change it or delete it.">
        {!recent.length && <Note>Nothing yet.</Note>}
        <ul className="grid gap-1">
          {recent.map((m) => (
            <li key={m.id}>
              <button type="button" onClick={() => onSelect({ type: "memory", id: m.id })} className="grid w-full gap-0.5 rounded-md px-2 py-1.5 text-left hover:bg-accent">
                <span className="text-md">{m.content}</span>
                <span className="text-xs text-muted-foreground">
                  {m.origin === "added" ? "Added" : "Learned"} {ago(m.createdAt)}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </Section>
      {!!used.length && (
        <Section title="Used most">
          <ul className="grid gap-1">
            {used.map((m) => (
              <li key={m.id}>
                <button type="button" onClick={() => onSelect({ type: "memory", id: m.id })} className="flex w-full items-baseline gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent">
                  <span className="grow text-md">{m.content}</span>
                  <span className="shrink-0 text-xs text-muted-foreground">{m.uses}×</span>
                </button>
              </li>
            ))}
          </ul>
        </Section>
      )}
    </>
  );
}

function MemoryDetail({ memory, data, onClose, onSelect }: { memory: Memory; data: MemoryState; onClose: () => void; onSelect: (s: Selection) => void }) {
  const client = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(memory.content);
  const [topicsText, setTopicsText] = useState("");
  const [busy, setBusy] = useState(false);
  const topics = memory.topics.map((id) => data.topics.find((t) => t.id === id)).filter((t): t is Topic => !!t);
  async function save(body: Parameters<typeof changeMemory>[1], done?: string) {
    setBusy(true);
    try {
      await changeMemory(memory.id, body);
      await refreshMemory(client);
      if (done) toast(done);
      return true;
    } catch (e) {
      toast(messageOf(e));
      return false;
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    setBusy(true);
    try {
      await deleteMemory(memory.id);
      onClose();
      await refreshMemory(client);
      toast("Memory deleted", {
        action: {
          label: "Undo",
          onClick: async () => {
            const { memory: back } = await addMemory({ content: memory.content, kind: memory.kind, pinned: memory.pinned, topics: topics.map((t) => t.name) });
            await refreshMemory(client);
            onSelect({ type: "memory", id: back.id });
          },
        },
      });
    } catch (e) {
      toast(messageOf(e));
      setBusy(false);
    }
  }
  return (
    <Section
      title={MEMORY_KINDS[memory.kind]?.[0] ?? "Memory"}
      actions={
        <Button variant="ghost" size="icon-xs" aria-label="Close" onClick={onClose}>
          <X />
        </Button>
      }
    >
      {editing ? (
        <form
          className="grid gap-2"
          onSubmit={async (e) => {
            e.preventDefault();
            if (await save({ content: text }, "Memory saved")) setEditing(false);
          }}
        >
          <Textarea value={text} onChange={(e) => setText(e.target.value)} maxLength={500} rows={3} aria-label="Memory" autoFocus />
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => {
                setText(memory.content);
                setEditing(false);
              }}
            >
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy || !text.trim()}>
              Save
            </Button>
          </div>
        </form>
      ) : (
        <button type="button" onClick={() => setEditing(true)} className="rounded-md text-left text-lg leading-snug hover:bg-accent/60" title="Edit">
          {memory.content}
        </button>
      )}
      <div className="flex flex-wrap gap-1.5">
        {topics.map((t) => (
          <TopicChip key={t.id} topic={t} onClick={() => onSelect({ type: "topic", id: t.id })} />
        ))}
        {!topics.length && <span className="text-xs text-muted-foreground">No topic yet</span>}
      </div>
      <form
        className="flex gap-2"
        onSubmit={async (e) => {
          e.preventDefault();
          const names = topicNames(topicsText);
          if (!names.length) return;
          const all = [...topics.map((t) => t.name), ...names].filter((n, i, a) => a.findIndex((x) => x.toLowerCase() === n.toLowerCase()) === i).slice(0, 3);
          if (await save({ topics: all })) setTopicsText("");
        }}
      >
        <Input value={topicsText} onChange={(e) => setTopicsText(e.target.value)} placeholder={topics.length >= 3 ? "Three topics at most" : "Add a topic"} disabled={topics.length >= 3} aria-label="Add a topic" list="memory-topic-names" className="h-8" />
        <datalist id="memory-topic-names">
          {data.topics.map((t) => (
            <option key={t.id} value={t.name} />
          ))}
        </datalist>
        {topics.length > 0 && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button type="button" variant="outline" size="sm" className="h-8">
                Remove
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {topics.map((t) => (
                <DropdownMenuItem key={t.id} onSelect={() => save({ topics: topics.filter((x) => x.id !== t.id).map((x) => x.name) })}>
                  {t.name}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </form>
      <SelectField id="memory-kind" label="Kind" value={memory.kind} onChange={(e) => save({ kind: e.target.value as MemoryKind })} options={KIND_OPTIONS} help={MEMORY_KINDS[memory.kind]?.[1]} />
      <div className="grid gap-1 text-sm text-muted-foreground">
        <span>
          <Origin memory={memory} />
        </span>
        <span>{memory.uses ? `Given to ${memory.uses} ${memory.uses === 1 ? "reply" : "replies"}, last ${ago(memory.usedAt)}` : "Not used in a reply yet"}</span>
        {memory.updatedAt !== memory.createdAt && <span>Changed {ago(memory.updatedAt)}</span>}
      </div>
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" size="sm" disabled={busy} onClick={() => save({ pinned: !memory.pinned }, memory.pinned ? "Unpinned" : "Pinned: every chat gets it")}>
          {memory.pinned ? <PinOff /> : <Pin />} {memory.pinned ? "Unpin" : "Pin"}
        </Button>
        <Button variant="outline" size="sm" disabled={busy} onClick={remove} className="text-destructive-foreground">
          <Trash2 /> Delete
        </Button>
      </div>
      {memory.pinned && <Note>Pinned: every chat gets this memory, and Chat never changes it on its own.</Note>}
    </Section>
  );
}

function TopicDetail({
  topic,
  data,
  onClose,
  onSelect,
  confirm,
}: {
  topic: Topic;
  data: MemoryState;
  onClose: () => void;
  onSelect: (s: Selection) => void;
  confirm: ReturnType<typeof useConfirm>["ask"];
}) {
  const client = useQueryClient();
  const [name, setName] = useState(topic.name);
  const memories = data.memories.filter((m) => m.topics.includes(topic.id));
  async function patch(body: { name?: string; kind?: TopicKind }) {
    try {
      const { id } = await api.request<{ id: string }>(topicPath(topic.id), { method: "PATCH", body });
      await refreshMemory(client);
      if (id !== topic.id) {
        toast("Merged into the topic of that name");
        onSelect({ type: "topic", id });
      }
    } catch (e) {
      toast(messageOf(e));
    }
  }
  const remove = (withMemories: boolean) =>
    confirm({
      title: withMemories ? `Forget everything about ${topic.name}?` : `Remove the topic ${topic.name}?`,
      text: withMemories
        ? `Chat deletes the ${memories.length} ${memories.length === 1 ? "memory" : "memories"} about ${topic.name}. This can't be undone.`
        : "The memories stay; only the topic goes.",
      label: withMemories ? "Forget" : "Remove topic",
      action: async () => {
        await api.request(`${topicPath(topic.id)}${withMemories ? "?memories=true" : ""}`, { method: "DELETE" });
        onClose();
        await refreshMemory(client);
      },
    });
  return (
    <Section
      title={
        <span className="flex items-center gap-2">
          <span className="size-2.5 rounded-full" style={{ background: topicColor(topic.kind) }} />
          {TOPIC_KINDS[topic.kind]?.[0] ?? "Topic"}
        </span>
      }
      actions={
        <Button variant="ghost" size="icon-xs" aria-label="Close" onClick={onClose}>
          <X />
        </Button>
      }
    >
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim() && name.trim() !== topic.name) patch({ name });
        }}
      >
        <Input value={name} onChange={(e) => setName(e.target.value)} maxLength={60} aria-label="Topic name" className="text-base font-semibold" />
        {name.trim() !== topic.name && (
          <Button type="submit" size="sm">
            Rename
          </Button>
        )}
      </form>
      <SelectField id="topic-kind" label="Kind" value={topic.kind} onChange={(e) => patch({ kind: e.target.value as TopicKind })} options={TOPIC_OPTIONS} />
      <div className="grid gap-1">
        <span className="text-sm font-medium">
          {memories.length} {memories.length === 1 ? "memory" : "memories"}
        </span>
        <ul className="grid gap-0.5">
          {memories.map((m) => (
            <li key={m.id}>
              <button type="button" onClick={() => onSelect({ type: "memory", id: m.id })} className="w-full rounded-md px-2 py-1.5 text-left text-md hover:bg-accent">
                {m.pinned && <Pin className="mr-1 inline size-3 text-muted-foreground" />}
                {m.content}
              </button>
            </li>
          ))}
        </ul>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" size="sm" onClick={() => remove(false)}>
          Remove topic
        </Button>
        <Button variant="outline" size="sm" className="text-destructive-foreground" onClick={() => remove(true)}>
          <Trash2 /> Forget all
        </Button>
      </div>
    </Section>
  );
}

function MemoryList({ data, query, selected, onSelect }: { data: MemoryState; query: string; selected: Selection; onSelect: (s: Selection) => void }) {
  const q = query.trim().toLowerCase();
  const groups = useMemo(() => {
    const shown = (m: Memory) => !q || m.content.toLowerCase().includes(q) || m.topics.some((id) => data.topics.find((t) => t.id === id)?.name.toLowerCase().includes(q));
    const out: { topic: Topic | null; memories: Memory[] }[] = data.topics.map((t) => ({ topic: t, memories: data.memories.filter((m) => m.topics[0] === t.id && shown(m)) }));
    out.push({ topic: null, memories: data.memories.filter((m) => !m.topics.length && shown(m)) });
    return out.filter((g) => g.memories.length);
  }, [data, q]);
  if (!groups.length) return <EmptyState title="No memory matches" text="Try other words." />;
  return (
    <div className="grid gap-3" aria-label="Memories by topic" role="region">
      {groups.map(({ topic, memories }) => (
        <section key={topic?.id ?? "none"} className="rounded-lg border bg-card p-3">
          <h2 className="mb-1.5 flex items-center gap-2 px-1 text-sm font-semibold">
            {topic ? (
              <button type="button" className="flex items-center gap-2 hover:underline" onClick={() => onSelect({ type: "topic", id: topic.id })}>
                <span className="size-2.5 rounded-full" style={{ background: topicColor(topic.kind) }} />
                {topic.name}
                <span className="font-normal text-muted-foreground">{TOPIC_KINDS[topic.kind]?.[0]}</span>
              </button>
            ) : (
              "No topic"
            )}
          </h2>
          <ul className="grid gap-0.5">
            {memories.map((m) => (
              <li key={m.id}>
                <button
                  type="button"
                  onClick={() => onSelect({ type: "memory", id: m.id })}
                  aria-pressed={selected?.type === "memory" && selected.id === m.id}
                  className={cn("flex w-full items-baseline gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent", selected?.type === "memory" && selected.id === m.id && "bg-accent")}
                >
                  {m.pinned && <Pin className="size-3 shrink-0 self-center text-muted-foreground" />}
                  <span className="grow text-md">{m.content}</span>
                  <Pill className="shrink-0">{MEMORY_KINDS[m.kind]?.[0]}</Pill>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function AddMemory({ data, onClose, onAdded }: { data: MemoryState; onClose: () => void; onAdded: (id: string) => void }) {
  const client = useQueryClient();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const kind = String(form.get("memory-new-kind") || "");
    const topics = topicNames(String(form.get("memory-new-topics") || ""));
    const { memory } = await addMemory({
      content: String(form.get("memory-new-content") || ""),
      kind: kind ? (kind as MemoryKind) : undefined,
      pinned: form.get("memory-new-pin") === "on",
      topics: topics.length ? topics : undefined,
    });
    await refreshMemory(client);
    // The model files a memory added without a kind or topics; look again shortly.
    if (!kind || !topics.length) setTimeout(() => refreshMemory(client), 4000);
    onAdded(memory.id);
    onClose();
    toast("Memory added");
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title="Add a memory" description="Something Chat should know about you in every chat where it fits.">
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextAreaField id="memory-new-content" label="Memory" required maxLength={500} rows={3} placeholder="You teach CHM 501 and prefer worked examples." help="Write it to you, in one sentence." autoFocus />
        <TextField id="memory-new-topics" label="Topics" list="memory-new-topic-names" placeholder="CHM 501, Teaching" help="Up to three, separated by commas. Leave blank and Chat chooses." autoComplete="off" />
        <datalist id="memory-new-topic-names">
          {data.topics.map((t) => (
            <option key={t.id} value={t.name} />
          ))}
        </datalist>
        <SelectField id="memory-new-kind" label="Kind" defaultValue="" options={[["", "Let Chat choose"], ...KIND_OPTIONS]} />
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" name="memory-new-pin" className="size-4" /> Pin it, so every chat gets it
        </label>
        <FormError message={error} />
        <FormActions>
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={busy}>
            Add memory
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

