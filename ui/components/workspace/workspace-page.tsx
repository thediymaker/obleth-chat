"use client";

// Compute › one workspace: the chat's
// workspace full width, with its Preview, Files, Terminal and Git tabs.

import { ArrowLeft, FileText, GitBranch, MessageSquare, Play, Terminal } from "lucide-react";
import Link from "next/link";
import { useEffect, useState } from "react";
import { usePageChrome } from "@/components/app-shell/chrome";
import { Page, NotFound } from "@/components/app-shell/page";
import { useChat } from "@/components/chat/chat-provider";
import { CodePanel, CodeTabLabel } from "@/components/code/code-panel";
import { Button } from "@/components/ui/button";
import { useConversations, useFeature } from "@/lib/queries";
import { ComputeOff } from "./compute";
import { usePreviewsFlag } from "./workspace-panel";
import { type HostTab, WorkspaceTabs } from "./workspace-tabs";
import { useWorkspaceList } from "./workspaces-page";

export function WorkspacePage({ conversationId }: { conversationId: string }) {
  const store = useChat();
  const bridge = store.workspace;
  const on = useFeature("workspaces");
  const code = useFeature("code");
  const previews = usePreviewsFlag(bridge);
  const list = useWorkspaceList();
  const chats = useConversations();
  const item = list.data?.items.find((p) => p.conversationId === conversationId);
  const title = item?.title || chats.data?.find((c) => c.id === conversationId)?.title || "Workspace";
  usePageChrome(title);
  const space = bridge.space(conversationId);
  useEffect(() => {
    if (on) void bridge.load(conversationId);
  }, [on, bridge, conversationId]);
  const tabs: HostTab[] = [
    ...(previews ? [{ id: "preview", label: "Preview", icon: Play }] : []),
    { id: "files", label: "Files", icon: FileText },
    { id: "terminal", label: "Terminal", icon: Terminal },
    ...(code ? [{ id: "code", label: <CodeTabLabel conversationId={conversationId} />, icon: GitBranch }] : []),
  ];
  const [tab, setTab] = useState("preview");
  const current = tabs.some((t) => t.id === tab) ? tab : tabs[0]!.id;
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
          <NotFound what="workspace" to="/workspaces" back="your workspaces" />
        </Page>
      ) : (
        <WorkspaceTabs
          conversationId={conversationId}
          label="Workspace"
          tabs={tabs}
          value={current}
          onValueChange={setTab}
          barClassName="px-2"
          panelClassName="h-[calc(100dvh-10rem)] min-h-96"
          render={() => <CodePanel conversationId={conversationId} />}
        />
      )}
    </div>
  );
}
