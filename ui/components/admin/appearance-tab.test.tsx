// Administration › Branding.

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
import { polyfillLayout, renderWithQueries } from "@/components/chat/test-queries";
import { choose } from "@/components/manage/test-utils";
import { normalizeBranding } from "@/lib/branding";
import { siteAppearance } from "@/lib/site-appearance";
import { AppearanceTab, brandingBody, formOf, shownOf, type AppearanceSettings } from "./appearance-tab";

polyfillLayout();

const unset: AppearanceSettings = { siteName: null, accent: null, defaultTheme: null, personalAccent: null, corners: null, branding: {}, images: {} };
const look = { siteName: null, accent: null, defaultTheme: null, personalAccent: null, corners: null };
const config = {
  siteName: "Lab Chat",
  siteSubtitle: "Research computing",
  login: { help: "Trouble signing in?", helpLink: { label: "Ask us", url: "https://example.org/help" } },
  links: [{ label: "Docs", url: "https://example.org/docs" }],
  home: { title: "What are you working on?" },
};
const file = normalizeBranding(config);

beforeEach(() => {
  window.localStorage.clear();
  siteAppearance.reset();
  apiRequest.mockReset();
  window.OblethChatConfig = config as never;
});

describe("brandingBody", () => {
  it("sends nothing for a form left as the file says", () => {
    expect(brandingBody(formOf(unset, file), file)).toEqual({ ...look, branding: {} });
  });
  it("sends only what differs from the file, and empty for a line that was cleared", () => {
    const form = { ...formOf(unset, file), siteName: " Research Computing ", accent: "5FD3BC", defaultTheme: "dark", personalAccent: "no", corners: "round", siteSubtitle: "", homeTitle: "Ask anything", ssoLabel: "Sign in with campus ID", hideLogo: true };
    expect(brandingBody(form, file)).toEqual({
      siteName: "Research Computing",
      accent: "#5fd3bc",
      defaultTheme: "dark",
      personalAccent: false,
      corners: "round",
      branding: { siteSubtitle: "", hideLogo: true, login: { ssoLabel: "Sign in with campus ID" }, home: { title: "Ask anything" } },
    });
  });
  it("sends the links whole when one changed, and the help link when it was taken away", () => {
    const form = { ...formOf(unset, file), links: [...file.links, { label: " Status ", url: "https://example.org/status" }, { label: "", url: "" }], helpLinkLabel: "", helpLinkUrl: "" };
    expect(brandingBody(form, file).branding).toEqual({
      links: [
        { label: "Docs", url: "https://example.org/docs" },
        { label: "Status", url: "https://example.org/status" },
      ],
      login: { helpLink: { label: "", url: "" } },
    });
  });
  it("refuses a color that is not one and a link with half of itself", () => {
    expect(() => brandingBody({ ...formOf(unset, file), accent: "teal" }, file)).toThrow("six hex digits");
    expect(() => brandingBody({ ...formOf(unset, file), links: [{ label: "Docs", url: "" }] }, file)).toThrow("Each link needs both");
    expect(() => brandingBody({ ...formOf(unset, file), helpLinkUrl: "" }, file)).toThrow("help link needs both");
  });
});

test("what is saved is what the page shows, with the uploaded images", () => {
  expect(shownOf({ ...unset, accent: "#7cb7ff", branding: { home: { title: "Hi" } }, images: { "logo-light": "/api/branding/images/logo-light?v=ab" }, updatedAt: "2026-11-21T10:00:00Z" })).toEqual({
    accent: "#7cb7ff",
    branding: { home: { title: "Hi" }, images: { "logo-light": "/api/branding/images/logo-light?v=ab" } },
  });
  expect(shownOf(unset)).toEqual({ branding: {} });
});

test("starts from what the site shows, previews a choice on both themes, and saves what changed for this page at once", async () => {
  apiRequest.mockImplementation(async (path: string, options?: { method?: string; body?: unknown }) => {
    if (path === "/api/admin/appearance" && !options?.method) return unset;
    if (path === "/api/admin/appearance" && options?.method === "PATCH") return { ...(options.body as object), images: {}, updatedAt: "2026-11-21T10:00:00Z" };
    throw new Error(`unexpected ${path}`);
  });
  renderWithQueries(<AppearanceTab />);
  expect((await screen.findByLabelText("Site name") as HTMLInputElement).value).toBe("Lab Chat");
  expect((screen.getByLabelText("Line under the name") as HTMLInputElement).value).toBe("Research computing");
  expect((screen.getByLabelText("Link 1 address") as HTMLInputElement).value).toBe("https://example.org/docs");
  // The installed accent is light, so the light sample shows it darkened.
  expect(document.querySelector('[data-sample="light"] figcaption')?.textContent).toContain("to stay readable");
  expect(document.querySelector('[data-sample="dark"] figcaption')?.textContent).toBe("Dark");

  fireEvent.click(screen.getByRole("radio", { name: "Blue" }));
  expect((screen.getByLabelText("Accent as a hex color") as HTMLInputElement).value).toBe("#7cb7ff");
  fireEvent.change(screen.getByLabelText("Site name"), { target: { value: "Research Computing" } });
  expect(document.querySelector('[data-sample="dark"]')?.textContent).toContain("Research Computing");
  fireEvent.change(screen.getByLabelText("Title", { selector: "#branding-home-title" }), { target: { value: "Ask anything" } });
  await choose(screen.getByLabelText("Corners"), "Round");
  fireEvent.click(screen.getByRole("button", { name: "Save branding" }));

  const body = { siteName: "Research Computing", accent: "#7cb7ff", defaultTheme: null, personalAccent: null, corners: "round", branding: { home: { title: "Ask anything" } } };
  await waitFor(() => expect(apiRequest).toHaveBeenCalledWith("/api/admin/appearance", { method: "PATCH", body }));
  await waitFor(() => expect(siteAppearance.get()).toEqual({ siteName: "Research Computing", accent: "#7cb7ff", corners: "round", branding: { home: { title: "Ask anything" } } }));
  // A long form with several menus: slow to draw when the whole suite runs at once.
}, 20_000);

test("an image is uploaded on its own and shows at once; removing it goes back to the installed one", async () => {
  const address = "/api/branding/images/logo-light?v=0a1b2c3d";
  apiRequest.mockImplementation(async (path: string, options?: { method?: string }) => {
    if (path === "/api/admin/appearance") return unset;
    if (path === "/api/admin/branding/images/logo-light" && options?.method === "PUT") return { images: { "logo-light": address } };
    if (path === "/api/admin/branding/images/logo-light" && options?.method === "DELETE") return { images: {} };
    throw new Error(`unexpected ${path}`);
  });
  renderWithQueries(<AppearanceTab />);
  const input = await screen.findByLabelText("Logo on the light theme: choose an image");
  const image = new File([new Uint8Array([137, 80, 78, 71])], "logo.png", { type: "image/png" });
  fireEvent.change(input, { target: { files: [image] } });
  await waitFor(() => expect(apiRequest).toHaveBeenCalledWith("/api/admin/branding/images/logo-light", { method: "PUT", raw: image, contentType: "application/octet-stream" }));
  await waitFor(() => expect(document.querySelector('[data-image-slot="logo-light"] img')?.getAttribute("src")).toBe(address));
  expect(siteAppearance.get().branding).toEqual({ images: { "logo-light": address } });

  fireEvent.click(screen.getByRole("button", { name: "Remove the uploaded logo on the light theme" }));
  await waitFor(() => expect(siteAppearance.get().branding ?? {}).toEqual({}));
  expect(document.querySelector('[data-image-slot="logo-light"]')?.textContent).toContain("As installed");
});

test("a color that is not one is not sent", async () => {
  apiRequest.mockResolvedValue(unset);
  renderWithQueries(<AppearanceTab />);
  fireEvent.change(await screen.findByLabelText("Accent as a hex color"), { target: { value: "teal" } });
  fireEvent.click(screen.getByRole("button", { name: "Save branding" }));
  expect(await screen.findByText("The accent is a color as six hex digits, such as #d7f25a.")).toBeTruthy();
  expect(apiRequest).toHaveBeenCalledTimes(1);
});
