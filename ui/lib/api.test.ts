// The API client's conditional reads: the tag of a kept copy goes out as
// If-None-Match, and an unchanged answer comes back as NOT_MODIFIED.

import { expect, test } from "vitest";
import { ApiClient, ApiError, NOT_MODIFIED } from "./api";

test("a read with a validator sends its tag and learns the new one", async () => {
  const sent: (string | undefined)[] = [];
  let current = '"v1"';
  const client = new ApiClient((async (_path: string, options: RequestInit) => {
    const tag = (options.headers as Record<string, string>)["If-None-Match"];
    sent.push(tag);
    return tag === current ? new Response(null, { status: 304, headers: { etag: current } }) : Response.json({ title: current }, { headers: { etag: current } });
  }) as unknown as typeof fetch);
  const validator = { tag: "" };
  expect(await client.request("/api/conversations/c1", { validator })).toEqual({ title: '"v1"' });
  expect(validator.tag).toBe('"v1"');
  expect(await client.request("/api/conversations/c1", { validator })).toBe(NOT_MODIFIED);
  current = '"v2"';
  expect(await client.request("/api/conversations/c1", { validator })).toEqual({ title: '"v2"' });
  expect(validator.tag).toBe('"v2"');
  expect(sent).toEqual([undefined, '"v1"', '"v1"']);
});

test("an error keeps its status, message and code", async () => {
  const answers = [
    Response.json({ error: { message: "Sign in again to create a token", code: "stale_sign_in" } }, { status: 403 }),
    Response.json({ error: { message: "Not found" } }, { status: 404 }),
  ];
  const client = new ApiClient((async () => answers.shift()) as unknown as typeof fetch);
  const stale = await client.request("/api/auth/tokens", { method: "POST", body: {} }).catch((e) => e);
  expect(stale).toBeInstanceOf(ApiError);
  expect([stale.status, stale.message, stale.code]).toEqual([403, "Sign in again to create a token", "stale_sign_in"]);
  const missing = await client.request("/api/things/1").catch((e) => e);
  expect([missing.status, missing.code]).toEqual([404, undefined]);
});
