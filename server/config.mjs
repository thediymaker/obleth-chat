import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export const defaultStaticDir = fileURLToPath(new URL('../web/', import.meta.url));

/** Configuration is explicit; this preview does not read credentials or .env files. */
export function readConfig(env = process.env) {
  const host = env.HOST ?? '127.0.0.1';
  const portText = env.PORT ?? '4173';
  if (!host || !/^[a-zA-Z0-9_.:-]+$/.test(host)) {
    throw new Error('HOST must be an IP address or hostname, without a scheme or path.');
  }
  if (!/^\d+$/.test(String(portText))) {
    throw new Error('PORT must be an integer between 1 and 65535.');
  }
  const port = Number(portText);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65535) {
    throw new Error('PORT must be an integer between 1 and 65535.');
  }
  if (env.STATIC_DIR !== undefined && !env.STATIC_DIR.trim()) {
    throw new Error('STATIC_DIR must be a non-empty directory path.');
  }
  return { host, port, staticDir: env.STATIC_DIR ? resolve(env.STATIC_DIR) : defaultStaticDir };
}
