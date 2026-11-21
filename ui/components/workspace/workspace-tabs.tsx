"use client";

// The tab bar over a workspace, in a chat's side pane and on its Compute page. The
// workspace panel (preview, files and terminal) stays mounted and hidden while another
// tab such as Git, Browser or Details shows, so its terminal session and preview page
// carry on; it goes away with the host.

import { useEffect, useState } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";
import { WorkspacePanel, type WorkspaceTab } from "./workspace-panel";

export interface HostTab {
  id: string;
  label: React.ReactNode;
  icon?: React.ComponentType<{ className?: string }>;
}

const isWorkspaceTab = (tab: string | null | undefined): tab is WorkspaceTab => tab === "preview" || tab === "files" || tab === "terminal";

export function WorkspaceTabs({
  conversationId,
  tabs,
  value,
  onValueChange,
  label,
  render,
  end,
  barClassName,
  panelClassName,
}: {
  conversationId: string;
  tabs: HostTab[];
  value: string;
  onValueChange: (tab: string) => void;
  label: string;
  /** Draws a tab that is not one of the workspace's own. */
  render: (tab: string) => React.ReactNode;
  /** Beside the tabs, such as a close button. */
  end?: React.ReactNode;
  barClassName?: string;
  panelClassName?: string;
}) {
  // The workspace tab last shown, kept while another tab is in front.
  const [last, setLast] = useState<WorkspaceTab | null>(isWorkspaceTab(value) ? value : null);
  useEffect(() => {
    if (isWorkspaceTab(value)) setLast(value);
  }, [value]);
  const workspace = isWorkspaceTab(value) ? value : last && tabs.some((t) => t.id === last) ? last : null;
  return (
    <Tabs value={value} onValueChange={onValueChange} className="min-h-0 flex-1 gap-0">
      <div className={cn("flex shrink-0 items-center gap-3 border-b", barClassName)}>
        {/* Stretched 1px over the bar's border, so the underline sits on it without the row scrolling. */}
        <TabsList variant="line" aria-label={label} className="-mb-px h-auto min-w-0 flex-1 justify-start gap-1 self-stretch overflow-x-auto overflow-y-hidden p-0 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {tabs.map(({ id, label: name, icon: Icon }) => (
            <TabsTrigger
              key={id}
              value={id}
              className="h-auto flex-none rounded-none border-0 border-b-2 border-transparent px-2.5 py-2.5 font-normal text-muted-foreground after:hidden data-[state=active]:border-foreground data-[state=active]:text-foreground"
            >
              {Icon && <Icon className="size-3.5" />}
              {name}
            </TabsTrigger>
          ))}
        </TabsList>
        {end}
      </div>
      {tabs
        .filter((t) => !isWorkspaceTab(t.id))
        .map((t) => (
          <TabsContent key={t.id} value={t.id} className={panelClassName}>
            {render(t.id)}
          </TabsContent>
        ))}
      {workspace && (
        <TabsContent value={workspace} forceMount hidden={value !== workspace} className={panelClassName}>
          <WorkspacePanel conversationId={conversationId} tab={workspace} />
        </TabsContent>
      )}
    </Tabs>
  );
}
