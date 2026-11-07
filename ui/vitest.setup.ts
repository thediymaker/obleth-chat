import "@testing-library/jest-dom/vitest";

// jsdom has the scroll properties but not the methods; components that keep a
// timeline pinned to its end call scrollTo in an effect.
if (typeof Element !== "undefined" && !Element.prototype.scrollTo) {
  Element.prototype.scrollTo = function scrollTo(options?: ScrollToOptions | number, y?: number): void {
    const top = typeof options === "number" ? y : options?.top;
    if (typeof top === "number") this.scrollTop = top;
  };
}
if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({ matches: false, media: query, onchange: null, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent: () => false }) as MediaQueryList;
}

// Radix Select (the app's Dropdown) captures the pointer and scrolls the chosen
// option into view; jsdom has neither.
if (typeof Element !== "undefined") {
  Element.prototype.hasPointerCapture ||= () => false;
  Element.prototype.setPointerCapture ||= () => {};
  Element.prototype.releasePointerCapture ||= () => {};
  Element.prototype.scrollIntoView ||= () => {};
}
