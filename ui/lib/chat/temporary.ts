// A temporary chat stays out of Recent, All chats and search, can't be shared, exported,
// moved into a project, branched or saved to the Library, and is deleted when the person
// leaves it. The server also deletes one that has been quiet for a day.

export type ChatMenuAction = "rename-chat" | "pin-chat" | "archive-chat" | "share-chat" | "export-chat" | "delete-chat";
export type ChatMenuIcon = "edit" | "pin" | "archive" | "link" | "download" | "trash";

/** The conversation menu's items; a temporary chat can only be renamed or deleted. */
export function chatMenuItems(c?: { temporary?: boolean; pinned?: boolean; archived?: boolean } | null): [ChatMenuIcon, string, ChatMenuAction][] {
  const all: [ChatMenuIcon, string, ChatMenuAction][] = [
    ["edit", "Rename", "rename-chat"],
    ["pin", c?.pinned ? "Unpin" : "Pin", "pin-chat"],
    ["archive", c?.archived ? "Restore" : "Archive", "archive-chat"],
    ["link", "Share…", "share-chat"],
    ["download", "Export JSON", "export-chat"],
    ["trash", "Delete", "delete-chat"],
  ];
  return c?.temporary ? all.filter(([, , action]) => action === "rename-chat" || action === "delete-chat") : all;
}

export const temporaryNoteText = (started = true) =>
  started ? "It isn’t in your chats or search, and it’s deleted when you leave it." : "It won’t be in your chats or search, and it’s deleted when you leave it.";

/** The temporary chat to delete now that the page shows `route`, if the person left it. */
export const leftTemporary = (open: string | null, route: { page: string; id: string }) => (open && !(route.page === "chat" && route.id === open) ? open : null);
