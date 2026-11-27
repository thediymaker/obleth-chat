"use client";

// A reply's versions side by side, up to three: what each model answered to the same
// question, what it cost and how fast it came, and a way to keep one, which the chat
// then goes on from. The same question can be put to another model from here, which
// adds a version. Chat also adds versions by itself now and then (lib/chat/compare).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { api } from "@/lib/api";
import { best, firstShown, versionStats, type ReplyVersion } from "@/lib/chat/compare";
import { ago } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useChatStore } from "./chat-provider";
import { Markdown } from "./markdown-view";

export const versionsKey = (chatId: string, messageId: string) => ["reply-versions", chatId, messageId] as const;

/** How many versions are read side by side. */
const COLUMNS = 3;

function Side({ versions, id, shown, onPick, onKeep, busy }: { versions: ReplyVersion[]; id: string; shown: string[]; onPick: (id: string) => void; onKeep: (id: string) => void; busy: boolean }) {
  const store = useChatStore();
  const version = versions.find((v) => v.id === id);
  if (!version) return null;
  const position = shown.indexOf(id) + 1;
  const name = (v: ReplyVersion) => `${versions.indexOf(v) + 1} · ${v.model ? store.modelName(v.model) : "Unknown model"}`;
  const stats = versionStats(version),
    others = versions.filter((v) => v.id !== id && shown.includes(v.id)).map((v) => new Map(versionStats(v)));
  const model = version.model ? store.modelName(version.model) : "this model";
  return (
    <section className={cn("flex min-h-0 min-w-0 flex-col rounded-lg border bg-card", version.selected && "border-line-strong")} aria-label={`Version ${versions.indexOf(version) + 1}`} data-compare-side={position}>
      <header className="flex items-center gap-2 border-b px-3 py-2">
        <Select value={id} onValueChange={onPick}>
          <SelectTrigger size="sm" className="min-w-0 flex-1" aria-label={`Version in column ${position}`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {versions.map((v) => (
              <SelectItem key={v.id} value={v.id} disabled={v.id !== id && shown.includes(v.id)}>
                {name(v)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="shrink-0 text-xs text-muted-foreground">{version.auto ? "Auto’s pick" : ago(version.createdAt)}</span>
      </header>
      {stats.length > 0 && (
        <dl className="flex flex-wrap gap-x-4 gap-y-1 border-b border-line-soft px-3 py-2 font-mono text-xs">
          {stats.map(([label, value]) => (
            <div key={label} className="flex gap-1.5">
              <dt className="text-muted-foreground">{label.toLowerCase()}</dt>
              <dd
                className={cn(
                  "tabular-nums",
                  best(
                    label,
                    value,
                    others.map((o) => o.get(label)),
                  )
                    ? "font-medium text-foreground"
                    : "text-text-2",
                )}
              >
                {value}
              </dd>
            </div>
          ))}
        </dl>
      )}
      <div className="min-h-40 flex-1 overflow-y-auto px-4 py-3 max-[760px]:min-h-0 max-[760px]:overflow-visible">
        {version.content.trim() ? <Markdown text={version.content} /> : <p className="text-sm text-muted-foreground">{version.status === "completed" ? "This version has no text." : `This version did not finish (${version.status}).`}</p>}
      </div>
      <footer className="flex items-center justify-end border-t px-3 py-2">
        {version.selected ? (
          <span className="text-sm text-muted-foreground">Shown in the chat</span>
        ) : (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => onKeep(version.id)} aria-label={`Keep this answer, from ${model}`}>
            Keep this answer
          </Button>
        )}
      </footer>
    </section>
  );
}

export function CompareDialog({
  chatId,
  messageId,
  open,
  onOpenChange,
  waiting = false,
  suggest = "",
}: {
  chatId: string;
  messageId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** More answers may still arrive: Chat is asking other models in the background. */
  waiting?: boolean;
  /** The model to offer first for asking again. */
  suggest?: string;
}) {
  const store = useChatStore();
  const query = useQuery({
    queryKey: versionsKey(chatId, messageId),
    queryFn: async () => (await api.request<{ versions?: ReplyVersion[] }>(`/api/conversations/${encodeURIComponent(chatId)}/messages/${encodeURIComponent(messageId)}/versions`)).versions ?? [],
    enabled: open,
    staleTime: 0,
    refetchInterval: waiting ? 3000 : false,
  });
  const versions = query.data ?? [];
  const [picked, setPicked] = useState<string[] | null>(null);
  const others = store.models.filter((m) => m.id !== "auto");
  const [model, setModel] = useState(others.some((m) => m.id === suggest) ? suggest : "");
  // What the person arranged stands while those versions exist; otherwise the one in the chat and the newest others.
  const arranged = picked && picked.every((id) => versions.some((v) => v.id === id)) && picked.length === Math.min(COLUMNS, versions.length);
  const shown = arranged ? picked : firstShown(versions, COLUMNS);
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
  const second = versions.some((v) => v.second);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={cn("flex max-h-[min(88vh,56rem)] flex-col gap-3", shown.length > 2 ? "sm:max-w-[min(96vw,92rem)]" : "sm:max-w-6xl")}>
        <DialogHeader>
          <DialogTitle>Compare answers</DialogTitle>
          <DialogDescription>
            {second ? "The same message, answered by Auto’s pick and by the models it ranked next." : "The versions of this reply, side by side."} Keep the one the chat should go on from; the chat then continues with that
            answer’s model.
          </DialogDescription>
        </DialogHeader>
        {query.isLoading ? (
          <p className="py-10 text-center text-sm text-muted-foreground" role="status">
            Loading the versions…
          </p>
        ) : query.error ? (
          <p className="py-10 text-center text-sm text-destructive-foreground" role="alert">
            {(query.error as Error).message}
          </p>
        ) : shown.length < 2 ? (
          <p className="py-10 text-center text-sm text-muted-foreground">{waiting ? "The other models are still answering…" : "This reply has one version. Ask another model below to get a second."}</p>
        ) : (
          // Side by side; on a phone, one under the other, each at its full height, and they scroll together.
          <div className={cn("grid min-h-0 flex-1 gap-3 max-[760px]:block max-[760px]:space-y-3 max-[760px]:overflow-y-auto", shown.length > 2 ? "grid-cols-3" : "grid-cols-2")} data-compare-columns={shown.length}>
            {shown.map((id, column) => (
              <Side key={id} versions={versions} id={id} shown={shown} busy={busy} onKeep={keep} onPick={(next) => setPicked(shown.map((old, i) => (i === column ? next : old)))} />
            ))}
          </div>
        )}
        {waiting && shown.length >= 2 && (
          <p className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
            <span className="relative size-1.5">
              <span className="signal-pulse absolute inset-0 rounded-full bg-signal" />
              <span className="absolute inset-0 rounded-full bg-signal" />
            </span>
            Another model is still answering.
          </p>
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
