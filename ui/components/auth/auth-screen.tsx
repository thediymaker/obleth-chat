"use client";

import { AlertCircle, LogIn, RefreshCw } from "lucide-react";
import { useState } from "react";
import { AuthIntro, AuthLayout } from "@/components/auth/auth-layout";
import { Copy, ExternalAwareLink } from "@/components/brand";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api } from "@/lib/api";
import { useBranding } from "@/lib/branding-context";
import { useSession } from "@/lib/session";

/** Signing in with the provider, back to this page afterwards. */
function signInHref() {
  if (typeof window === "undefined") return "/api/auth/oidc/login";
  const here = window.location.pathname + window.location.search;
  return here === "/" ? "/api/auth/oidc/login" : `/api/auth/oidc/login?next=${encodeURIComponent(here)}`;
}

/** Whatever the person sees before the app: checking, sign-in, a required password change, or a failed check. */
export function AuthScreen() {
  const session = useSession();
  return (
    <AuthLayout>
      {session.phase === "loading" ? (
        <Checking />
      ) : session.phase === "error" ? (
        <Failed />
      ) : session.phase === "password-required" ? (
        <RequiredPassword />
      ) : (
        <SignIn />
      )}
    </AuthLayout>
  );
}

export function Checking({ label = "Checking your sign-in…" }: { label?: string }) {
  return (
    <div role="status" className="flex flex-col items-center gap-3 py-10 text-sm text-muted-foreground">
      <span aria-hidden className="size-5 animate-spin rounded-full border-2 border-border border-t-foreground" />
      <p>{label}</p>
    </div>
  );
}

function Failed() {
  const { error, reload } = useSession();
  return (
    <>
      <AuthIntro title="Can’t connect" text="We couldn’t check your sign-in. Try again in a minute." />
      {error && <p className="mb-4 text-sm text-muted-foreground">{error}</p>}
      <Button onClick={() => reload()}>
        <RefreshCw /> Try again
      </Button>
    </>
  );
}

export function FormError({ message }: { message: string }) {
  if (!message) return null;
  return (
    <div role="alert" className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive-foreground">
      <AlertCircle className="mt-0.5 size-4 shrink-0" />
      <span>{message}</span>
    </div>
  );
}

export function Field({ id, label, help, ...input }: React.ComponentProps<typeof Input> & { id: string; label: string; help?: string }) {
  return (
    <div className="grid gap-1.5">
      <Label htmlFor={id}>{label}</Label>
      <Input id={id} name={id} aria-describedby={help ? `${id}-help` : undefined} {...input} />
      {help && (
        <p id={`${id}-help`} className="text-xs text-muted-foreground">
          {help}
        </p>
      )}
    </div>
  );
}

function SignIn() {
  const { oidc, passwords, notice, signIn } = useSession();
  const branding = useBranding();
  const login = branding.login;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  // With single sign-on the provider is the main way in and passwords come second;
  // with passwords turned off it is the only way in.
  const withPasswords = !(oidc && passwords === false);
  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setBusy(true);
    setError("");
    try {
      await signIn(String(form.get("username") || "").trim(), String(form.get("password") || ""));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const help = [login.help].filter(Boolean);
  return (
    <>
      <AuthIntro title={login.title ? <Copy text={login.title} /> : `Sign in to ${branding.siteName}`} text={login.description ? <Copy text={login.description} /> : undefined} />
      <div className="grid gap-4">
        {notice && !error && (
          <p role="status" className="text-sm text-muted-foreground">
            {notice}
          </p>
        )}
        <FormError message={error} />
        {oidc && (
          <>
            <Button asChild>
              <a href={signInHref()}>
                <LogIn /> {login.ssoLabel}
              </a>
            </Button>
            {login.ssoHint && (
              <p className="text-center text-xs text-muted-foreground">
                <Copy text={login.ssoHint} />
              </p>
            )}
            {withPasswords && <div className="text-center text-xs text-muted-foreground">or sign in with a password</div>}
          </>
        )}
        {withPasswords && (
          <form onSubmit={submit} className="grid gap-4">
            <Field id="username" label="Username" autoComplete="username" maxLength={128} required autoFocus={!oidc} />
            <Field id="password" label="Password" type="password" autoComplete="current-password" required />
            <Button type="submit" variant={oidc ? "outline" : "default"} disabled={busy}>
              {busy ? "Signing in…" : oidc ? "Sign in with password" : "Sign in"}
            </Button>
          </form>
        )}
        {(help.length > 0 || login.helpLink) && (
          <p className="text-xs text-muted-foreground">
            {login.help && <Copy text={login.help} />}{" "}
            {login.helpLink && (
              <ExternalAwareLink url={login.helpLink.url} className="underline underline-offset-2 hover:text-foreground">
                {login.helpLink.label}
              </ExternalAwareLink>
            )}
          </p>
        )}
      </div>
    </>
  );
}

/** New-password fields that must match; used by activation and the required change. */
export function usePasswordPair() {
  const [error, setError] = useState("");
  const check = (form: FormData, first: string, second: string) => {
    if (form.get(first) !== form.get(second)) throw new Error("The passwords do not match.");
    return String(form.get(first));
  };
  return { error, setError, check };
}

function RequiredPassword() {
  const { user, adopt, signOut } = useSession();
  const { error, setError, check } = usePasswordPair();
  const [busy, setBusy] = useState(false);
  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setBusy(true);
    setError("");
    try {
      const newPassword = check(form, "required-password", "required-confirm");
      const result = await api.request<{ csrfToken?: string }>("/api/auth/password", {
        method: "POST",
        body: { currentPassword: String(form.get("required-current")), newPassword },
      });
      await adopt(result.csrfToken);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <AuthIntro title="Choose a new password" text="Your administrator asked for a new password before you continue." />
      <form onSubmit={submit} className="grid gap-4">
        <input type="hidden" autoComplete="username" value={user?.username ?? ""} readOnly />
        <Field id="required-current" label="Current password" type="password" autoComplete="current-password" required />
        <Field id="required-password" label="New password" type="password" minLength={12} autoComplete="new-password" required help="At least 12 characters." />
        <Field id="required-confirm" label="Type it again" type="password" minLength={12} autoComplete="new-password" required />
        <FormError message={error} />
        <Button type="submit" disabled={busy}>
          Update password
        </Button>
      </form>
      <Button variant="ghost" size="sm" className="mt-4" onClick={() => signOut()}>
        Sign out
      </Button>
    </>
  );
}
