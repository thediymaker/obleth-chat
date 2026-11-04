import { NextResponse, type NextRequest } from "next/server";
import { contentSecurityPolicy, generateNonce } from "@/lib/csp";

// A fresh nonce per document. Forwarding the policy on the request is what lets
// Next.js stamp the nonce on the scripts it renders; the same value goes back on
// the response so the browser enforces it. Sign-in is checked in the browser
// against /api/session, since accounts live in the Rust server.
export function proxy(request: NextRequest) {
  const nonce = generateNonce();
  const csp = contentSecurityPolicy(nonce, { previews: request.headers.get("x-obleth-chat-preview-origin") });
  const headers = new Headers(request.headers);
  headers.set("content-security-policy", csp);
  headers.set("x-nonce", nonce);
  const response = NextResponse.next({ request: { headers } });
  response.headers.set("Content-Security-Policy", csp);
  return response;
}

export const config = {
  // Build assets are skipped; every HTML document carries the policy.
  matcher: ["/((?!_next/static|_next/image|fonts/).*)"],
};
