"use client";

import { useParams, useRouter } from "next/navigation";
import { useState } from "react";
import { AuthIntro, AuthLayout } from "@/components/auth/auth-layout";
import { Field, FormError, usePasswordPair } from "@/components/auth/auth-screen";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { useSession } from "@/lib/session";

/** An invitation or reset link: choose a password, then go straight in. */
export default function ActivatePage() {
  const { token } = useParams<{ token: string }>();
  const router = useRouter();
  const { adopt } = useSession();
  const { error, setError, check } = usePasswordPair();
  const [busy, setBusy] = useState(false);
  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setBusy(true);
    setError("");
    try {
      const newPassword = check(form, "activation-password", "activation-confirm");
      const result = await api.request<{ csrfToken?: string }>("/api/auth/activate", { method: "POST", body: { token, newPassword } });
      await adopt(result.csrfToken);
      router.replace("/");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <AuthLayout>
      <AuthIntro title="Set your password" text="Choose the password you’ll use to sign in." />
      <form onSubmit={submit} className="grid gap-4">
        <Field id="activation-password" label="New password" type="password" minLength={12} autoComplete="new-password" required help="At least 12 characters." />
        <Field id="activation-confirm" label="Type it again" type="password" minLength={12} autoComplete="new-password" required />
        <FormError message={error} />
        <Button type="submit" disabled={busy}>
          Save and sign in
        </Button>
      </form>
    </AuthLayout>
  );
}
