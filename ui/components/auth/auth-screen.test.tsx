import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { renderPage } from "@/components/manage/test-utils";
import { defaultBranding } from "@/lib/branding";
import { AuthScreen } from "./auth-screen";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

describe("the sign-in screen", () => {
  it.each([
    ["not_provisioned", "has not been provisioned"],
    ["account_disabled", "This account is disabled."],
    ["state_expired", "took too long or was already used"],
    ["not_completed", "Single sign-on was not completed."],
    ["throttled", "Too many sign-in attempts."],
    ["provider_unavailable", "provider is unavailable"],
    ["something_else", "Single sign-on failed."],
  ])("explains a single sign-on failure named in the address: %s", async (code, text) => {
    window.history.replaceState(null, "", `/chat/c1?signin_error=${code}&tab=files`);
    renderPage(<AuthScreen />, { oidc: true });
    expect(await screen.findByRole("alert")).toHaveTextContent(text);
    // The address keeps everything but the failure, so a reload does not repeat it.
    expect(window.location.pathname + window.location.search).toBe("/chat/c1?tab=files");
    expect(screen.getByRole("link", { name: new RegExp(defaultBranding.login.ssoLabel) })).toHaveAttribute("href", "/api/auth/oidc/login?next=%2Fchat%2Fc1%3Ftab%3Dfiles");
  });
});
