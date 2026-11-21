"use client";

// A reply's versions side by side: what each model answered to the same question,
// what it cost and how fast it came, and a way to keep one. The same question can
// be put to another model from here, which adds a version.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { api } from "@/lib/api";
import { better, firstPair, versionStats, type ReplyVersion } from "@/lib/chat/compare";
import { ago } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useChatStore } from "./chat-provider";
import { Markdown } from "./markdown-view";

export const versionsKey = (chatId: string, messageId: string) => ["reply-versions", chatId, messageId] as const;

function Side({ versions, id, other, side, onPick, onKeep, busy }: { versions: ReplyVersion[]; id: string; other: ReplyVersion | undefined; side: "left" | "right"; onPick: (id: string) => void; onKeep: (id: string) => void; busy: boolean }) {
  const store = useChatStore();
  const version = versions.find((v) => v.id === id);
  if (!version) return null;
  const name = (v: ReplyVersion) => `${versions.indexOf(v) + 1} · ${v.model ? store.modelName(v.model) : "Unknown model"}`;
  const stats = versionStats(version),
    others = new Map(other ? versionStats(other) : []);
  return (
    <section className="flex min-h-0 min-w-0 flex-col rounded-lg border bg-card" aria-label={`Version ${versions.indexOf(version) + 1}`} data-compare-side={side}>
      <header className="flex items-center gap-2 border-b px-3 py-2">
        <Select value={id} onValueChange={onPick}>
          <SelectTrigger size="sm" className="min-w-0 flex-1" aria-label={`Version on the ${side}`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {versions.map((v) => (
              <SelectItem key={v.id} value={v.id}>
                {name(v)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="shrink-0 text-xs text-muted-foreground">{ago(version.createdAt)}</span>
      </header>
      {stats.length > 0 && (
        <dl className="flex flex-wrap gap-x-4 gap-y-1 border-b border-line-soft px-3 py-2 font-mono text-xs">
          {stats.map(([label, value]) => (
            <div key={label} className="flex gap-1.5">
              <dt className="text-muted-foreground">{label.toLowerCase()}</dt>
              <dd className={cn("tabular-nums", better(label, value, others.get(label)) === "left" ? "font-medium text-foreground" : "text-text-2")}>{value}</dd>
            </div>
          ))}
        </dl>
      )}
      <div className="min-h-40 flex-1 overflow-y-auto px-4 py-3">
        {version.content.trim() ? <Markdown text={version.content} /> : <p className="text-sm text-muted-foreground">{version.status === "completed" ? "This version has no text." : `This version did not finish (${version.status}).`}</p>}
      </div>
      <footer className="flex items-center justify-end border-t px-3 py-2">
        {version.selected ? (
          <span className="text-sm text-muted-foreground">Shown in the chat</span>
        ) : (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => onKeep(version.id)}>
            Keep this one
          </Button>
        )}
      </footer>
    </section>
  );
}

export function CompareDialog({ chatId, messageId, open, onOpenChange }: { chatId: string; messageId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const store = useChatStore();
  const query = useQuery({
    queryKey: versionsKey(chatId, messageId),
    queryFn: async () => (await api.request<{ versions?: ReplyVersion[] }>(`/api/conversations/${encodeURIComponent(chatId)}/messages/${encodeURIComponent(messageId)}/versions`)).versions ?? [],
    enabled: open,
    staleTime: 0,
  });
  const versions = query.data ?? [];
  const [picked, setPicked] = useState<[string, string] | null>(null);
  const [model, setModel] = useState("");
  const pair = picked && picked.every((id) => versions.some((v) => v.id === id)) ? picked : firstPair(versions);
  const busy = store.streaming || store.switchingVersion;
  const keep = (id: string) => {
    onOpenChange(false);
    void store.selectMessageVersion(id);
  };
  // The same question to another model: the chat goes on with that model, and its answer is a new version.
  const ask = () => {
    if (!model) return;
    onOpenChange(false);
    store.useModel(model);
    void store.run(() => store.forkAndSend(messageId, "regenerate"));
  };
  const others = store.models.filter((m) => m.id !== "auto");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[min(88vh,56rem)] flex-col gap-3 sm:max-w-6xl">
        <DialogHeader>
          <DialogTitle>Compare answers</DialogTitle>
          <DialogDescription>The versions of this reply, side by side. Keep the one the chat should go on from.</DialogDescription>
        </DialogHeader>
        {query.isLoading ? (
          <p className="py-10 text-center text-sm text-muted-foreground" role="status">
            Loading the versions…
          </p>
        ) : query.error ? (
          <p className="py-10 text-center text-sm text-destructive-foreground" role="alert">
            {(query.error as Error).message}
          </p>
        ) : !pair ? (
          <p className="py-10 text-center text-sm text-muted-foreground">This reply has one version. Ask another model below to get a second.</p>
        ) : (
          <div className="grid min-h-0 flex-1 grid-cols-2 gap-3 max-[760px]:grid-cols-1 max-[760px]:overflow-y-auto">
            <Side versions={versions} id={pair[0]} other={versions.find((v) => v.id === pair[1])} side="left" busy={busy} onKeep={keep} onPick={(id) => setPicked([id, pair[1]])} />
            <Side versions={versions} id={pair[1]} other={versions.find((v) => v.id === pair[0])} side="right" busy={busy} onKeep={keep} onPick={(id) => setPicked([pair[0], id])} />
          </div>
        )}
        {others.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 border-t pt-3">
            <span className="text-sm text-muted-foreground">Ask the same question of</span>
            <Select value={model} onValueChange={setModel}>
              <SelectTrigger size="sm" className="w-56" aria-label="Another model">
                <SelectValue placeholder="another model" />
              </SelectTrigger>
              <SelectContent>
                {others.map((m) => (
                  <SelectItem key={m.id} value={m.id}>
                    {store.modelName(m.id)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button size="sm" disabled={!model || busy} onClick={ask}>
              Ask
            </Button>
            <span className="text-xs text-muted-foreground">The chat goes on with that model.</span>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
