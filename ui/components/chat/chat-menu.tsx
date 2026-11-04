"use client";

// The options menu of one chat, shared by the sidebar's Recent list and the chat
// page: rename, pin,
// archive, share, export and delete. A temporary chat can only be renamed or deleted.

import { Archive, Download, Link2, Pencil, Pin, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { exportChats } from "@/lib/chat/history";
import { chatMenuItems, type ChatMenuIcon } from "@/lib/chat/temporary";
import type { ConversationSummary } from "@/lib/queries";
import { useChat } from "./chat-provider";

const ICONS: Record<ChatMenuIcon, typeof Pencil> = { edit: Pencil, pin: Pin, archive: Archive, link: Link2, download: Download, trash: Trash2 };

export function ChatMenuItems({ chat }: { chat: Pick<ConversationSummary, "id" | "title"> & { pinned?: boolean; archived?: boolean; temporary?: boolean } }) {
  const store = useChat();
  // The open chat knows more (temporary, archived) than its Recent row.
  const c = store.active?.id === chat.id ? { ...chat, ...store.active } : chat;
  const act = {
    "rename-chat": () => store.openDialog({ kind: "rename", id: c.id, title: c.title }),
    "pin-chat": () => store.run(() => store.update(c.id, { pinned: !c.pinned })),
    "archive-chat": () => store.run(() => store.update(c.id, { archived: !c.archived })),
    "share-chat": () => store.openDialog({ kind: "share", id: c.id }),
    "export-chat": () => exportChats([c.id]).catch((e: Error) => toast(e.message)),
    "delete-chat": () => store.openDialog({ kind: "delete", id: c.id }),
  };
  return (
    <>
      {chatMenuItems(c).map(([icon, label, action]) => {
        const Icon = ICONS[icon];
        return (
          <span key={action} className="contents">
            {action === "delete-chat" && <DropdownMenuSeparator />}
            <DropdownMenuItem
              variant={action === "delete-chat" ? "destructive" : "default"}
              disabled={action === "delete-chat" && store.streaming && store.streamChatId === c.id}
              onSelect={act[action]}
            >
              <Icon /> {label}
            </DropdownMenuItem>
          </span>
        );
      })}
    </>
  );
}
