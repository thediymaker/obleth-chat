import { beforeEach, describe, expect, it, vi } from "vitest";
import { defaultBranding } from "./branding";
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
  const file = { ...defaultBranding, siteName: "Lab Chat", siteNameLines: ["Lab", "Chat"], appearance: { defaultTheme: "light" as const, accent: "#7cb7ff", personalAccent: true } };
  it("lays the administrator's settings over the file's", () => {
    const shown = withAppearance(file, { siteName: "Research Computing", accent: "#5fd3bc", personalAccent: false });
    expect(shown.siteName).toBe("Research Computing");
    expect(shown.siteNameLines).toEqual(["Research Computing"]);
    expect(shown.appearance).toEqual({ defaultTheme: "light", accent: "#5fd3bc", personalAccent: false });
  });
  it("is the file's own when nothing is set", () => {
    expect(withAppearance(file, {})).toBe(file);
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
