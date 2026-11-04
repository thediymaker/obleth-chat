"use client";

// Selecting text in a reply offers Save, Ask about this and Copy. Save needs the
// Library; Ask isn't offered in someone else's chat.

import { Bookmark, Copy, MessageSquare } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useChatProjectId } from "@/components/projects/chat-project";
import { useChat } from "./chat-provider";

export function SelectionBar() {
  const store = useChat();
  const [pick, setPick] = useState<{ text: string; index: number; left: number; top: number } | null>(null);
  const bar = useRef<HTMLDivElement>(null);
  const project = useChatProjectId(store.shared ? null : (store.active?.id ?? null));
  const library = store.feature("knowledge");
  useEffect(() => {
    const show = () => {
      const sel = window.getSelection(),
        text = sel?.toString().trim();
      const node = sel?.rangeCount ? sel.getRangeAt(0).commonAncestorContainer : null;
      const el = (node?.nodeType === 1 ? (node as Element) : node?.parentElement)?.closest?.("[data-message] [data-answer]");
      const article = el?.closest("[data-message]");
      if (!text || text.length < 3 || !el || !article || article.getAttribute("data-role") !== "assistant") {
        setPick(null);
        return;
      }
      const rect = sel!.getRangeAt(0).getBoundingClientRect();
      setPick({ text: text.slice(0, 20000), index: Number(article.getAttribute("data-message")), left: rect.left + rect.width / 2, top: rect.top });
    };
    const later = () => setTimeout(show, 0);
    const onKey = (e: KeyboardEvent) => {
      if (e.shiftKey || e.key === "Escape") later();
    };
    const onDown = (e: MouseEvent) => {
      if (!bar.current?.contains(e.target as Node)) setPick(null);
    };
    document.addEventListener("mouseup", later);
    document.addEventListener("keyup", onKey);
    document.addEventListener("mousedown", onDown);
    return () => {
      document.removeEventListener("mouseup", later);
      document.removeEventListener("keyup", onKey);
      document.removeEventListener("mousedown", onDown);
    };
  }, []);
  if (!pick || !library) return null;
  const width = 280;
  const done = () => {
    setPick(null);
    window.getSelection()?.removeAllRanges();
  };
  return (
    <div
      ref={bar}
      role="toolbar"
      aria-label="Selected text"
      onMouseDown={(e) => e.preventDefault()}
      className="fixed z-50 flex gap-0.5 rounded-lg border bg-popover p-1 text-sm shadow-lg"
      style={{ left: Math.max(8, Math.min(window.innerWidth - width - 8, pick.left - width / 2)), top: Math.max(8, pick.top - 44) }}
    >
      {library && (
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 hover:bg-accent"
          onClick={() => {
            store.saveFromChat({ kind: "excerpt", body: pick.text, title: pick.text.split(/[.\n]/)[0]!.slice(0, 80) }, pick.index, project);
            done();
          }}
        >
          <Bookmark className="size-3.5" /> Save
        </button>
      )}
      {!store.shared && (
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 hover:bg-accent"
          onClick={() => {
            store.quote(pick.text);
            done();
          }}
        >
          <MessageSquare className="size-3.5" /> Ask about this
        </button>
      )}
      <button
        type="button"
        className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 hover:bg-accent"
        onClick={() => {
          store.copy(pick.text);
          done();
        }}
      >
        <Copy className="size-3.5" /> Copy
      </button>
    </div>
  );
}
