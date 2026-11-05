// Administration › Activity: each event as one plain sentence with names in bold, its
// kind, and its stored details. Conversation content never appears here.

import { Box, Clock, LayoutGrid, Link2, LogIn, Settings, Trash2, Users, Zap } from "lucide-react";
import { titleCase } from "@/components/manage/kit";

export const AUDIT_KINDS = [
  ["accounts", Users, "Accounts"],
  ["sign_in", LogIn, "Sign-in and tokens"],
  ["groups", LayoutGrid, "Groups"],
  ["settings", Settings, "Settings"],
  ["connections", Link2, "Connections"],
  ["mcp", Zap, "MCP"],
  ["apps", Box, "Apps"],
  ["cleanup", Trash2, "Automatic cleanup"],
] as const;
export const kindInfo = (kind: string) => {
  const k = AUDIT_KINDS.find(([id]) => id === kind);
  return k ? { icon: k[1], name: k[2] } : { icon: Clock, name: "Other" };
};

export interface AuditEvent {
  id?: string;
  action: string;
  kind: string;
  createdAt: string;
  actorName?: string | null;
  targetName?: string | null;
  memberName?: string | null;
  targetType?: string | null;
  targetId?: string | null;
  metadata?: Record<string, unknown> | null;
}

/** A sentence as text and bold parts. */
export type Part = string | { bold: string };

const quoted = (name: unknown) => (name ? `“${String(name)}”` : "");

export function auditSentence(e: AuditEvent): Part[] {
  const m = (e.metadata || {}) as Record<string, unknown>;
  const bold = (value: string | null | undefined, fallback: string): Part => (value ? { bold: value } : fallback);
  const actor = bold(e.actorName, "Chat");
  const target = (fallback: string) => bold(e.targetName, fallback);
  const member = bold(e.memberName, "someone");
  // "created the group" with no name drops the trailing space, as .trim() did.
  const trim = (parts: Part[]): Part[] => {
    const last = parts.at(-1);
    if (last === "") parts.pop();
    const prev = parts.at(-1);
    if (typeof prev === "string") parts[parts.length - 1] = prev.trimEnd();
    return parts;
  };
  const removed = (
    [
      ["conversations", "inactive conversation"],
      ["unusedFiles", "unused file"],
      ["auditEvents", "old activity entry", "old activity entries"],
      ["expiredSessions", "expired sign-in"],
      ["expiredAccessTokens", "expired sign-in link"],
    ] as [string, string, string?][]
  )
    .filter(([key]) => Number(m[key]) > 0)
    .map(([key, one, many]) => `${Number(m[key]).toLocaleString()} ${Number(m[key]) === 1 ? one : many || `${one}s`}`);
  const models = Array.isArray(m.models) ? m.models : [];
  const sentences: Record<string, () => Part[]> = {
    user_created: () => [actor, " created the account ", target("a person")],
    user_invited: () => [actor, " invited ", target("a person")],
    user_updated: () => (m.enabled === false ? [actor, " turned off the account ", target("a person")] : [actor, " updated the account ", target("a person")]),
    profile_updated: () => [actor, " updated their profile"],
    password_changed: () => [actor, " changed their password"],
    password_reset: () => [actor, " reset the password for ", target("a person")],
    oidc_linked: () => [target("An account"), " was linked to single sign-on"],
    access_link_created: () => [actor, " made a sign-in link for ", target("a person")],
    access_link_redeemed: () => [target("Someone"), " used a sign-in link"],
    session_revoked: () => (m.current ? [actor, " signed out"] : [actor, " signed out one of their sessions"]),
    other_sessions_revoked: () => [actor, " signed out their other sessions"],
    user_sessions_revoked: () => [actor, " signed ", target("a person"), " out everywhere"],
    sign_in_ended: () => [member, "’s single sign-on session ended"],
    api_token_created: () => trim([actor, " created the access token ", target("")]),
    api_token_revoked: () => [actor, " revoked an access token"],
    group_created: () => trim([actor, " created the group ", target("")]),
    group_updated: () => trim([actor, " updated the group ", target("")]),
    group_deleted: () => [actor, " deleted a group"],
    group_member_added: () => [actor, " added ", member, " to ", target("a group")],
    group_member_removed: () => [actor, " removed ", member, " from ", target("a group")],
    oidc_groups_reconciled: () => ["Single sign-on updated the groups of ", target("a person")],
    "model.update": () => trim([actor, " changed the model ", target("")]),
    "models.short_list": () => (models.length ? [actor, ` set the short list of models to ${models.length}`] : [actor, " cleared the short list of models"]),
    "limits.update": () => (e.targetType === "user" ? [actor, " changed the limits for ", target("a person")] : [actor, " changed the workspace limits"]),
    "decisions.update": () => [actor, " changed the decisions settings"],
    "insights.update": () => [actor, " changed the usage insights settings"],
    "app.published": () => [actor, ` published the app ${quoted(m.name)} as v${Number(m.version) || ""}`],
    "app.unpublished": () => trim([actor, ` unpublished the app ${quoted(m.name)}`]),
    "app.deleted": () => trim([actor, ` deleted the app ${quoted(m.name)}`]),
    "app.key_created": () => [actor, ` made a key ${quoted(m.name)} for an app`],
    "app.key_revoked": () => trim([actor, ` revoked an app key ${quoted(m.name)}`]),
    "workspaces.stop": () => [actor, " stopped ", target("a person"), `’s ${m.kind === "browser" ? "browser" : "workspace"}`],
    "insights.report": () => [actor, " asked to find the workstreams again"],
    "banner.update": () => [actor, " changed the banner"],
    "knowledge.embedding.update": () => [actor, " changed the knowledge search model"],
    "retention.update": () => [actor, " changed data retention"],
    "retention.cleanup": () => [removed.length ? `Automatic cleanup removed ${removed.join(", ")}` : "Automatic cleanup ran"],
    "connection.create": () => trim([actor, " added the shared connection ", target("")]),
    "connection.update": () => trim([actor, " changed the shared connection ", target("")]),
    "connection.delete": () => trim([actor, " removed the shared connection ", target("")]),
    mcp_chat_started: () => [actor, " started a chat over MCP"],
    mcp_message_sent: () => [actor, " sent a message over MCP"],
    mcp_chat_stopped: () => [actor, " stopped a reply over MCP"],
    mcp_chat_deleted: () => [actor, " deleted a chat over MCP"],
  };
  return sentences[e.action]?.() || [actor, ` · ${titleCase(e.action)}`];
}

/** The sentence as plain text, for tests and titles. */
export const sentenceText = (parts: Part[]) => parts.map((p) => (typeof p === "string" ? p : p.bold)).join("");

/** The event's stored details as label and value rows. */
export function auditDetails(e: AuditEvent): [string, string][] {
  const label = (key: string) => titleCase(key.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase());
  const value = (v: unknown): string =>
    v === true
      ? "Yes"
      : v === false
        ? "No"
        : v == null
          ? "—"
          : Array.isArray(v)
            ? v.map((x) => (typeof x === "object" ? JSON.stringify(x) : String(x))).join(", ") || "None"
            : typeof v === "object"
              ? JSON.stringify(v)
              : String(v);
  const rows: [string, string][] = Object.entries(e.metadata || {})
    .filter(([key]) => !(e.action === "retention.cleanup" && key === "at"))
    .map(([key, v]) => [label(key), value(v)]);
  if (e.targetId) rows.push([`${titleCase(e.targetType)} ID`, e.targetId]);
  return rows;
}

/** "Today", "Yesterday", or "Mon, Nov 2". */
export function auditDay(value: string, now = new Date()) {
  const d = new Date(value);
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const days = Math.round((today.getTime() - new Date(d).setHours(0, 0, 0, 0)) / 864e5);
  return days === 0
    ? "Today"
    : days === 1
      ? "Yesterday"
      : d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric", ...(d.getFullYear() === today.getFullYear() ? {} : { year: "numeric" }) });
}
export const auditTime = (value: string) => new Date(value).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
