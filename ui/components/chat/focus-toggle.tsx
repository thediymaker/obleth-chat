"use client";

// The header's Focus mode switch. It lives apart from the chat view so the app shell,
// which shows it on every page, does not load the chat renderer with it.

import { PanelLeftClose } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useChatSelect, useChatStore } from "./chat-provider";

export function FocusToggle() {
  const store = useChatStore();
  const focus = useChatSelect((s) => s.focus);
  return (
    <Button variant="ghost" size="sm" className="max-[760px]:hidden" aria-pressed={focus} onClick={() => store.run(() => store.savePreference({ focus: !store.focus }))}>
      <PanelLeftClose /> <span className="max-[1000px]:hidden">Focus mode</span>
    </Button>
  );
}
