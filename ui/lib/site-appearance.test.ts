import { beforeEach, describe, expect, it, vi } from "vitest";
import { normalizeBranding } from "./branding";
import { cornerProperties, readAppearance, siteAppearance, withAppearance } from "./site-appearance";

describe("readAppearance", () => {
  it("keeps what is set and makes sense", () => {
    expect(readAppearance({ siteName: " Research Computing ", accent: "#5FD3BC", defaultTheme: "dark", personalAccent: false, corners: "round" })).toEqual({
      siteName: "Research Computing",
      accent: "#5fd3bc",
      defaultTheme: "dark",
      personalAccent: false,
      corners: "round",
    });
  });
  it("leaves the rest to the file", () => {
    expect(readAppearance({ siteName: " ", accent: "teal", defaultTheme: "sepia", personalAccent: "no", corners: "square", logo: "x" })).toEqual({});
    expect(readAppearance(null)).toEqual({});
    expect(readAppearance("dark")).toEqual({});
  });
});

describe("withAppearance", () => {
  const file = { siteName: "Lab\nChat", assistantName: "Helper", logo: { light: "./assets/lab.svg", alt: "Lab" }, home: { eyebrow: "Welcome", title: "Hello" }, login: { help: "Ask IT" }, appearance: { defaultTheme: "light", accent: "#7cb7ff" } };
  it("lays the administrator's settings over the file's", () => {
    const shown = withAppearance(file, { siteName: "Research Computing", accent: "#5fd3bc", personalAccent: false, branding: { assistantName: "Sparky", home: { eyebrow: "" }, login: { helpLink: { label: "Request an account", url: "https://accounts.example.edu" } }, links: [] } });
    expect(shown.siteName).toBe("Research Computing");
    expect(shown.siteNameLines).toEqual(["Research Computing"]);
    expect(shown.appearance).toEqual({ defaultTheme: "light", accent: "#5fd3bc", personalAccent: false });
    expect(shown.assistantName).toBe("Sparky");
    // A present but empty text hides the line; what was not sent stays the file's.
    expect(shown.home).toMatchObject({ eyebrow: "", title: "Hello" });
    expect(shown.login.help).toBe("Ask IT");
    expect(shown.login.helpLink).toEqual({ label: "Request an account", url: "https://accounts.example.edu" });
    expect(shown.links).toEqual([]);
    expect(shown.logo).toEqual({ light: "/assets/lab.svg", dark: "/assets/lab.svg", alt: "Lab" });
  });
  it("shows an uploaded logo and tab icon in place of the file's, or no logo at all", () => {
    const images = { "logo-dark": "/api/branding/images/logo-dark?v=abc", favicon: "/api/branding/images/favicon?v=def" };
    const shown = withAppearance(file, { branding: { images, logoAlt: "Research Computing" } });
    expect(shown.logo).toEqual({ light: images["logo-dark"], dark: images["logo-dark"], alt: "Research Computing" });
    expect(shown.favicon).toEqual({ light: images.favicon, dark: images.favicon });
    expect(withAppearance(file, { branding: { images, hideLogo: true } }).logo).toEqual({ light: "", dark: "", alt: "" });
  });
  it("is the file's own when nothing is set, and does not trust an address that leaves the site", () => {
    expect(withAppearance(file, {})).toEqual(normalizeBranding(file));
    expect(withAppearance(file, { branding: { images: { "logo-light": "https://elsewhere.example/x.svg" } } }).logo.light).toBe("");
  });
});

describe("cornerProperties", () => {
  it("sets the radii for sharp and round, and nothing for Chat's own", () => {
    expect(cornerProperties("sharp")["--corner-lg"]).toBe("3px");
    expect(cornerProperties("round")["--corner-xl"]).toBe("20px");
    expect(cornerProperties("soft")).toEqual({});
    expect(cornerProperties(undefined)).toEqual({});
  });
});

describe("siteAppearance", () => {
  beforeEach(() => {
    window.localStorage.clear();
    siteAppearance.reset();
  });
  it("keeps the server's word for the next visit and tells its listeners once", () => {
    const heard = vi.fn();
    const stop = siteAppearance.subscribe(heard);
    siteAppearance.set({ accent: "#5fd3bc", unknown: 1 });
    siteAppearance.set({ accent: "#5fd3bc" });
    expect(heard).toHaveBeenCalledTimes(1);
    expect(JSON.parse(window.localStorage.getItem("obleth.site-appearance")!)).toEqual({ accent: "#5fd3bc" });
    siteAppearance.reset();
    expect(siteAppearance.get()).toEqual({ accent: "#5fd3bc" });
    siteAppearance.set({});
    expect(window.localStorage.getItem("obleth.site-appearance")).toBeNull();
    expect(heard).toHaveBeenCalledTimes(2);
    stop();
  });
  it("reads nothing from a broken note", () => {
    window.localStorage.setItem("obleth.site-appearance", "{not json");
    expect(siteAppearance.get()).toEqual({});
  });
});
