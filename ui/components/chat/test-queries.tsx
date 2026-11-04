// Shared by the chat components' tests: a fresh query client per render.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";

export function renderWithQueries(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return { client, ...render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>) };
}

/** cmdk and Radix measure and scroll elements jsdom doesn't lay out. */
export function polyfillLayout() {
  if (!("ResizeObserver" in globalThis))
    (globalThis as any).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = function scrollIntoView() {};
  if (!Element.prototype.hasPointerCapture) Element.prototype.hasPointerCapture = () => false;
}
