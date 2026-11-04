"use client";

// Who is signed in. Accounts and sessions live in the Rust server: this asks
// /api/session once, keeps the CSRF token on the API client, and sends the
// person back to sign in when any request answers 401.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@/lib/api";

export interface User {
  id: string;
  username: string;
  display_name: string;
  role: "admin" | "user";
}
export interface Banner {
  text: string;
  tone?: string;
  dismissible?: boolean;
  [key: string]: unknown;
}

export type Phase = "loading" | "login" | "password-required" | "ready" | "error";

interface SessionResponse {
  authenticated: boolean;
  user?: User;
  csrfToken?: string;
  oidcEnabled?: boolean;
  passwordSignIn?: boolean;
  passwordChangeRequired?: boolean;
  banner?: Banner | null;
}

export interface SessionState {
  phase: Phase;
  user: User | null;
  /** Single sign-on is configured. */
  oidc: boolean;
  /** Password sign-in is allowed (false only when single sign-on replaces it). */
  passwords: boolean;
  banner: Banner | null;
  error: string;
  /** Why the person was sent back to sign in, such as an expired session. */
  notice: string;
}

interface SessionContext extends SessionState {
  isAdmin: boolean;
  /** Asks the server again, as after signing in. */
  reload: () => Promise<void>;
  /** Signs in with a password; throws the server's message on failure. */
  signIn: (username: string, password: string) => Promise<void>;
  /** Ends the session here and on the server. */
  signOut: () => Promise<void>;
  /** Called with the CSRF token an activation or password change returns. */
  adopt: (csrfToken?: string) => Promise<void>;
  setBanner: (banner: Banner | null) => void;
}

const Context = createContext<SessionContext | null>(null);

// Work that must reach the server while the session still exists, such as leaving
// a temporary chat (which deletes it).
const beforeSignOut = new Set<() => Promise<unknown> | unknown>();
/** Runs `hook` just before signing out; returns a function that removes it. */
export function onBeforeSignOut(hook: () => Promise<unknown> | unknown) {
  beforeSignOut.add(hook);
  return () => {
    beforeSignOut.delete(hook);
  };
}

const initial: SessionState = { phase: "loading", user: null, oidc: false, passwords: true, banner: null, error: "", notice: "" };

export function SessionProvider({ children }: { children: React.ReactNode }) {
  const [state, setState] = useState<SessionState>(initial);
  const queries = useQueryClient();
  // A sign-out or expiry while an older check is still running wins over it.
  const epoch = useRef(0);

  const expire = useCallback(
    (notice: string) => {
      epoch.current++;
      api.clearSession();
      queries.clear();
      setState((s) => ({ ...s, phase: "login", user: null, error: "", notice }));
    },
    [queries],
  );

  const reload = useCallback(async () => {
    const mine = ++epoch.current;
    setState((s) => ({ ...s, phase: s.phase === "ready" ? "ready" : "loading", error: "" }));
    try {
      const result = await api.request<SessionResponse>("/api/session");
      if (mine !== epoch.current) return;
      const base = { oidc: result.oidcEnabled ?? false, passwords: result.passwordSignIn ?? true, banner: result.banner ?? null };
      if (!result.authenticated || !result.user) {
        setState((s) => ({ ...s, ...base, phase: "login", user: null }));
        return;
      }
      api.csrfToken = result.csrfToken || "";
      setState((s) => ({
        ...s,
        ...base,
        user: result.user!,
        phase: result.passwordChangeRequired ? "password-required" : "ready",
        notice: "",
      }));
    } catch (error) {
      if (mine !== epoch.current || (error as Error).name === "AbortError") return;
      setState((s) => (s.phase === "login" ? s : { ...s, phase: "error", error: (error as Error).message }));
    }
  }, []);

  useEffect(() => {
    api.onUnauthorized = (error: ApiError) => expire(error.message);
    reload();
    return () => {
      api.onUnauthorized = () => {};
    };
  }, [expire, reload]);

  const value = useMemo<SessionContext>(
    () => ({
      ...state,
      isAdmin: state.user?.role === "admin",
      reload,
      async signIn(username, password) {
        const result = await api.request<{ csrfToken?: string }>("/api/auth/login", { method: "POST", body: { username, password } });
        if (result.csrfToken) api.csrfToken = result.csrfToken;
        await reload();
      },
      async signOut() {
        await Promise.allSettled([...beforeSignOut].map((hook) => hook()));
        try {
          await api.request("/api/auth/logout", { method: "POST" });
        } finally {
          expire("");
        }
      },
      async adopt(csrfToken) {
        if (csrfToken) api.csrfToken = csrfToken;
        await reload();
      },
      setBanner: (banner) => setState((s) => ({ ...s, banner })),
    }),
    [state, reload, expire],
  );
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useSession(): SessionContext {
  const value = useContext(Context);
  if (!value) throw new Error("useSession needs a SessionProvider");
  return value;
}

/** The signed-in person; only for components under the signed-in layout. */
export function useUser(): User {
  const { user } = useSession();
  if (!user) throw new Error("useUser needs a signed-in session");
  return user;
}
