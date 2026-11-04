"use client";

// Compute › one workspace: the chat's
// workspace full width, with its Preview, Files, Terminal and Git tabs.

import { ArrowLeft, FileText, GitBranch, MessageSquare, Play, Terminal } from "lucide-react";
import Link from "next/link";
import { useEffect, useState } from "react";
import { usePageChrome } from "@/components/app-shell/chrome";
import { Page } from "@/components/app-shell/page";
import { useChat } from "@/components/chat/chat-provider";
import { CodePanel, CodeTabLabel } from "@/components/code/code-panel";
import { Button } from "@/components/ui/button";
import { useConversations, useFeature } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { ComputeOff } from "./compute";
import { usePreviewsFlag, WorkspacePanel, type WorkspaceTab } from "./workspace-panel";
import { useWorkspaceList } from "./workspaces-page";

type Tab = WorkspaceTab | "code";

export function WorkspacePage({ conversationId }: { conversationId: string }) {
  const store = useChat();
  const bridge = store.workspace;
  const on = useFeature("workspaces");
  const code = useFeature("code");
  const previews = usePreviewsFlag(bridge);
  const list = useWorkspaceList();
  const chats = useConversations();
  const item = list.data?.items.find((p) => p.conversationId === conversationId);
  const title = item?.title || chats.data?.find((c) => c.id === conversationId)?.title || "Project";
  usePageChrome(title);
  const space = bridge.space(conversationId);
  useEffect(() => {
    if (on) void bridge.load(conversationId);
  }, [on, bridge, conversationId]);
  const tabs: [Tab, string, typeof Play][] = [...(previews ? [["preview", "Preview", Play] as [Tab, string, typeof Play]] : []), ["files", "Files", FileText], ["terminal", "Terminal", Terminal], ...(code ? [["code", "Git", GitBranch] as [Tab, string, typeof Play]] : [])];
  const [tab, setTab] = useState<Tab>("preview");
  const current: Tab = tabs.some(([t]) => t === tab) ? tab : tabs[0]![0];
  // The preview and terminal stay alive behind the Git tab.
  const [last, setLast] = useState<WorkspaceTab>("preview");
  useEffect(() => {
    if (current !== "code") setLast(current);
  }, [current]);
  if (store.capabilities && !on)
    return (
      <Page>
        <ComputeOff page="workspaces" />
      </Page>
    );
  const missing = space.loaded && !space.workspace;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2.5">
        <Button asChild variant="ghost" size="sm">
          <Link href="/workspaces">
            <ArrowLeft /> Workspaces
          </Link>
        </Button>
        <h1 className="min-w-0 flex-1 truncate text-base font-semibold" title={title}>
          {title}
        </h1>
        <Button asChild variant="outline" size="sm">
          <Link href={`/chat/${encodeURIComponent(conversationId)}`}>
            <MessageSquare /> Open chat
          </Link>
        </Button>
      </div>
      {missing ? (
        <Page>
          <p className="text-sm text-muted-foreground">
            This conversation has no workspace.{" "}
            <Link href="/workspaces" className="underline underline-offset-2">
              Back to your workspaces
            </Link>
          </p>
        </Page>
      ) : (
        <>
          <div className="flex gap-0.5 overflow-x-auto border-b px-2 py-1.5" role="tablist" aria-label="Workspace">
            {tabs.map(([t, label, Icon]) => (
              <button
                key={t}
                type="button"
                role="tab"
                aria-selected={current === t}
                onClick={() => setTab(t)}
                className={cn("inline-flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1 text-sm", current === t ? "bg-secondary text-foreground" : "text-muted-foreground hover:text-foreground")}
              >
                <Icon className="size-3.5" />
                {t === "code" ? <CodeTabLabel conversationId={conversationId} /> : label}
              </button>
            ))}
          </div>
          <div className="h-[calc(100dvh-10rem)] min-h-96">
            {current === "code" && <CodePanel conversationId={conversationId} />}
            <div className={cn("h-full", current === "code" && "hidden")}>
              <WorkspacePanel conversationId={conversationId} tab={current === "code" ? last : current} />
            </div>
          </div>
        </>
      )}
    </div>
  );
}
