"use client";

// The dialogs and actions both connection pages use: add (with a kind picker for
// clusters), edit, your own token, on and off, check or refresh tools, and remove.
// `useConnectionManager()` returns the actions and one element that renders them;
// `ConnectionRow` is one connection in a list.

import { useQueryClient } from "@tanstack/react-query";
import { Check, ChevronDown, Lock, RefreshCw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { CheckField, Field, FormActions, FormDialog, FormError, Note, Pill, TextField, useConfirm, useSubmit } from "@/components/manage/kit";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { useCapabilities } from "@/lib/queries";
import { useSession, useUser } from "@/lib/session";
import { cn } from "@/lib/utils";
import { KINDS, TOKEN_HELP, addedMessage, connectionBody, connectionsKey, credentialBody, kindOf, native, plural, type Connection, type ConnectionKind } from "./connections";

type Dialog = null | { type: "pick"; kinds: ConnectionKind[] } | { type: "edit"; connection: Connection | null; kind: ConnectionKind } | { type: "credential"; connection: Connection };

const path = (id: string) => `/api/connections/${encodeURIComponent(id)}`;

export function useConnectionManager({ onChange }: { onChange?: (item: Connection | { id: string; removed: true }) => void } = {}) {
  const queries = useQueryClient();
  const confirm = useConfirm();
  const [dialog, setDialog] = useState<Dialog>(null);
  /** Puts a changed connection in the shared list. */
  const replace = (item: Connection) => {
    queries.setQueryData<Connection[]>(connectionsKey, (old = []) => (old.some((c) => c.id === item.id) ? old.map((c) => (c.id === item.id ? item : c)) : [...old, item]));
    onChange?.(item);
  };
  const run = async (work: () => Promise<void>) => {
    try {
      await work();
    } catch (e) {
      toast((e as Error).message);
    }
  };
  const actions = {
    add: (kind: ConnectionKind = "mcp") => setDialog({ type: "edit", connection: null, kind }),
    /** Choose what to add: Slurm or Kubernetes on the Clusters page. */
    pick: (kinds: ConnectionKind[] = ["slurm", "kubernetes"]) => setDialog({ type: "pick", kinds }),
    edit: (connection: Connection) => setDialog({ type: "edit", connection, kind: kindOf(connection) }),
    credential: (connection: Connection) => setDialog({ type: "credential", connection }),
    removeCredential: (c: Connection) =>
      run(async () => {
        replace(await api.request<Connection>(`${path(c.id)}/credential`, { method: "DELETE" }));
        toast("Your token was removed");
      }),
    toggle: (c: Connection) =>
      run(async () => {
        const body = c.shared ? { optedOut: c.active } : { enabled: !c.active };
        replace(await api.request<Connection>(path(c.id), { method: "PATCH", body }));
      }),
    refresh: (c: Connection) =>
      run(async () => {
        const item = await api.request<Connection>(`${path(c.id)}/refresh`, { method: "POST" });
        replace(item);
        toast(native(item) ? `${item.name} is reachable` : `${plural(item.tools.length, "tool")} found`);
      }),
    remove: (c: Connection) =>
      confirm.ask({
        title: `Remove ${c.name || "this connection"}?`,
        text: `${c.shared ? "It will disappear from everyone’s chats." : "Its tools will no longer be offered in your chats."} ${c.personalTokens ? "Everyone’s saved tokens for it are deleted." : "Its saved token is deleted."}`,
        label: "Remove",
        cancel: "Keep it",
        action: async () => {
          await api.request(path(c.id), { method: "DELETE" });
          queries.setQueryData<Connection[]>(connectionsKey, (old = []) => old.filter((x) => x.id !== c.id));
          onChange?.({ id: c.id, removed: true });
          toast("Connection removed");
        },
      }),
  };
  const close = () => setDialog(null);
  const element = (
    <>
      {confirm.element}
      <FormDialog open={dialog?.type === "pick"} onOpenChange={(o) => !o && close()} title={dialog?.type === "pick" && dialog.kinds.includes("mcp") ? "Add a connection" : "Add a cluster"}>
        <div className="grid gap-2">
          {dialog?.type === "pick" &&
            dialog.kinds.map((k) => {
              const { icon: Icon, label, about } = KINDS[k];
              return (
                <button key={k} type="button" onClick={() => actions.add(k)} className="flex items-start gap-3 rounded-md border p-3 text-left hover:bg-accent">
                  <Icon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                  <span className="grid gap-0.5">
                    <strong className="text-md font-medium">{label}</strong>
                    <small className="text-sm text-muted-foreground">{about}</small>
                  </span>
                </button>
              );
            })}
        </div>
      </FormDialog>
      {dialog?.type === "edit" && (
        <ConnectionDialog
          connection={dialog.connection}
          kind={dialog.kind}
          onClose={close}
          onSaved={(item, created) => {
            replace(item);
            if (!created) {
              close();
              toast("Connection saved");
              return;
            }
            const { message, askToken } = addedMessage(item);
            toast(message);
            if (askToken) setDialog({ type: "credential", connection: item });
            else close();
          }}
        />
      )}
      {dialog?.type === "credential" && (
        <CredentialDialog
          connection={dialog.connection}
          onClose={close}
          onSaved={(item) => {
            replace(item);
            close();
            toast(`${item.name} ready${native(item) ? ` as ${item.credential?.identity || "you"}` : " with your token"}`);
          }}
        />
      )}
    </>
  );
  return { ...actions, element };
}
export type ConnectionManager = ReturnType<typeof useConnectionManager>;

function ConnectionDialog({ connection: c, kind, onClose, onSaved }: { connection: Connection | null; kind: ConnectionKind; onClose: () => void; onSaved: (item: Connection, created: boolean) => void }) {
  const { isAdmin } = useSession();
  const jobOutput = !!useCapabilities().data?.features?.jobOutput;
  const [sharedNew, setSharedNew] = useState(false);
  const k = KINDS[kind];
  const cluster = kind !== "mcp";
  const shared = c ? c.shared : sharedNew;
  const ownToken = !cluster || !c?.shared;
  const sharedNote = cluster ? "Every account can use it with their own token. Only administrators can change it." : "Every account can use it. Only administrators can change it.";
  const { busy, error, setError, onSubmit } = useSubmit(async (form) => {
    setError("Checking…");
    const body = connectionBody(form, c, kind, { admin: isAdmin, jobOutput });
    const item = await api.request<Connection>(c ? path(c.id) : "/api/connections", { method: c ? "PATCH" : "POST", body });
    onSaved(item, !c);
  });
  // A shared server either uses the token entered here for everyone, or each person adds their own.
  const personalBox = !cluster && isAdmin && (!c || c.shared) && (c || sharedNew);
  // A new shared cluster takes no personal fields: each person adds their own later.
  const personalFields = !(cluster && !c && sharedNew);
  const shareBox = !c && isAdmin && <CheckField id="connection-shared" label="Share with everyone" description={sharedNote} checked={sharedNew} onCheckedChange={setSharedNew} />;
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title={c ? `Edit ${c.name}` : `Add a ${k.label}`}>
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField
          id="connection-name"
          label="Name"
          maxLength={40}
          required
          defaultValue={c?.name || ""}
          placeholder={kind === "mcp" ? "Research tools" : kind === "slurm" ? "HPC cluster" : "Lab cluster"}
          help="Tools appear to the model under this name."
        />
        <TextField
          id="connection-url"
          label={kind === "mcp" ? "Server URL" : kind === "slurm" ? "slurmrestd URL" : "API server URL"}
          type="url"
          maxLength={2048}
          required
          defaultValue={c?.url || ""}
          placeholder={k.url}
          help={k.urlHelp}
        />
        {kind === "slurm" && (
          <TextField
            id="connection-api-version"
            label="REST API version"
            maxLength={10}
            autoComplete="off"
            placeholder="Newest offered"
            pattern="v0\.0\.[0-9]{2,3}"
            defaultValue={c?.apiVersion || ""}
            help="Leave empty to use the newest one the server offers, from v0.0.40 to v0.0.45. Set it, such as v0.0.46, when the server runs a newer one."
          />
        )}
        {kind === "slurm" && jobOutput && (
          <CheckField
            id="connection-return-output"
            label="Bring job output back"
            defaultChecked={!!c?.returnOutput}
            description="Each job submitted from Chat sends the last 1 MiB of its output to Chat’s file storage when it ends, so the model can read it. The compute nodes must be able to reach that storage."
          />
        )}
        {kind === "kubernetes" && (
          <>
            <Field id="connection-ca" label="Cluster CA certificate" help="PEM, from certificate-authority-data in your kubeconfig (base64-decoded). Leave empty if the server has a public certificate.">
              <Textarea id="connection-ca" name="connection-ca" rows={3} maxLength={16384} className="font-mono text-sm" placeholder={c?.hasCa ? "Saved. Leave empty to keep it" : "-----BEGIN CERTIFICATE-----"} />
            </Field>
            {c?.hasCa && <CheckField id="connection-clear-ca" label="Remove the saved certificate" />}
          </>
        )}
        {cluster && shareBox}
        {personalFields && kind === "slurm" && !shared && (
          <TextField id="connection-user" label="Slurm user name" maxLength={63} autoComplete="off" placeholder="jdoe" defaultValue={c?.user || ""} help="The account the token belongs to. Jobs run as this user." />
        )}
        {personalFields && kind === "kubernetes" && !shared && (
          <TextField
            id="connection-namespace"
            label="Namespace"
            maxLength={63}
            autoComplete="off"
            placeholder="From the token"
            defaultValue={c?.namespace || ""}
            help="Where tools work when the model names none. Empty: a service account’s own namespace, or default."
          />
        )}
        {ownToken && personalFields && (
          <>
            <TextField
              id="connection-token"
              label="Access token"
              type="password"
              autoComplete="off"
              maxLength={4096}
              placeholder={c?.hasToken ? "Saved. Leave empty to keep it" : cluster ? "" : "Optional"}
              help={
                c?.hasToken
                  ? "Enter a new token to replace the saved one."
                  : cluster
                    ? `${TOKEN_HELP[kind]} Stored encrypted and never shown again.`
                    : "Sent as a Bearer token. Stored encrypted and never shown again."
              }
            />
            {c?.hasToken && !cluster && <CheckField id="connection-clear-token" label="Remove the saved token" />}
          </>
        )}
        <CheckField
          id="connection-confirm"
          label="Ask before tools that may change things"
          defaultChecked={c ? c.confirmActions : true}
          description={cluster ? "Submitting, cancelling, applying and deleting. Reading always runs without asking." : "Read-only tools always run without asking."}
        />
        {!cluster && shareBox}
        {personalBox && (
          <CheckField
            id="connection-personal-tokens"
            label="Each person adds their own token"
            defaultChecked={!!c?.personalTokens}
            description="For servers where tokens are personal. Calls then act as that person. The token above is used only to list the server’s tools."
          />
        )}
        {c?.shared && <CheckField id="connection-enabled" label="Available to everyone" defaultChecked={c.enabled} />}
        {error === "Checking…" ? (
          <p role="status" className="text-sm text-muted-foreground">
            Checking…
          </p>
        ) : (
          <FormError message={error} />
        )}
        <FormActions>
          <Button type="submit" disabled={busy}>
            {c ? "Save" : "Connect"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

function CredentialDialog({ connection: c, onClose, onSaved }: { connection: Connection; onClose: () => void; onSaved: (item: Connection) => void }) {
  const user = useUser();
  const kind = kindOf(c);
  const mine = c.credential || {};
  const { busy, error, setError, onSubmit } = useSubmit(async (form) => {
    setError("Checking…");
    onSaved(await api.request<Connection>(`${path(c.id)}/credential`, { method: "PUT", body: credentialBody(form, c) }));
  });
  return (
    <FormDialog open onOpenChange={(o) => !o && onClose()} title={`Your token for ${c.name}`}>
      <form onSubmit={onSubmit} className="grid gap-4">
        <Note>Tools on this {kind === "mcp" ? "server" : "cluster"} act as you, with your permissions. The token is checked now, stored encrypted and never shown again.</Note>
        {kind === "slurm" && <TextField id="credential-user" label="Slurm user name" maxLength={63} autoComplete="off" required defaultValue={mine.user || user.username || ""} help="The account the token belongs to." />}
        {kind === "kubernetes" && (
          <TextField
            id="credential-namespace"
            label="Namespace"
            maxLength={63}
            autoComplete="off"
            placeholder="From the token"
            defaultValue={mine.namespace === "default" ? "" : mine.namespace || ""}
            help="Where tools work when the model names none. Empty: a service account’s own namespace, or default."
          />
        )}
        <TextField id="credential-token" label="Token" type="password" autoComplete="off" maxLength={4096} required help={TOKEN_HELP[kind]} />
        {error === "Checking…" ? (
          <p role="status" className="text-sm text-muted-foreground">
            Checking…
          </p>
        ) : (
          <FormError message={error} />
        )}
        <FormActions>
          <Button type="submit" disabled={busy}>
            {c.credential ? "Replace token" : "Save token"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

/** A person's own token on a shared cluster, or on a shared server set to personal tokens. */
function Credential({ c, manager }: { c: Connection; manager: ConnectionManager }) {
  if (!c.personalTokens) return null;
  const buttons = (
    <div className="flex flex-wrap gap-1.5">
      <Button size="xs" variant="ghost" onClick={() => manager.credential(c)}>
        Replace token
      </Button>
      <Button size="xs" variant="ghost" onClick={() => manager.removeCredential(c)}>
        Remove my token
      </Button>
    </div>
  );
  if (!c.credential)
    return c.enabled ? (
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-amber/40 bg-amber/10 px-3 py-2 text-sm">
        <p className="inline-flex items-center gap-2">
          <Lock className="size-3.5" />
          Add your own token to use this {native(c) ? "cluster" : "server"}. Tools then act as you.
        </p>
        <Button size="xs" onClick={() => manager.credential(c)}>
          Add your token
        </Button>
      </div>
    ) : null;
  const who = !native(c) ? (
    <>
      Using <strong>your own token</strong>
    </>
  ) : (
    <>
      Acting as <strong>{c.credential.identity || c.credential.user || "you"}</strong> · {kindOf(c) === "slurm" ? `Slurm user ${c.credential.user}` : `namespace ${c.credential.namespace}`}
    </>
  );
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border px-3 py-2 text-sm">
      <p className="inline-flex items-center gap-2">
        <Check className="size-3.5 text-success" />
        <span>{who}</span>
      </p>
      {buttons}
    </div>
  );
}

function Tools({ c }: { c: Connection }) {
  const [open, setOpen] = useState(false);
  const list = c.tools || [];
  if (!list.length) return null;
  return (
    <div className="grid gap-2">
      <button type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)} className="inline-flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground">
        <ChevronDown className={cn("size-3.5", open && "rotate-180")} /> {plural(list.length, "tool")}
      </button>
      {open && (
        <ul className="grid gap-2 border-l pl-3">
          {list.map((t) => (
            <li key={t.name} className="grid gap-0.5">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <code className="font-mono text-sm">{t.title || t.name}</code>
                {t.readOnly ? <Pill>Read-only</Pill> : <Pill tone="amber">{c.confirmActions ? "Asks first" : "Runs without asking"}</Pill>}
              </div>
              {t.description && <p className="text-sm text-muted-foreground">{t.description}</p>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function ConnectionRow({ c, manager }: { c: Connection; manager: ConnectionManager }) {
  const kind = KINDS[kindOf(c)];
  const Icon = kind.icon;
  const changes = (c.tools || []).filter((t) => !t.readOnly).length;
  const who = !c.shared && native(c) && c.identity ? `as ${c.identity}` : "";
  const facts = [
    !native(c) && c.serverName,
    who,
    c.apiVersion && `REST API ${c.apiVersion}`,
    c.returnOutput && "job output comes back",
    changes ? `${changes} can make changes` : "all read-only",
    c.checkedAt && `checked ${when(c.checkedAt)}`,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div className={cn("grid gap-3 rounded-md border p-3", !c.active && "opacity-70")}>
      <div className="flex items-start gap-3">
        <Icon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 grow">
          <h3 className="flex flex-wrap items-center gap-1.5 text-md font-medium">
            {c.name} {native(c) && <Pill>{kind.label.split(" ")[0]}</Pill>} {c.shared && <Pill>Shared</Pill>} {c.shared && !c.enabled && <Pill tone="amber">Off for everyone</Pill>}
          </h3>
          <p className="truncate font-mono text-sm text-muted-foreground" title={c.url}>
            {c.url}
          </p>
          <small className="text-xs text-muted-foreground">{facts}</small>
        </div>
        {c.usable !== false && (
          <Button size="xs" variant={c.active ? "outline" : "default"} aria-label={`${c.active ? "Stop using" : "Use"} ${c.name} in my chats`} onClick={() => manager.toggle(c)}>
            {c.active ? "On" : "Off"}
          </Button>
        )}
      </div>
      <Credential c={c} manager={manager} />
      <Tools c={c} />
      {c.canManage && (
        <div className="flex flex-wrap gap-1.5">
          <Button size="xs" variant="ghost" onClick={() => manager.refresh(c)}>
            <RefreshCw /> {native(c) ? "Check" : "Refresh tools"}
          </Button>
          <Button size="xs" variant="outline" onClick={() => manager.edit(c)}>
            Edit
          </Button>
          <Button size="xs" variant="outline" onClick={() => manager.remove(c)}>
            Remove
          </Button>
        </div>
      )}
    </div>
  );
}
