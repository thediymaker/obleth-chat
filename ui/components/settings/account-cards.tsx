"use client";

// Security (password, this session, signed-in devices) and API access (personal
// tokens for an MCP client such as Claude Code).

import { useQueryClient } from "@tanstack/react-query";
import { Lock, Plus, RefreshCw, Shield } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { ErrorBox } from "@/components/app-shell/page";
import { Button } from "@/components/ui/button";
import { CheckField, CopyField, FormActions, FormDialog, FormError, Note, Pill, Section, SelectField, TextField, useConfirm, useSubmit } from "@/components/manage/kit";
import { api, ApiError } from "@/lib/api";
import { when } from "@/lib/format";
import { useSession, useUser } from "@/lib/session";

export interface DeviceSession {
  id: string;
  current?: boolean;
  userAgent?: string;
  createdAt?: string;
  expiresAt?: string;
}
export interface ApiToken {
  id: string;
  name: string;
  prefix: string;
  scopes: string[];
  createdAt?: string;
  expiresAt?: string;
  lastUsedAt?: string | null;
}

/** The server's answer when a token needs a more recent sign-in. */
export const STALE_SIGN_IN = "Sign in again to create a token";

function Line({ icon: Icon, title, text, action }: { icon: typeof Lock; title: string; text: string; action: React.ReactNode }) {
  return (
    <div className="flex items-center gap-3">
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0 grow">
        <h3 className="text-md font-medium">{title}</h3>
        <p className="text-sm text-muted-foreground">{text}</p>
      </div>
      {action}
    </div>
  );
}

export function PasswordDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const user = useUser();
  const { adopt } = useSession();
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    const result = await api.request<{ csrfToken?: string }>("/api/auth/password", {
      method: "POST",
      body: { currentPassword: String(form.get("current-password") || ""), newPassword: String(form.get("new-password") || "") },
    });
    onOpenChange(false);
    await adopt(result.csrfToken);
    toast("Password updated");
  });
  return (
    <FormDialog open={open} onOpenChange={onOpenChange} title="Change your password">
      <form onSubmit={onSubmit} className="grid gap-4">
        <input type="hidden" name="username" autoComplete="username" value={user.username} />
        <Note>Use at least 12 characters for your new password.</Note>
        <TextField id="current-password" label="Current password" type="password" autoComplete="current-password" required />
        <TextField id="new-password" label="New password" type="password" autoComplete="new-password" minLength={12} required />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Update password
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

export function SecurityCard({ sessions, loading, error, onRefresh }: { sessions: DeviceSession[]; loading: boolean; error: unknown; onRefresh: () => void }) {
  const user = useUser();
  const { passwords, signOut } = useSession();
  const [password, setPassword] = useState(false);
  const act = async (path: string, method: string, done: string) => {
    try {
      await api.request(path, { method });
      onRefresh();
      toast(done);
    } catch (e) {
      toast((e as Error).message);
    }
  };
  return (
    <Section title="Security" description="Manage your password and active sessions.">
      <PasswordDialog open={password} onOpenChange={setPassword} />
      {passwords !== false && (
        <Line
          icon={Lock}
          title="Password"
          text="Change your local account password."
          action={
            <Button size="sm" variant="outline" onClick={() => setPassword(true)}>
              Change
            </Button>
          }
        />
      )}
      <Line
        icon={Shield}
        title="Current session"
        text={`Signed in as ${user.username}.`}
        action={
          <Button size="sm" variant="outline" onClick={() => signOut()}>
            Sign out
          </Button>
        }
      />
      <div className="flex items-center justify-between gap-2 border-t pt-3">
        <h3 className="text-md font-medium">Signed-in devices</h3>
        <Button size="sm" variant="ghost" onClick={onRefresh}>
          <RefreshCw /> Refresh
        </Button>
      </div>
      <ErrorBox error={error} onRetry={onRefresh} />
      {loading ? (
        <p role="status" className="text-sm text-muted-foreground">
          Loading sessions…
        </p>
      ) : sessions.length ? (
        <>
          <ul className="grid gap-2">
            {sessions.map((s) => (
              <li key={s.id} className="flex min-w-0 items-start gap-3 rounded-md border px-3 py-2.5">
                <div className="min-w-0 grow">
                  <h3 className="flex items-center gap-2 text-md font-medium">
                    {s.current ? "This device" : "Other device"} {s.current && <Pill tone="accent">Current</Pill>}
                  </h3>
                  <p className="truncate text-sm text-muted-foreground" title={s.userAgent || ""}>
                    {s.userAgent || "Browser session"}
                  </p>
                  <small className="text-xs text-muted-foreground">
                    Signed in {when(s.createdAt)}
                    <br />
                    Expires {when(s.expiresAt)}
                  </small>
                </div>
                {!s.current && (
                  <Button size="xs" variant="outline" onClick={() => act(`/api/auth/sessions/${encodeURIComponent(s.id)}`, "DELETE", "Session revoked, and API tokens made on it")}>
                    Revoke
                  </Button>
                )}
              </li>
            ))}
          </ul>
          {sessions.some((s) => !s.current) && (
            <Button size="sm" variant="outline" className="w-fit" onClick={() => act("/api/auth/sessions/revoke-others", "POST", "Other devices signed out, and API tokens made on them revoked")}>
              Sign out other devices
            </Button>
          )}
        </>
      ) : (
        <Note>No sessions to display.</Note>
      )}
    </Section>
  );
}

const mcpUrl = () => `${typeof location === "undefined" ? "" : location.origin}/mcp`;
export const setupCommand = (token: string) => `claude mcp add --transport http obleth-chat ${mcpUrl()} --header "Authorization: Bearer ${token}"`;

type TokenStep = null | "new" | "stale" | { token: string };

/** New token, sign in again, or copy the token just made: one dialog that changes step. */
export function TokenDialog({ step, onStep, onCreated }: { step: TokenStep; onStep: (step: TokenStep) => void; onCreated: () => void }) {
  const { signOut } = useSession();
  const [staleMessage, setStaleMessage] = useState(STALE_SIGN_IN);
  const { busy, error, onSubmit } = useSubmit(async (form) => {
    let result: { token: string };
    try {
      result = await api.request("/api/auth/tokens", {
        method: "POST",
        body: { name: String(form.get("token-name") || "").trim(), days: Number(form.get("token-days")), readOnly: form.get("token-read-only") === "on" },
      });
    } catch (e) {
      // Making a token needs a sign-in from the last few minutes: offer one instead of a dead end.
      if (e instanceof ApiError && e.status === 403 && e.message === STALE_SIGN_IN) {
        setStaleMessage(e.message);
        onStep("stale");
        return;
      }
      throw e;
    }
    onStep({ token: result.token });
    onCreated();
  });
  const open = step !== null;
  const close = (o: boolean) => !o && onStep(null);
  if (step === "stale")
    return (
      <FormDialog open={open} onOpenChange={close} title="Sign in again">
        <Note>{staleMessage}. For your security, Chat asks for a fresh sign-in before it makes a token. Sign out, sign back in, and create the token within 15 minutes.</Note>
        <FormActions>
          <Button variant="outline" onClick={() => onStep(null)}>
            Not now
          </Button>
          <Button onClick={() => signOut()}>Sign out to sign in again</Button>
        </FormActions>
      </FormDialog>
    );
  if (step && typeof step === "object")
    return (
      <FormDialog open={open} onOpenChange={close} title="Copy your token now" wide>
        <Note>This is the only time Chat shows it. Anyone with the token can read and run your chats until it expires or you revoke it.</Note>
        <CopyField id="new-token" label="Token" value={step.token} copyLabel="Copy token" />
        <CopyField
          id="new-token-setup"
          label="Add it to Claude Code"
          value={setupCommand(step.token)}
          rows={4}
          copyLabel="Copy command"
          help={`Other MCP clients connect to ${mcpUrl()} with the header Authorization: Bearer and the token.`}
        />
        <FormActions>
          <Button onClick={() => onStep(null)}>Done</Button>
        </FormActions>
      </FormDialog>
    );
  return (
    <FormDialog open={open} onOpenChange={close} title="New API token">
      <form onSubmit={onSubmit} className="grid gap-4">
        <TextField id="token-name" label="Name" maxLength={64} required placeholder="Claude Code on my laptop" />
        <SelectField
          id="token-days"
          label="Expires after"
          defaultValue="7"
          options={[
            ["1", "1 day"],
            ["7", "7 days"],
            ["30", "30 days"],
          ]}
        />
        <CheckField id="token-read-only" label="Read only" description="It can list and read your chats but not start, answer or stop them." />
        <FormError message={error} />
        <FormActions>
          <Button type="submit" disabled={busy}>
            Create token
          </Button>
        </FormActions>
      </form>
    </FormDialog>
  );
}

export function TokensCard({ tokens, onChange }: { tokens: ApiToken[]; onChange: () => void }) {
  const [step, setStep] = useState<TokenStep>(null);
  const confirm = useConfirm();
  return (
    <Section
      title="API access"
      description="Let an MCP client such as Claude Code start chats, check on them and answer them for you. The chats show up here as yours. A token works only for the MCP endpoint, lasts up to 30 days, and can’t change your account. Signing out the device a token was made on revokes it too."
      actions={
        <Button size="sm" variant="outline" onClick={() => setStep("new")}>
          <Plus /> New token
        </Button>
      }
    >
      <TokenDialog step={step} onStep={setStep} onCreated={onChange} />
      {confirm.element}
      {tokens.length ? (
        <ul className="grid gap-2">
          {tokens.map((t) => (
            <li key={t.id} className="flex items-start gap-3 rounded-md border px-3 py-2.5">
              <div className="min-w-0 grow">
                <h3 className="flex items-center gap-2 text-md font-medium">
                  {t.name} {!t.scopes.includes("chats:write") && <Pill>Read only</Pill>}
                </h3>
                <p className="font-mono text-sm text-muted-foreground">obc_{t.prefix}…</p>
                <small className="text-xs text-muted-foreground">
                  Created {when(t.createdAt)} · expires {when(t.expiresAt)}
                  <br />
                  {t.lastUsedAt ? `Last used ${when(t.lastUsedAt)}` : "Not used yet"}
                </small>
              </div>
              <Button
                size="xs"
                variant="outline"
                onClick={() =>
                  confirm.ask({
                    title: "Revoke this token?",
                    text: `${t.name} stops working at once. Chats it started keep running.`,
                    label: "Revoke token",
                    cancel: "Keep token",
                    action: async () => {
                      await api.request(`/api/auth/tokens/${encodeURIComponent(t.id)}`, { method: "DELETE" });
                      onChange();
                      toast("Token revoked");
                    },
                  })
                }
              >
                Revoke
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <Note>No tokens yet.</Note>
      )}
    </Section>
  );
}

/** Keeps the Security and API access lists fresh after a change. */
export function useRefreshAccount() {
  const queries = useQueryClient();
  return () => queries.invalidateQueries({ queryKey: ["settings", "account"] });
}
