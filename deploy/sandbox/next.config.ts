import type { NextConfig } from "next";

// The workspace preview is served from its own origin through Chat's proxy.
const preview = process.env.OBLETH_PREVIEW_ORIGIN;

const nextConfig: NextConfig = {
  allowedDevOrigins: preview ? [new URL(preview).hostname] : [],
  // Tailwind CSS v4 runs as a Turbopack loader (as create-next-app configures it).
  turbopack: {
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
};

export default nextConfig;
