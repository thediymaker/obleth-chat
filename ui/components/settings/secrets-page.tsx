"use client";

// Settings › Secrets: named values the model can use without ever seeing them,
// such as a device's console password. The model is told the names; a tool that
// takes a secret name gets the value where it runs (a device at your computer:
// your own browser). Values are shown once, when you type them, and never again.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Plus } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { usePageChrome } from "@/components/app-shell/chrome";
import { EmptyState, ErrorBox, Loading } from "@/components/app-shell/page";
import { FormActions, FormDialog, FormError, Note, TextField, useConfirm, useSubmit } from "@/components/manage/kit";
import { SectionHeading } from "@/components/settings/settings-frame";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { when } from "@/lib/format";
import { useCapabilities, useSignedIn } from "@/lib/queries";

export interface Secret {
  name: string;
  note: string;
  updatedAt?: string;
}

export const secretsKey = ["secrets"] as const;
export function useSecrets(enabled = true) {
  const signedIn = useSignedIn();
  return useQuery({
    queryKey: secretsKey,
    queryFn: async () => (await api.request<{ secrets?: Secret[] }>("/api/secrets")).secrets ?? [],
    enabled: signedIn && enabled,
  });
}

/** Saves a secret: a new one, or a new value and note for an existing one. */
export async function saveSecret(name: string, value: string, note: string) {
  const body: Record<string, string> = { note };
  if (value) body.value = value;
  return api.request<Secret>(`/api/secrets/${encodeURIComponent(name.trim())}`, { method: "PUT", body });
}

export function SecretDialog({ secret, open, onClose, onSaved }: { secret: Secret | null; open: boolean; onClose: () => void; onSaved?: (s: Secret) => void }) {
  const queries = useQueryClient();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const saved = await saveSecret(String(form.get("secret-name") || ""), String(form.get("secret-value") || ""), String(form.get("secret-note") || "").trim());
    await queries.invalidateQueries({ queryKey: secretsKey });
    toast(secret ? `${saved.name} updated` : `${saved.name} saved`);
    onSaved?.(saved);
    onClose();
  });
  return (
    <FormDialog open={open} onOpenChange={(o) => !o && onClose()} title={secret ? `Change ${secret.name}` : "Add a secret"}>
      <form className="grid gap-4" onSubmit={onSubmit}>
        <TextField id="secret-name" label="Name" required maxLength={40} autoComplete="off" pattern="[A-Za-z0-9][A-Za-z0-9_.\-]*" defaultValue={secret?.name || ""} readOnly={!!secret} help="How tools refer to it, such as beaglebone-password. Letters, digits, dots, dashes and underscores." />
        <TextField id="secret-value" label={secret ? "New value" : "Value"} type="password" autoComplete="new-password" maxLength={4096} required={!secret} help={secret ? "Leave empty to keep the saved value." : "Stored encrypted and never shown again."} />
        <TextField id="secret-note" label="What it is for" maxLength={200} autoComplete="off" defaultValue={secret?.note || ""} help="Shown beside the name, to you only." />
        <FormError message={error} />
        <FormActions>
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={busy}>
            {secret ? "Save" : "Add secret"}
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

export function SecretsPage() {
  usePageChrome("Secrets");
  const capabilities = useCapabilities();
  const enabled = !!capabilities.data?.features?.connections;
  const queries = useQueryClient();
  const { data, error, isLoading, refetch } = useSecrets(enabled);
  const [dialog, setDialog] = useState<null | { secret: Secret | null }>(null);
  const confirm = useConfirm();
  const remove = (s: Secret) =>
    confirm.ask({
      title: `Remove ${s.name}?`,
      text: "Tools set to use it stop working until you add it again.",
      label: "Remove",
      tone: "danger",
      action: async () => {
        await api.request(`/api/secrets/${encodeURIComponent(s.name)}`, { method: "DELETE" });
        await queries.invalidateQueries({ queryKey: secretsKey });
      },
    });
  if (capabilities.data && !enabled)
    return (
      <div>
        <SectionHeading title="Secrets" />
        <EmptyState icon={KeyRound} title="Not set up on this server" text="Secrets go with connections, which an administrator turns on where Chat runs." />
      </div>
    );
  return (
    <div>
      {confirm.element}
      <SectionHeading
        title="Secrets"
        description="Passwords and keys the model can use without ever seeing them. It is told the names; a tool that takes a secret name gets the value where it runs, such as your own browser signing in at a device's console."
        actions={
          <Button size="sm" onClick={() => setDialog({ secret: null })} disabled={!enabled}>
            <Plus /> Add secret
          </Button>
        }
      />
      {error ? (
        <ErrorBox error={error} onRetry={() => refetch()} />
      ) : isLoading ? (
        <Loading />
      ) : !data?.length ? (
        <EmptyState icon={KeyRound} title="No secrets yet." text="Add one here, or from a device's console sign-in under Compute › Devices." action={<Button onClick={() => setDialog({ secret: null })}>Add secret</Button>} />
      ) : (
        <ul className="grid gap-2">
          {data.map((s) => (
            <li key={s.name} className="flex flex-wrap items-center gap-3 rounded-lg border bg-card px-4 py-3">
              <KeyRound className="size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <h3 className="font-mono text-md font-medium">{s.name}</h3>
                <p className="text-sm text-muted-foreground">{[s.note, s.updatedAt ? `updated ${when(s.updatedAt)}` : ""].filter(Boolean).join(" · ") || "No note"}</p>
              </div>
              <Button size="sm" variant="ghost" onClick={() => setDialog({ secret: s })}>
                Change
              </Button>
              <Button size="sm" variant="ghost" onClick={() => remove(s)}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}
      <Note className="mt-4">The value never reaches the model, the gateway or anyone else: it stays encrypted on the server and opens only for your own browser when a tool you allowed needs it.</Note>
      <SecretDialog secret={dialog?.secret ?? null} open={!!dialog} onClose={() => setDialog(null)} />
    </div>
  );
}
