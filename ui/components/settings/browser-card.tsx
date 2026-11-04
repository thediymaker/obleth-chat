"use client";

// The person's browser in Settings: whether it runs, the chats with a tab, the search
// engine, sites allowed to submit forms, and putting it to sleep or forgetting it all.
// (browser.js settingsCard; the rest of the browser belongs to the chat page.)

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { X } from "lucide-react";
import Link from "next/link";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { DetailRow, Field, NativeSelect, Note, Pill, Section, useConfirm } from "@/components/manage/kit";
import { api } from "@/lib/api";

export interface BrowserSettings {
  status: string;
  idleMinutes?: number;
  searchEngine?: string;
  tabs?: { conversationId: string; title?: string; url?: string; controller?: string }[];
  sites?: { host: string }[];
}
export const browserKey = ["settings", "browser"] as const;
const ENGINES = [
  ["duckduckgo", "DuckDuckGo"],
  ["google", "Google"],
  ["bing", "Bing"],
] as const;
const STATUS: Record<string, string> = { running: "Running", starting: "Starting", stopped: "Asleep", error: "Error" };
const hostOf = (url?: string) => {
  try {
    return url ? new URL(url).host : "";
  } catch {
    return "";
  }
};

export function BrowserCard() {
  const queries = useQueryClient();
  const confirm = useConfirm();
  const { data: d, error } = useQuery({ queryKey: browserKey, queryFn: () => api.request<BrowserSettings>("/api/browser") });
  const reload = () => queries.invalidateQueries({ queryKey: browserKey });
  const engine = useMutation({
    mutationFn: (searchEngine: string) => api.request("/api/browser", { method: "PUT", body: { searchEngine } }),
    onSuccess: (_r, searchEngine) => {
      queries.setQueryData<BrowserSettings>(browserKey, (old) => (old ? { ...old, searchEngine } : old));
      toast("Search engine saved");
    },
    onError: (e) => toast(e.message),
  });
  if (!d)
    return (
      <Section title="Browser">
        <Note>{error ? error.message : "Loading…"}</Note>
      </Section>
    );
  return (
    <Section title="Browser" actions={<Pill tone={d.status === "running" ? "accent" : d.status === "error" ? "amber" : ""}>{STATUS[d.status] || d.status}</Pill>}>
      {confirm.element}
      <Note>One Chrome for you, shared by your chats: each chat with the browser on has its own tab. It sleeps after {d.idleMinutes} minutes unused and keeps your sign-ins.</Note>
      {!!d.tabs?.length && (
        <div>
          {d.tabs.slice(0, 5).map((t) => (
            <Link key={t.conversationId} href={`/chat/${encodeURIComponent(t.conversationId)}`} className="block hover:bg-accent">
              <DetailRow label={t.title || "Untitled chat"}>{t.controller === "user" ? "You have control" : hostOf(t.url) || "No page yet"}</DetailRow>
            </Link>
          ))}
        </div>
      )}
      <Field id="browser-engine" label="Search with" help="The model searches by opening this search page in your browser.">
        <NativeSelect id="browser-engine" value={d.searchEngine} onChange={(e) => engine.mutate(e.target.value)}>
          {ENGINES.map(([v, n]) => (
            <option key={v} value={v}>
              {n}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <div className="grid gap-1">
        <h3 className="text-md font-medium">Ask before</h3>
        <Note>Submitting a form, or pressing a button that buys, sends, posts or deletes something, asks you first. Passwords, sign-ins and captchas are always yours: the model hands you the tab.</Note>
      </div>
      {!!d.sites?.length && (
        <div className="grid gap-1">
          <h3 className="text-md font-medium">Always allowed to submit on</h3>
          {d.sites.map((s) => (
            <DetailRow key={s.host} label={s.host}>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Ask again on ${s.host}`}
                title={`Ask again on ${s.host}`}
                onClick={async () => {
                  try {
                    await api.request(`/api/browser/sites/${encodeURIComponent(s.host)}`, { method: "DELETE" });
                    await reload();
                  } catch (e) {
                    toast((e as Error).message);
                  }
                }}
              >
                <X />
              </Button>
            </DetailRow>
          ))}
        </div>
      )}
      <div className="flex flex-wrap justify-end gap-2">
        {d.status === "running" && (
          <Button
            size="sm"
            variant="outline"
            onClick={async () => {
              try {
                await api.request("/api/browser/sleep", { method: "POST" });
                await reload();
              } catch (e) {
                toast((e as Error).message);
              }
            }}
          >
            Sleep now
          </Button>
        )}
        <Button
          size="sm"
          variant="outline"
          className="text-destructive-foreground"
          onClick={() =>
            confirm.ask({
              title: "Forget all browser data?",
              text: "Your sign-ins, cookies and history are deleted, every chat’s tab closes, and sites you allowed to submit forms ask again.",
              label: "Forget everything",
              cancel: "Keep it",
              action: async () => {
                await api.request("/api/browser", { method: "DELETE" });
                toast("Browser data deleted");
                await reload();
              },
            })
          }
        >
          Forget all browser data
        </Button>
      </div>
    </Section>
  );
}
