// Administration › Appearance.

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";
import { choose } from "@/components/manage/test-utils";
import { siteAppearance } from "@/lib/site-appearance";
import { AppearanceTab, appearanceBody } from "./appearance-tab";

polyfillLayout();

const unset = { siteName: null, accent: null, defaultTheme: null, personalAccent: null, corners: null };

beforeEach(() => {
  window.localStorage.clear();
  siteAppearance.reset();
  apiRequest.mockReset();
});

describe("appearanceBody", () => {
  it("sends what was chosen, and null for what is left as installed", () => {
    expect(appearanceBody({ siteName: " Research Computing ", accent: "5FD3BC", defaultTheme: "dark", personalAccent: "no", corners: "round" })).toEqual({
      siteName: "Research Computing",
      accent: "#5fd3bc",
      defaultTheme: "dark",
      personalAccent: false,
      corners: "round",
    });
    expect(appearanceBody({ siteName: " ", accent: "", defaultTheme: "", personalAccent: "", corners: "" })).toEqual(unset);
  });
  it("refuses a color that is not one", () => {
    expect(() => appearanceBody({ siteName: "", accent: "teal", defaultTheme: "", personalAccent: "", corners: "" })).toThrow("six hex digits");
  });
});

test("shows what is saved, previews a choice on both themes, and saves it for this page at once", async () => {
  apiRequest.mockImplementation(async (path: string, options?: { method?: string; body?: unknown }) => {
    if (path === "/api/admin/appearance" && !options?.method) return unset;
    if (path === "/api/admin/appearance" && options?.method === "PATCH") return { ...(options.body as object), updatedAt: "2026-11-21T10:00:00Z" };
    throw new Error(`unexpected ${path}`);
  });
  renderWithQueries(<AppearanceTab />);
  expect(await screen.findByText("As installed")).toBeTruthy();
  // The installed accent is light, so the light sample shows it darkened.
  expect(document.querySelector('[data-sample="light"] figcaption')?.textContent).toContain("to stay readable");
  expect(document.querySelector('[data-sample="dark"] figcaption')?.textContent).toBe("Dark");

  fireEvent.click(screen.getByRole("radio", { name: "Blue" }));
  expect((screen.getByLabelText("Accent as a hex color") as HTMLInputElement).value).toBe("#7cb7ff");
  fireEvent.change(screen.getByLabelText("Site name"), { target: { value: "Research Computing" } });
  expect(document.querySelector('[data-sample="dark"]')?.textContent).toContain("Research Computing");
  await choose(screen.getByLabelText("Corners"), "Round");
  fireEvent.click(screen.getByRole("button", { name: "Save appearance" }));

  await waitFor(() => expect(apiRequest).toHaveBeenCalledWith("/api/admin/appearance", { method: "PATCH", body: { siteName: "Research Computing", accent: "#7cb7ff", defaultTheme: null, personalAccent: null, corners: "round" } }));
  await waitFor(() => expect(siteAppearance.get()).toEqual({ siteName: "Research Computing", accent: "#7cb7ff", corners: "round" }));
});

test("a color that is not one is not sent", async () => {
  apiRequest.mockResolvedValue(unset);
  renderWithQueries(<AppearanceTab />);
  fireEvent.change(await screen.findByLabelText("Accent as a hex color"), { target: { value: "teal" } });
  fireEvent.click(screen.getByRole("button", { name: "Save appearance" }));
  expect(await screen.findByText("The accent is a color as six hex digits, such as #d7f25a.")).toBeTruthy();
  expect(apiRequest).toHaveBeenCalledTimes(1);
});
