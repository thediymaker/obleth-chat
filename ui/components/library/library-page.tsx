"use client";

// Library → Saved and Generated. Old library.js page(), savedView() and generatedView().

import { useQueryClient } from "@tanstack/react-query";
import { Bookmark, Clock, ExternalLink, File, Folder, Image as ImageIcon, Lock, MessageSquare, Plus, Trash2 } from "lucide-react";
import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Loading, Page, PageHeader } from "@/components/app-shell/page";
import { Chips, DangerButton, Quota, SearchForm, useConfirm } from "@/components/manage/kit";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { api } from "@/lib/api";
import { when, bytes } from "@/lib/format";
import { useProjects } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { host, KINDS, kindLabel, kindOf, type SavedItem, type SavedKind, useGeneratedImages, useSaved, SAVED_PAGE } from "./api";
import { LibraryTabs } from "./library-shell";
import { useLibraryDialogs } from "./saved-dialogs";

function Preview({ item }: { item: SavedItem }) {
  if (item.kind === "code")
    return (
      <pre className="max-h-36 overflow-hidden rounded-md bg-code p-2.5 font-mono text-xs leading-relaxed text-text-2">
        <code>{item.body.split("\n").slice(0, 7).join("\n")}</code>
      </pre>
    );
  if (item.kind === "link")
    return (
      <>
        {item.url && (
          <span className="inline-flex items-center gap-1 text-xs text-blue">
            <ExternalLink className="size-3" /> {host(item.url)}
          </span>
        )}
        {item.body && <p className="line-clamp-3 text-sm text-muted-foreground">{item.body}</p>}
      </>
    );
  if (item.kind === "excerpt") return <blockquote className="line-clamp-4 border-l-2 pl-3 text-sm text-text-2 italic">{item.body}</blockquote>;
  return item.body ? <p className="line-clamp-4 text-sm text-muted-foreground">{item.body}</p> : null;
}

function SavedCard({ item, onOpen }: { item: SavedItem; onOpen: () => void }) {
  const [Icon] = kindOf(item.kind);
  return (
    <article
      role="button"
      tabIndex={0}
      aria-label={`Open ${item.title}`}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
      className="flex cursor-pointer flex-col gap-2 rounded-lg border bg-card p-4 transition-colors outline-none hover:border-line-strong hover:bg-raised focus-visible:ring-2 focus-visible:ring-ring/50"
    >
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Icon className="size-3.5" />
        <span className="grow">{kindLabel(item)}</span>
        {!item.searchable && (
          <span title="Chats cannot find it">
            <Lock className="size-3.5" />
            <span className="sr-only">Chats cannot find it</span>
          </span>
        )}
      </div>
      <h3 className="text-md font-semibold">{item.title}</h3>
      <Preview item={item} />
      {item.tags.length > 0 && (
        <div className="flex flex-wrap gap-1.5 text-xs text-faint">
          {item.tags.map((t) => (
            <span key={t}>#{t}</span>
          ))}
        </div>
      )}
      <div className="mt-auto flex items-center gap-1.5 pt-1 text-xs text-faint">
        {item.source ? (
          <>
            <MessageSquare className="size-3.5" />
            <span className="truncate">From “{item.source.title || "a chat"}”</span>
          </>
        ) : (
          <>
            <Clock className="size-3.5" />
            <span>{when(item.createdAt)}</span>
          </>
        )}
      </div>
    </article>
  );
}

const ALL = "all";

function SavedView({ openId, dialogs }: { openId?: string; dialogs: ReturnType<typeof useLibraryDialogs> }) {
  const router = useRouter();
  const path = usePathname();
  const params = useSearchParams();
  const project = params.get("project") || "";
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<"" | SavedKind>("");
  const [tag, setTag] = useState("");
  const filters = { q: query, kind, tag, project };
  const [shown, setShown] = useState(SAVED_PAGE);
  const { data, error, isPending, refetch } = useSaved(filters, true, shown);
  const { data: projects } = useProjects();
  const opened = useRef("");
  // /library/<id> opens that item, once.
  useEffect(() => {
    if (openId && opened.current !== openId) {
      opened.current = openId;
      dialogs.open(openId);
    }
  }, [openId, dialogs]);

  const items = data?.items;
  const kinds = data?.kinds ?? [];
  const total = kinds.reduce((n, k) => n + k.count, 0);
  const countOf = (k: string) => kinds.find((x) => x.kind === k)?.count || 0;
  const all = projects?.projects ?? [];
  const chosen = all.find((p) => p.id === project);
  const setProject = (id: string) => {
    setQuery("");
    setKind("");
    setTag("");
    router.replace(id ? `${path}?project=${encodeURIComponent(id)}` : path);
  };
  const filtered = !!(query || kind || tag || project);
  return (
    <>
      <SearchForm id="library-query" label="Search saved things" placeholder="Search saved things, like “that throughput function”" maxLength={500} value={query} onSearch={setQuery} className="mb-4 sm:max-w-none">
        {query && (
          <Button type="button" variant="ghost" onClick={() => setQuery("")}>
            Clear
          </Button>
        )}
      </SearchForm>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <Chips
          label="Kind"
          value={kind}
          onChange={setKind}
          options={[["", "All", { icon: Bookmark, count: total || undefined }], ...(Object.entries(KINDS) as [SavedKind, (typeof KINDS)[SavedKind]][]).map(([k, [Icon, label]]) => [k, label, { icon: Icon, count: countOf(k) || undefined }] as const)]}
        />
        {all.length > 0 && (
          <Select value={project || ALL} onValueChange={(v) => setProject(v === ALL ? "" : v)}>
            <SelectTrigger aria-label="Project" className="w-56">
              <Folder className="size-4 text-muted-foreground" />
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All my saved things</SelectItem>
              {all.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.name}
                  {p.mine ? "" : ` · ${String(p.ownerName ?? "")}`}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>
      {chosen && !chosen.mine && <p className="mb-3 text-sm text-muted-foreground">Showing everything saved in this shared project, by anyone in it. Kind and tag counts are your own.</p>}
      {(data?.tags?.length ?? 0) > 0 && (
        <div className="mb-4 flex flex-wrap items-center gap-1.5">
          {data!.tags.slice(0, 16).map((t) => (
            <button
              key={t.tag}
              type="button"
              aria-pressed={tag === t.tag}
              onClick={() => setTag(tag === t.tag ? "" : t.tag)}
              className={cn("inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground", tag === t.tag && "bg-secondary text-foreground")}
            >
              #{t.tag}
              <span className="text-faint">{t.count}</span>
            </button>
          ))}
          {tag && (
            <Button variant="ghost" size="xs" onClick={() => setTag("")}>
              Clear
            </Button>
          )}
        </div>
      )}
      <ErrorBox error={error} onRetry={() => refetch()} />
      {isPending ? (
        <Loading />
      ) : !items?.length ? (
        filtered ? (
          <p className="py-6 text-sm text-muted-foreground">Nothing matches. Try other words or clear the filters.</p>
        ) : (
          !error && (
            <EmptyState
              icon={Bookmark}
              title="Nothing saved yet"
              text="In a chat, use Save on a code block or a reply, select text and choose Save, or say “remember this”. You can also add a note or a link here."
              action={
                <Button onClick={() => dialogs.create("note")}>
                  <Plus /> New note
                </Button>
              }
            />
          )
        )
      ) : (
        <>
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {items.map((item) => (
              <SavedCard key={item.id} item={item} onOpen={() => dialogs.show(item)} />
            ))}
          </div>
          {data?.more && (
            <div className="py-4">
              <Button variant="outline" size="sm" onClick={() => setShown(shown + SAVED_PAGE)}>
                Show more
              </Button>
            </div>
          )}
        </>
      )}
    </>
  );
}

function GeneratedView() {
  const client = useQueryClient();
  const confirm = useConfirm();
  const { data, error, isPending, refetch } = useGeneratedImages();
  const images = data?.images ?? [];
  const remove = (id: string) =>
    confirm.ask({
      title: "Delete this image?",
      text: "It is deleted for good; chats that showed it lose it.",
      label: "Delete image",
      cancel: "Keep it",
      action: async () => {
        await api.request(`/api/images/${encodeURIComponent(id)}`, { method: "DELETE" });
        client.setQueryData(["library", "images"], { ...data, images: images.filter((i) => i.id !== id) });
        toast("Image deleted");
      },
    });
  if (isPending) return <Loading />;
  return (
    <>
      {confirm.element}
      {data?.usage && (
        <div className="mb-4 flex justify-end">
          <Quota label="Generated images" value={data.usage.used} max={data.usage.limit} caption={`${bytes(data.usage.used)} of ${bytes(data.usage.limit)}`} />
        </div>
      )}
      <ErrorBox error={error} onRetry={() => refetch()} />
      {images.length ? (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          {images.map((i) => (
            <figure key={i.id} className="group overflow-hidden rounded-lg border bg-card">
              <a href={i.url} target="_blank" rel="noopener noreferrer" className="block aspect-square overflow-hidden bg-muted">
                {/* eslint-disable-next-line @next/next/no-img-element -- served by Chat with the session */}
                <img src={i.url} alt={`Generated in ${i.conversationTitle}`} loading="lazy" className="size-full object-cover transition-transform group-hover:scale-[1.02]" />
              </a>
              <figcaption className="flex items-start gap-2 p-2.5">
                <div className="min-w-0 grow">
                  <Link href={`/chat/${encodeURIComponent(i.conversationId)}`} title={i.conversationTitle} className="block truncate text-sm font-medium hover:underline">
                    {i.conversationTitle}
                  </Link>
                  <span className="text-xs text-faint">
                    {when(i.createdAt)} · {bytes(i.size)}
                  </span>
                </div>
                <DangerButton size="icon-sm" aria-label="Delete image" title="Delete" onClick={() => remove(i.id)}>
                  <Trash2 />
                </DangerButton>
              </figcaption>
            </figure>
          ))}
        </div>
      ) : (
        !error && <EmptyState icon={ImageIcon} title="No generated images" text="Images a model makes in a chat are kept here. They are deleted with their chat." />
      )}
    </>
  );
}

export function LibraryPage({ tab, openId }: { tab: "saved" | "generated"; openId?: string }) {
  usePageChrome(tab === "generated" ? "Library › Generated" : "Library");
  const dialogs = useLibraryDialogs();
  return (
    <Page>
      <PageHeader
        title="Library"
        description={tab === "generated" ? "Pictures made in your chats, kept here to find again." : "Everything worth keeping: functions, articles, notes and pictures you saved while thinking."}
        actions={
          <>
            <Button variant="outline" onClick={() => dialogs.create("note")}>
              <File /> New note
            </Button>
            <Button onClick={() => dialogs.create("link")}>
              <Plus /> Save a link
            </Button>
          </>
        }
      >
        <LibraryTabs />
      </PageHeader>
      {tab === "generated" ? <GeneratedView /> : <SavedView openId={openId} dialogs={dialogs} />}
      {dialogs.element}
    </Page>
  );
}
