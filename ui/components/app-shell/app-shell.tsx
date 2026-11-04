"use client";

import { PanelLeft, Search } from "lucide-react";
import { usePathname, useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { ChromeProvider, useChrome } from "@/components/app-shell/chrome";
import { Sidebar } from "@/components/app-shell/sidebar";
import { ChatDialogs } from "@/components/chat/chat-dialogs";
import { NEW_CHAT_EVENT, OPEN_SEARCH_EVENT } from "@/components/chat/chat-provider";
import { SearchDialog } from "@/components/chat/search-dialog";
import { StatusBar } from "@/components/chat/status-bar";
import { FocusToggle } from "@/components/chat/chat-view";
import { SiteBanner } from "@/components/site-banner";
import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet";
import { usePreferences } from "@/lib/queries";
import { useSession } from "@/lib/session";
import { cn } from "@/lib/utils";

/** The signed-in frame: sidebar, top bar and the page. */
export function AppShell({ children }: { children: React.ReactNode }) {
  return (
    <ChromeProvider>
      <Frame>{children}</Frame>
    </ChromeProvider>
  );
}

function Frame({ children }: { children: React.ReactNode }) {
  const [drawer, setDrawer] = useState(false);
  const [search, setSearch] = useState(false);
  const router = useRouter();
  const path = usePathname();
  const { title, actions, crumb } = useChrome();
  const { banner } = useSession();
  // Settings' layout choices: focus mode hides the navigation, compact spacing fits more.
  const preferences = (usePreferences().data as { preferences?: { focus?: boolean; density?: string } } | undefined)?.preferences;
  const focus = !!preferences?.focus;
  const density = preferences?.density === "compact" ? "compact" : "comfortable";

  // A new page starts at the top, and the mobile drawer closes.
  useEffect(() => {
    setDrawer(false);
  }, [path]);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setSearch((open) => !open);
      } else if (mod && event.shiftKey && event.key.toLowerCase() === "o") {
        event.preventDefault();
        window.dispatchEvent(new Event(NEW_CHAT_EVENT));
        router.push("/");
      }
    }
    const openSearch = () => setSearch(true);
    window.addEventListener("keydown", onKey);
    window.addEventListener(OPEN_SEARCH_EVENT, openSearch);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener(OPEN_SEARCH_EVENT, openSearch);
    };
  }, [router]);

  return (
    <div className="flex h-dvh overflow-hidden" data-density={density} data-focus={focus || undefined}>
      <a href="#main" className="fixed -top-16 left-2.5 z-50 rounded-md bg-foreground p-3 text-background focus:top-2.5">
        Skip to content
      </a>
      <div className={cn("hidden", !focus && "min-[761px]:flex")}>
        <Sidebar onSearch={() => setSearch(true)} />
      </div>
      <Sheet open={drawer} onOpenChange={setDrawer}>
        <SheetContent side="left" className="w-56 p-0">
          <SheetTitle className="sr-only">Navigation</SheetTitle>
          <Sidebar onNavigate={() => setDrawer(false)} onSearch={() => setSearch(true)} />
        </SheetContent>
      </Sheet>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-4 border-b px-6 leading-[normal] max-[760px]:h-[52px] max-[760px]:gap-2 max-[760px]:px-3">
          <Button variant="ghost" size="icon-sm" className={cn(!focus && "min-[761px]:hidden")} aria-label="Open navigation" onClick={() => setDrawer(true)}>
            <PanelLeft />
          </Button>
          <div className="flex min-w-0 items-center gap-2.5 text-base font-medium max-[760px]:gap-1.5">
            <span className="font-normal text-muted-foreground max-[760px]:hidden">Personal workspace</span>
            <span className="font-normal text-control-border max-[760px]:hidden">/</span>
            <span className="truncate">{crumb || title}</span>
          </div>
          <div className="mx-auto hidden min-w-0 lg:block">
            <SiteBanner banner={banner} />
          </div>
          <div className="ml-auto flex items-center gap-2">
            <ThemeToggle className="hidden sm:inline-flex" />
            <Button variant="ghost" size="icon-sm" aria-label="Search conversations" title="Search conversations (⌘K)" onClick={() => setSearch(true)}>
              <Search />
            </Button>
            {/* A page's own action (a chat's Details), or Focus mode. */}
            {actions ?? <FocusToggle />}
          </div>
        </header>
        <main id="main" tabIndex={-1} className="min-h-0 flex-1 overflow-y-auto outline-none">
          {children}
        </main>
        <StatusBar />
      </div>
      <SearchDialog open={search} onOpenChange={setSearch} />
      <ChatDialogs />
    </div>
  );
}
