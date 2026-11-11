"use client";

// Connections the model can use in chats: MCP servers (Settings › MCP servers) and
// Slurm or Kubernetes clusters (the Clusters page). Both pages share these types,
// the list query, and the request bodies their dialogs send.

import { useQuery } from "@tanstack/react-query";
import { Box, Cpu, Link2, Server } from "lucide-react";
import { api } from "@/lib/api";
import { useCapabilities } from "@/lib/queries";
import { useSession } from "@/lib/session";

export type ConnectionKind = "mcp" | "slurm" | "kubernetes" | "device";
export interface ConnectionTool {
  name: string;
  title?: string;
  description?: string;
  readOnly?: boolean;
}
export interface Connection {
  id: string;
  kind: ConnectionKind;
  name: string;
  url: string;
  shared: boolean;
  enabled: boolean;
  active: boolean;
  usable?: boolean;
  canManage?: boolean;
  confirmActions: boolean;
  personalTokens?: boolean;
  hasToken?: boolean;
  hasCa?: boolean;
  credential?: { identity?: string; user?: string; namespace?: string } | null;
  identity?: string;
  serverName?: string;
  apiVersion?: string;
  returnOutput?: boolean;
  checkedAt?: string;
  user?: string;
  namespace?: string;
  /** A device: its transport and the USB ids and label the browser read off the port. */
  transport?: string;
  vendorId?: number | null;
  productId?: number | null;
  label?: string | null;
  tools: ConnectionTool[];
}

export const KINDS: Record<ConnectionKind, { label: string; icon: typeof Link2; url: string; urlHelp: string; about: string }> = {
  mcp: { label: "MCP server", icon: Link2, url: "https://example.com/mcp", urlHelp: "A Streamable HTTP MCP endpoint.", about: "Tools from any server that speaks the Model Context Protocol." },
  slurm: {
    label: "Slurm cluster",
    icon: Server,
    url: "https://slurm.example.edu:6820",
    urlHelp: "The slurmrestd address, without /slurm/v0.0.x.",
    about: "Look at partitions and jobs, and submit or cancel jobs as your Slurm user.",
  },
  kubernetes: {
    label: "Kubernetes cluster",
    icon: Box,
    url: "https://k8s.example.edu:6443",
    urlHelp: "The API server address from your kubeconfig.",
    about: "Read and change resources with your own token, so the cluster’s RBAC decides what is allowed.",
  },
  device: {
    label: "Device",
    icon: Cpu,
    url: "",
    urlHelp: "",
    about: "A board or instrument plugged into your computer, reached through this browser: serial monitor, commands, identify and flash.",
  },
};
export const kindOf = (c: { kind?: string } | null | undefined): ConnectionKind => (c?.kind && c.kind in KINDS ? (c.kind as ConnectionKind) : "mcp");
export const native = (c: { kind?: string } | null | undefined) => kindOf(c) === "slurm" || kindOf(c) === "kubernetes";
export const isDevice = (c: { kind?: string } | null | undefined) => kindOf(c) === "device";
export const TOKEN_HELP: Record<ConnectionKind, string> = {
  device: "",
  slurm: "On a login node run scontrol token lifespan=2592000 and paste the part after SLURM_JWT=. It acts as the Slurm user named above.",
  kubernetes: "For a service account run kubectl create token NAME -n NAMESPACE --duration=720h, or use the token from your kubeconfig.",
  mcp: "Your personal token or API key for this server. It is sent as a Bearer token.",
};
export const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

export const connectionsKey = ["connections"] as const;
/** Every connection this account sees: its own, and shared ones. Nothing is asked while connections are off. */
export function useConnections() {
  const signedIn = useSession().phase === "ready";
  const on = !!useCapabilities().data?.features?.connections;
  return useQuery({
    queryKey: connectionsKey,
    queryFn: async () => (await api.request<{ connections?: Connection[] }>("/api/connections")).connections ?? [],
    enabled: signedIn && on,
  });
}

const text = (form: FormData, name: string) => String(form.get(name) ?? "").trim();
const checked = (form: FormData, name: string) => form.get(name) === "on";

/** What the add or edit dialog sends (connections.js submit). */
export function connectionBody(form: FormData, c: Connection | null, kind: ConnectionKind, { admin, jobOutput }: { admin: boolean; jobOutput: boolean }) {
  const token = text(form, "connection-token");
  const shared = c ? c.shared : checked(form, "connection-shared");
  const body: Record<string, unknown> = { name: text(form, "connection-name"), url: text(form, "connection-url"), confirmActions: checked(form, "connection-confirm") };
  if (!c) body.kind = kind;
  if (token && !(shared && kind !== "mcp")) body.token = token;
  else if (checked(form, "connection-clear-token")) body.token = "";
  if (!c && shared) body.shared = true;
  if (c?.shared) body.enabled = checked(form, "connection-enabled");
  if (kind === "mcp" && shared && admin) {
    const personal = checked(form, "connection-personal-tokens");
    if (!c || personal !== !!c.personalTokens) body.options = personal ? { personalTokens: true } : {};
  }
  if (kind !== "mcp") {
    const options: Record<string, unknown> = {};
    const ca = text(form, "connection-ca");
    if (kind === "kubernetes" && (ca || checked(form, "connection-clear-ca"))) options.ca = ca;
    if (!shared && kind === "slurm") options.user = text(form, "connection-user");
    if (!shared && kind === "kubernetes") options.namespace = text(form, "connection-namespace");
    if (kind === "slurm") options.apiVersion = text(form, "connection-api-version");
    if (kind === "slurm" && jobOutput) options.returnOutput = checked(form, "connection-return-output");
    const changed =
      !c ||
      "ca" in options ||
      (options.user ?? c.user) !== c.user ||
      (options.namespace ?? c.namespace) !== c.namespace ||
      (kind === "slurm" && options.apiVersion !== (c.apiVersion || "")) ||
      ("returnOutput" in options && options.returnOutput !== !!c.returnOutput);
    if (changed) body.options = options;
  }
  return body;
}

/** What "Your token" sends: the token, and the Slurm user or Kubernetes namespace. */
export function credentialBody(form: FormData, c: Connection) {
  const body: Record<string, string> = { token: text(form, "credential-token") };
  if (kindOf(c) === "slurm") body.user = text(form, "credential-user");
  else if (kindOf(c) === "kubernetes") body.namespace = text(form, "credential-namespace");
  return body;
}

/** The toast after adding a connection, and whether to ask for the person's own token next. */
export function addedMessage(item: Connection): { message: string; askToken: boolean } {
  if (kindOf(item) === "mcp" && item.personalTokens) return { message: `${item.name} shared. Add your own token to use it.`, askToken: true };
  if (kindOf(item) === "mcp") return { message: `${item.name} connected with ${plural(item.tools.length, "tool")}`, askToken: false };
  if (item.shared) return { message: `${item.name} shared. Add your own token to use it.`, askToken: true };
  return { message: `${item.name} connected as ${item.identity || "you"}`, askToken: false };
}
