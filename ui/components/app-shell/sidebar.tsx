"use client";

import { Bookmark, ChevronDown, ChevronRight, Folder, LogOut, MessageSquare, MessageSquarePlus, MoreHorizontal, Plus, Search, Server, Settings, Users } from "lucide-react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { Brand } from "@/components/brand";
import { ChatMenuItems } from "@/components/chat/chat-menu";
import { NEW_CHAT_EVENT } from "@/components/chat/chat-provider";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { initials } from "@/lib/format";
import { useCapabilities, useConversations, useProjects } from "@/lib/queries";
import { useSession, useUser } from "@/lib/session";
import { cn } from "@/lib/utils";

const PROJECT_DOT: Record<string, string> = {
  blue: "bg-project-blue",
  green: "bg-project-green",
  gold: "bg-project-gold",
  rose: "bg-project-rose",
  violet: "bg-project-violet",
  gray: "bg-project-gray",
};
export const projectDot = (color: string) => PROJECT_DOT[color] ?? "bg-project-gray";

/** Which top-level section a path belongs to, for the active nav item. */
export function sectionOf(path: string): string {
  const first = path.split("/")[1] ?? "";
  if (first === "" || first === "chat" || first === "history") return "chat";
  if (first === "knowledge" || first === "library") return "library";
  if (first === "workspaces" || first === "clusters") return "compute";
  return first;
}

export function Sidebar({ onNavigate, onSearch, onFeedback }: { onNavigate?: () => void; onSearch: () => void; onFeedback: () => void }) {
  const path = usePathname();
  const router = useRouter();
  const user = useUser();
  const { signOut } = useSession();
  const { data: capabilities } = useCapabilities();
  const { data: chats = [] } = useConversations();
  const { data: projects } = useProjects();
  const features = capabilities?.features ?? {};
  const section = sectionOf(path);
  const name = user.display_name || user.username;
  const nav = [
    { id: "chat", href: "/", label: "Chat", icon: MessageSquare, on: true },
    { id: "projects", href: "/projects", label: "Projects", icon: Folder, on: true },
    { id: "library", href: "/library", label: "Library", icon: Bookmark, on: !!features.knowledge },
    { id: "compute", href: features.workspaces ? "/workspaces" : "/clusters", label: "Compute", icon: Server, on: !!features.workspaces || !!features.clusters },
    { id: "groups", href: "/groups", label: "Groups", icon: Users, on: true },
  ].filter((item) => item.on);
  const chatId = path.startsWith("/chat/") ? decodeURIComponent(path.split("/")[2] ?? "") : "";
  const projectId = path.startsWith("/projects/") ? decodeURIComponent(path.split("/")[2] ?? "") : "";
  const settingsOn = path.startsWith("/settings");
  // Administration › Feedback sets how the button shows; highlighted suits a site in development.
  const feedback = capabilities?.feedback ?? "off";
  const openFeedback = () => {
    onNavigate?.();
    onFeedback();
  };
  return (
    // 224px (208px up to 1200px wide), 8px gutters, the brand bar as tall as the top bar.
    <aside aria-label="Main navigation" className="flex h-full w-56 shrink-0 flex-col border-r leading-[normal] border-sidebar-border bg-sidebar px-2 text-sidebar-foreground max-[1200px]:w-52">
      <div className="-mx-2 flex h-14 shrink-0 items-center border-b border-sidebar-border px-4">
        <Brand />
      </div>
      <Button
        className="my-3 h-9 w-full justify-start gap-2 px-3 text-md"
        onClick={() => {
          // A fresh chat: the store clears the draft; just opening / keeps it.
          window.dispatchEvent(new Event(NEW_CHAT_EVENT));
          router.push("/");
          onNavigate?.();
        }}
      >
        <Plus /> New chat
        <kbd className="ml-auto rounded-sm border border-current px-[5px] py-px font-sans text-2xs font-normal">⇧⌘O</kbd>
      </Button>
      <nav className="grid gap-0.5">
        {nav.map(({ id, href, label, icon: Icon }) => (
          <Link
            key={id}
            href={href}
            onClick={onNavigate}
            aria-current={section === id ? "page" : undefined}
            className={cn(
              "flex items-center gap-2.5 rounded-md px-3 py-2 text-base leading-[normal] text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
              section === id && "bg-sidebar-accent text-sidebar-accent-foreground",
            )}
          >
            <Icon className="size-4 opacity-80" />
            {label}
          </Link>
        ))}
      </nav>
      {!!projects?.projects?.length && (
        <div className="mt-3 grid border-t border-sidebar-border pt-3">
          <Link href="/projects" onClick={onNavigate} className="mb-1 pl-3 text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase hover:text-foreground">
            Projects
          </Link>
          {projects.projects.slice(0, 5).map((p) => (
            <Link
              key={p.id}
              href={`/projects/${encodeURIComponent(p.id)}`}
              onClick={onNavigate}
              title={p.name}
              className={cn("flex min-w-0 items-center gap-2.5 rounded-md px-2.5 py-1.5 text-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground", projectId === p.id && "bg-sidebar-accent text-sidebar-accent-foreground")}
            >
              <span className={cn("size-2 shrink-0 rounded-full", projectDot(p.color))} />
              <span className="truncate">{p.name}</span>
            </Link>
          ))}
        </div>
      )}
      <div className="mt-3 flex min-h-0 flex-1 flex-col border-t border-sidebar-border pt-3">
        <div className="mb-1 flex items-center justify-between pr-1 pl-3">
          <span className="text-2xs font-medium tracking-[.08em] text-muted-foreground uppercase">Recent</span>
          <Button variant="ghost" size="icon-xs" className="size-7 [&_svg]:size-3.5" aria-label="Search conversations" title="Search conversations" onClick={onSearch}>
            <Search />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {chats.length ? (
            chats.slice(0, 15).map((chat) => (
              <div key={chat.id} className={cn("group my-px flex min-w-0 items-center gap-px rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground", chatId === chat.id && "bg-sidebar-accent text-sidebar-accent-foreground")}>
                <Link href={`/chat/${encodeURIComponent(chat.id)}`} onClick={onNavigate} title={chat.title} className="min-w-0 flex-1 truncate px-2.5 py-[7px] text-md">
                  {chat.title}
                </Link>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost" size="icon-xs" aria-label={`Options for ${chat.title}`} className="mr-1 size-[26px] opacity-0 group-hover:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100">
                      <MoreHorizontal />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="start">
                    <ChatMenuItems chat={chat} />
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            ))
          ) : (
            <p className="px-3 py-1.5 text-sm leading-[1.7] text-faint">Your next idea starts here.</p>
          )}
          {chats.length > 0 && (
            <Link href="/history" onClick={onNavigate} className={cn("mt-1 flex items-center gap-1.5 rounded-md px-2.5 py-[7px] text-sm leading-[normal] text-faint hover:bg-sidebar-accent hover:text-foreground", path === "/history" && "bg-sidebar-accent text-foreground")}>
              All chats <ChevronRight className="size-3" />
            </Link>
          )}
        </div>
      </div>
      <div className="grid border-t border-sidebar-border pt-2">
        {feedback === "highlighted" && (
          <button
            type="button"
            onClick={openFeedback}
            className="mb-2 flex items-center gap-2.5 rounded-md border border-line-strong bg-secondary px-3 py-2 text-left leading-[normal] text-foreground transition-colors hover:bg-sidebar-accent"
          >
            <MessageSquarePlus className="size-4 shrink-0" />
            <span className="min-w-0">
              <span className="block text-base font-medium">Send feedback</span>
              <span className="mt-0.5 block text-xs text-muted-foreground">Report a problem or share an idea</span>
            </span>
          </button>
        )}
        {feedback === "sidebar" && (
          <button
            type="button"
            onClick={openFeedback}
            className="flex items-center gap-2.5 rounded-md px-3 py-2 text-left text-base leading-[normal] text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground"
          >
            <MessageSquarePlus className="size-4 opacity-80" /> Feedback
          </button>
        )}
        <Link
          href="/settings"
          onClick={onNavigate}
          aria-current={settingsOn ? "page" : undefined}
          className={cn("mb-1.5 flex items-center gap-2.5 rounded-md px-3 py-2 text-base leading-[normal] text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground", settingsOn && "bg-sidebar-accent text-sidebar-accent-foreground")}
        >
          <Settings className="size-4 opacity-80" /> Settings
        </Link>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" className="group/profile flex w-full items-center gap-2.5 border-t border-sidebar-border px-1.5 pt-3 pb-3.5 text-left">
              <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-secondary text-xs font-medium text-text-2">{initials(name)}</span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-md font-medium group-hover/profile:text-foreground">{name}</span>
                <span className="mt-0.5 block truncate text-xs text-muted-foreground">{user.role} · Personal</span>
              </span>
              <ChevronDown className="size-[15px] text-muted-foreground" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-52">
            <DropdownMenuLabel>
              <span className="block">{name}</span>
              <span className="block text-xs font-normal text-muted-foreground">
                {user.username} · {user.role}
              </span>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem asChild>
              <Link href="/settings">
                <Settings /> Settings
              </Link>
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => signOut()}>
              <LogOut /> Sign out
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </aside>
  );
}
