import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// jsdom gives components a DOM; Vite resolves the `@/*` alias from tsconfig.
export default defineConfig({
  plugins: [react()],
  resolve: { tsconfigPaths: true },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    include: ["{app,lib,components,hooks}/**/*.{test,spec}.{ts,tsx}"],
  },
});
