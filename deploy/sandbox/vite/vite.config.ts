import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The workspace preview is served from its own origin through Chat's proxy.
const preview = process.env.OBLETH_PREVIEW_ORIGIN;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    allowedHosts: preview ? [new URL(preview).hostname] : [],
  },
});
