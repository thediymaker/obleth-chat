import path from "node:path";
import type { NextConfig } from "next";

// The Rust server is the front door: it answers /api, streams and websockets
// itself and proxies every other request here. `next dev` has no Rust server in
// front, so OBLETH_CHAT_API_URL points its /api requests at one.
const api = process.env.OBLETH_CHAT_API_URL;

const nextConfig: NextConfig = {
  output: "standalone",
  reactStrictMode: true,
  poweredByHeader: false,
  // next dev would otherwise write an AGENTS.md into the project.
  agentRules: false,
  outputFileTracingRoot: path.join(__dirname),
  async rewrites() {
    if (!api) return [];
    return ["/api/:path*", "/site.config.js", "/assets/:path*", "/.well-known/:path*"].map((source) => ({
      source,
      destination: `${api}${source}`,
    }));
  },
};

export default nextConfig;
