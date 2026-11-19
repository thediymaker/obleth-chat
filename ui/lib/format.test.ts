import { ago, bytes, compact, count, duration, durationWords, NONE, num, when } from "./format";

test("counts group by locale and agree in number", () => {
  expect(num(1234)).toBe("1,234");
  expect(num(null)).toBe("0");
  expect(count(1, "request")).toBe("1 request");
  expect(count(2, "request")).toBe("2 requests");
  expect(count(1200, "saved item")).toBe("1,200 saved items");
  expect(count(0, "reply", "replies")).toBe("0 replies");
});

test("large amounts read at a glance with K and M", () => {
  expect([compact(950), compact(999), compact(1000), compact(1500), compact(9999), compact(10_000), compact(48_000), compact(999_999), compact(1_250_000)]).toEqual([
    "950",
    "999",
    "1.0K",
    "1.5K",
    "10.0K",
    "10K",
    "48K",
    "1000K",
    "1.3M",
  ]);
  expect(compact(null)).toBe("0");
});

test("sizes are 1024-based with one decimal past bytes", () => {
  expect([bytes(0), bytes(512), bytes(1023), bytes(1024), bytes(2048), bytes(3 * 1048576), bytes(1.5 * 1073741824)]).toEqual(["0 B", "512 B", "1023 B", "1.0 KB", "2.0 KB", "3.0 MB", "1.5 GB"]);
  expect(bytes(null)).toBe("");
});

test("spans are compact in ms and in words in seconds", () => {
  expect([duration(400), duration(12000), duration(100000), duration(null)]).toEqual(["0.4s", "12s", "1m 40s", ""]);
  expect([durationWords(1), durationWords(90), durationWords(3 * 3600 + 60), durationWords(2 * 86400)]).toEqual(["1 second", "2 minutes", "3 hours 1 minute", "2 days"]);
});

test("relative times take milliseconds or seconds, and a missing value is nothing", () => {
  const now = Date.UTC(2026, 10, 3, 15, 0, 0);
  expect(ago(now - 20_000, now)).toBe("just now");
  expect(ago(now - 600_000, now)).toBe("10 min ago");
  expect(ago(new Date(now - 3 * 3600_000).toISOString(), now)).toBe("3 h ago");
  expect(ago(now / 1000 - 30 * 3600, now, { seconds: true })).toBe("yesterday");
  expect(ago(now / 1000 - 5 * 86400, now, { seconds: true })).toBe("5 days ago");
  expect(ago(now - 40 * 86400_000, now)).toMatch(/Sep \d+/);
  expect(ago(null)).toBe("");
  expect(ago("")).toBe("");
  expect(when(null)).toBe(NONE);
});
