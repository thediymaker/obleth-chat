// Content Security Policy for every document, built per request in proxy.ts.
// Scripts run by nonce: Next.js reads it from the request's policy and stamps
// it on the scripts it emits, and 'strict-dynamic' extends trust to the chunks
// those load. Styles stay 'unsafe-inline' for Radix and React Flow's positioning.
//
// Workspace previews live on their own origins (one per preview); Chat frames
// them and fetches their status. The Rust server passes that origin pattern in
// the x-obleth-chat-preview-origin request header when previews are configured.

export interface CspOptions {
  development?: boolean;
  previews?: string | null;
}

const PREVIEW_ORIGIN = /^https?:\/\/\*\.[a-z0-9.-]+(:\d+)?$/i;

export function contentSecurityPolicy(nonce: string, opts: CspOptions = {}): string {
  const development = opts.development ?? process.env.NODE_ENV === "development";
  const previews = opts.previews && PREVIEW_ORIGIN.test(opts.previews) ? opts.previews : null;
  return [
    "default-src 'self'",
    `script-src 'self' 'nonce-${nonce}' 'strict-dynamic'${development ? " 'unsafe-eval'" : ""}`,
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "media-src 'self' blob:",
    "font-src 'self' data:",
    `connect-src 'self' blob: data:${previews ? ` ${previews}` : ""}${development ? " ws:" : ""}`,
    "worker-src 'self' blob:",
    ...(previews ? [`frame-src ${previews}`] : []),
    "frame-ancestors 'none'",
    "base-uri 'none'",
    "form-action 'self'",
    "object-src 'none'",
  ].join("; ");
}

/** A fresh base64 nonce per document; 128 bits of randomness. */
export function generateNonce(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return btoa(String.fromCharCode(...bytes));
}
