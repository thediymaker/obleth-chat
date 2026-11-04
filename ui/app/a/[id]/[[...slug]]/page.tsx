"use client";

import { useParams } from "next/navigation";
import { AppChatPage } from "@/components/apps/app-chat";
import { AuthScreen } from "@/components/auth/auth-screen";
import { useSession } from "@/lib/session";

/** An app's chat page, on its own without Chat's sidebar. Signing in happens at the
 * same address, so the app opens afterwards. */
export default function Page() {
  const { id, slug } = useParams<{ id: string; slug?: string[] }>();
  const { phase } = useSession();
  if (phase !== "ready") return <AuthScreen />;
  return <AppChatPage id={id} slug={slug?.[0]} />;
}
