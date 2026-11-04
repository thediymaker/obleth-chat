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
  const parts = hash.slice(1).split("/").filter(Boolean).map(decodeURIComponent);
  if (!parts.length) return null;
  let [page, ...rest] = parts;
  if (page in MOVED) {
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
