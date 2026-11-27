// Addresses written with a hash (#chat/<id>, #activate/<token>,
// #settings/admin/users…), as in bookmarks and invitations sent before Chat used
// paths. This turns such a hash into its path, or null when there is nothing to move.

const MOVED: Record<string, string> = {
  playground: "",
  classroom: "groups",
  compute: "workspaces",
  system: "settings",
  admin: "settings/admin",
};

export function hashPath(hash: string): string | null {
  if (!hash.startsWith("#") || hash.length < 2) return null;
  let parts: string[];
  try {
    parts = hash.slice(1).split("/").filter(Boolean).map(decodeURIComponent);
  } catch {
    // Not an address Chat wrote (a stray % in it): nothing to move.
    return null;
  }
  if (!parts.length) return null;
  let [page, ...rest] = parts;
  // Only MOVED's own names: every object also answers to "toString" and the like.
  if (page !== undefined && Object.hasOwn(MOVED, page)) {
    const moved = MOVED[page]!;
    parts.splice(0, 1, ...(moved ? moved.split("/") : []));
    [page, ...rest] = parts;
  }
  if (page === undefined) return "/";
  if (page === "chat") return rest[0] ? `/chat/${encodeURIComponent(rest[0])}` : "/";
  if (page === "settings") {
    if (rest[0] === "infrastructure") return "/settings";
    if (rest[0] === "connections") return "/settings/mcp";
  }
  return `/${[page, ...rest].map(encodeURIComponent).join("/")}`;
}
