"use client";

// Search conversations (⌘K). With nothing typed it lists recent chats; typing asks
// the server, which searches titles and message text in every chat, archived ones
// too (the same search All chats uses). See all results opens that page.

import { ArrowRight, MessageSquare, Pin } from "lucide-react";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Badge } from "@/components/ui/badge";
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { fetchHistory, type HistoryChat } from "@/lib/chat/history";
import { ago } from "@/lib/format";
import { keys, useConversations, useModels } from "@/lib/queries";
import { useSession } from "@/lib/session";

const DEBOUNCE_MS = 200;
const RESULTS = 20;

type Found = HistoryChat;

export function SearchDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const router = useRouter();
  const signedIn = useSession().phase === "ready";
  const { data: recent = [], isLoading: recentLoading } = useConversations();
  const { data: models = [] } = useModels();
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");

  // Each dialog starts with an empty search.
  useEffect(() => {
    if (!open) {
      setText("");
      setQuery("");
    }
  }, [open]);
  useEffect(() => {
    const trimmed = text.trim().slice(0, 256);
    const timer = setTimeout(() => setQuery(trimmed), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text]);

  const typed = text.trim();
  const results = useQuery({
    queryKey: [...keys.conversations, "search", query],
    queryFn: ({ signal }) => fetchHistory(query, "all", 0, RESULTS, signal),
    enabled: signedIn && open && !!query,
    staleTime: 10_000,
  });
  const waiting = !!typed && (typed !== query || results.isFetching);
  const modelName = (id: string) => models.find((m) => m.id === id)?.name || id;

  function go(path: string) {
    onOpenChange(false);
    router.push(path);
  }
  const row = (chat: Found) => (
    <CommandItem key={chat.id} value={chat.id} onSelect={() => go(`/chat/${encodeURIComponent(chat.id)}`)} className="items-start">
      {chat.pinned ? <Pin className="mt-0.5" /> : <MessageSquare className="mt-0.5" />}
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span className="truncate">{chat.title}</span>
          {chat.archived && <Badge variant="secondary">Archived</Badge>}
        </span>
        <span className="block truncate text-xs text-muted-foreground">
          {modelName(chat.model)}
          {chat.updatedAt || chat.createdAt ? ` · ${ago(chat.updatedAt || chat.createdAt)}` : ""}
        </span>
      </span>
    </CommandItem>
  );

  const found = (results.data?.conversations ?? []) as Found[];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="overflow-hidden p-0" showCloseButton>
        <DialogTitle className="sr-only">Search conversations</DialogTitle>
        <DialogDescription className="sr-only">Find a chat by its title or what was said in it</DialogDescription>
        <Command
          shouldFilter={false}
          className="**:data-[slot=command-input-wrapper]:h-12 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group]]:px-2 [&_[cmdk-input]]:h-12 [&_[cmdk-item]]:px-2 [&_[cmdk-item]]:py-2.5"
        >
          <CommandInput placeholder="Search titles and messages…" value={text} onValueChange={setText} maxLength={256} aria-label="Search conversations" />
          <CommandList>
            {!typed ? (
              recent.length ? (
                <CommandGroup heading="Recent">{(recent as Found[]).slice(0, 15).map(row)}</CommandGroup>
              ) : (
                <div className="py-6 text-center text-sm text-muted-foreground" role="status">
                  {recentLoading ? "Loading conversations…" : "Your conversations will appear here as you use Chat."}
                </div>
              )
            ) : results.error && !waiting ? (
              <div role="alert" className="px-4 py-6 text-center text-sm text-destructive-foreground">
                {(results.error as Error).message}
              </div>
            ) : waiting && !found.length ? (
              <div role="status" className="py-6 text-center text-sm text-muted-foreground">
                Searching…
              </div>
            ) : (
              <>
                {!found.length && <CommandEmpty>No conversations found.</CommandEmpty>}
                {found.length > 0 && (
                  <CommandGroup heading={results.data && results.data.total > found.length ? `Chats · ${found.length} of ${results.data.total}` : "Chats"}>{found.map(row)}</CommandGroup>
                )}
                <CommandGroup>
                  <CommandItem value="see-all" onSelect={() => go(`/history?query=${encodeURIComponent(typed)}&archived=all`)}>
                    <ArrowRight /> See all results for “{typed}”
                  </CommandItem>
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </DialogContent>
    </Dialog>
  );
}
