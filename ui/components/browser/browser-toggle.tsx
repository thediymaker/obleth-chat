"use client";

// The composer dock's Browser button: a new chat marks the browser for the next chat;
// in a chat the first press turns the browser on, and later presses show or hide its
// tab. The browser goes off from the Power button on its tab.

import { Globe } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { useChat } from "@/components/chat/chat-provider";
import { cn } from "@/lib/utils";

export function BrowserToggle() {
  const store = useChat();
  if (!store.browser.enabled()) return null;
  const id = store.route.id;
  const on = id ? store.browser.on(id) : store.browserDraft;
  return (
    <Button
      type="button"
      variant="ghost"
      size="xs"
      aria-pressed={on}
      className={cn("rounded-full", on && "bg-secondary text-foreground", on && !!id && store.showing(id, "browser") && "ring-1 ring-line-strong")}
      title={on ? "The model can search the web and open pages in a browser you can watch" : "Let the model search the web and open pages in a browser you can watch"}
      disabled={store.streaming}
      onClick={() =>
        store.run(async () => {
          if (!id) return store.toggleBrowser();
          if (!store.browser.on(id)) {
            await store.browser.setOn(id, true);
            toast("Browser on. Ask for something to look up.");
          } else if (store.showing(id, "browser")) {
            // A second press hides the tab: back to the workspace's preview, or no pane.
            store.browser.dismiss(id);
            if (store.workspace.has(id)) store.showTab(id, "preview");
            else store.closePane(id);
          } else store.browser.show(id);
        })
      }
    >
      <Globe />
      <span className="hidden sm:inline">Browser</span>
    </Button>
  );
}
