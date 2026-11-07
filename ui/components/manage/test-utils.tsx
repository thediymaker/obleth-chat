// Test helpers for Settings, Administration, MCP servers and Groups: a fake session,
// the API answered by a handler, and a fresh query client per render.
//
//   vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
//   const calls = mockApi((path, options) => ({ ... }));
//   renderPage(<PeopleTab />, { user: { role: "admin" } });

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { api, type RequestOptions } from "@/lib/api";
import type { Banner, User } from "@/lib/session";

// Radix measures checkboxes and switches; jsdom has no ResizeObserver.
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

const state = {
  user: { id: "u1", username: "ana", display_name: "Ana", role: "admin" } as User,
  passwords: true,
  oidc: false,
  banner: null as Banner | null,
  reload: vi.fn(async () => {}),
  signOut: vi.fn(async () => {}),
  adopt: vi.fn(async () => {}),
  setBanner: vi.fn(),
};

export function useSession() {
  return { ...state, phase: "ready" as const, isAdmin: state.user.role === "admin", error: "", notice: "", signIn: async () => {} };
}
export function useUser() {
  return state.user;
}
export function SessionProvider({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}
export type { Banner, User };

export interface Call {
  path: string;
  method: string;
  body?: unknown;
}
/** Answers every API request with the handler and records it. Capabilities answer with `features`. */
export function mockApi(handler: (path: string, options: RequestOptions) => unknown, features: Record<string, boolean> = {}) {
  const calls: Call[] = [];
  vi.spyOn(api, "request").mockImplementation((async (path: string, options: RequestOptions = {}) => {
    if (path === "/api/capabilities") return { application: "chat", mode: "connected", features };
    calls.push({ path, method: options.method || "GET", body: options.body });
    const result = await handler(path, options);
    if (result instanceof Error) throw result;
    return result ?? {};
  }) as typeof api.request);
  return calls;
}

export function renderPage(ui: React.ReactElement, { user, passwords = true, oidc = false }: { user?: Partial<User>; passwords?: boolean; oidc?: boolean } = {}) {
  Object.assign(state, { user: { id: "u1", username: "ana", display_name: "Ana", role: "admin", ...user }, passwords, oidc });
  state.reload.mockClear();
  state.setBanner.mockClear();
  state.signOut.mockClear();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return { client, session: state, ...render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>) };
}

/** Picks an option in a Dropdown: opens it and clicks the option with that name. */
export async function choose(trigger: HTMLElement, name: string | RegExp) {
  await userEvent.click(trigger);
  await userEvent.click(await screen.findByRole("option", { name }));
}
