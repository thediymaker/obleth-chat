"use client";

import dynamic from "next/dynamic";
import { AuthScreen } from "@/components/auth/auth-screen";
import { HashRedirect } from "@/components/hash-redirect";
import { useSession } from "@/lib/session";

const SignedInApp = dynamic(() => import("@/components/app-shell/signed-in-app").then((m) => m.SignedInApp));

/** Every signed-in page. Until the session is known (or when there is none) this
 * shows the sign-in screens at the same URL, so the page opens after signing in.
 * The app shell and each page's body load only once someone is signed in. */
export default function SignedInLayout({ children }: { children: React.ReactNode }) {
  const { phase } = useSession();
  return (
    <>
      <HashRedirect />
      {phase === "ready" ? <SignedInApp>{children}</SignedInApp> : <AuthScreen />}
    </>
  );
}
