import { describe, expect, it } from "vitest";
import { ACCENTS, DEFAULT_ACCENT, GROUND, accentHex, accentProperties, contrast, shadeFor } from "./accent";

describe("accentHex", () => {
  it("keeps a six-digit color, in lower case", () => {
    expect(accentHex(" #D7F25A ")).toBe("#d7f25a");
  });
  it("widens a three-digit color", () => {
    expect(accentHex("#0af")).toBe("#00aaff");
  });
  it("drops anything that is not a hex color", () => {
    for (const value of ["red", "#12345", "rgb(0,0,0)", "#d7f25a; color:red", "", null, 7]) expect(accentHex(value)).toBe("");
  });
});

describe("shadeFor", () => {
  it("keeps an accent that already reads on the ground", () => {
    expect(shadeFor(DEFAULT_ACCENT, GROUND.dark)).toEqual({ color: DEFAULT_ACCENT, foreground: "#0b0b0c", adjusted: false });
  });
  it("darkens a light accent until it reads on the light ground", () => {
    const shade = shadeFor(DEFAULT_ACCENT, GROUND.light);
    expect(shade.adjusted).toBe(true);
    expect(contrast(shade.color, GROUND.light)).toBeGreaterThanOrEqual(4.5);
    expect(shade.foreground).toBe("#ffffff");
  });
  it("lightens a dark accent until it reads on the dark ground", () => {
    const shade = shadeFor("#1a2a6c", GROUND.dark);
    expect(shade.adjusted).toBe(true);
    expect(contrast(shade.color, GROUND.dark)).toBeGreaterThanOrEqual(4.5);
  });
  it("reads on both grounds for every accent Settings offers", () => {
    for (const { value } of ACCENTS)
      for (const ground of Object.values(GROUND)) {
        const shade = shadeFor(value, ground);
        expect(contrast(shade.color, ground)).toBeGreaterThanOrEqual(4.5);
        expect(contrast(shade.foreground, shade.color)).toBeGreaterThanOrEqual(4.5);
      }
  });
});

describe("accentProperties", () => {
  it("sets nothing for the default or for a value that is not a color", () => {
    expect(accentProperties(DEFAULT_ACCENT)).toEqual({});
    expect(accentProperties("url(x)")).toEqual({});
  });
  it("carries one shade per theme", () => {
    const set = accentProperties("#7cb7ff");
    expect(set["--signal-dark"]).toBe("#7cb7ff");
    expect(contrast(set["--signal-light"]!, GROUND.light)).toBeGreaterThanOrEqual(4.5);
    expect(Object.keys(set)).toHaveLength(4);
  });
});
