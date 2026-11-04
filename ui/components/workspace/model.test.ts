import { describe, expect, it } from "vitest";
import { csvRows, csvTable, diskUsage, formatBytes, previewReload, statusLabel, terminalUrl, treeItem } from "./model";

describe("previewReload", () => {
  it("reloads a running app and otherwise wakes the workspace", () => {
    expect(previewReload({ status: "running", previewPort: 3000 })).toEqual({ label: "Reload preview", wake: false, disabled: false });
    expect(previewReload({ status: "stopped", previewPort: null })).toEqual({ label: "Wake the workspace", wake: true, disabled: false });
    expect(previewReload({ status: "running", previewPort: null })).toEqual({ label: "Start the preview", wake: true, disabled: false });
    expect(previewReload({ status: "error" }).label).toBe("Start the workspace again");
  });
  it("waits while the workspace is already waking", () => {
    expect(previewReload({ status: "starting" }).disabled).toBe(true);
    expect(previewReload({ status: "stopped" }, true).disabled).toBe(true);
  });
});

describe("the file viewer", () => {
  it("reads quoted CSV fields and line endings", () => {
    expect(csvRows('a,"b,c"\r\n"say ""hi""",2\n3', 10)).toEqual([
      ["a", "b,c"],
      ['say "hi"', "2"],
      ["3"],
    ]);
  });
  it("shows the first 200 rows of a CSV", () => {
    const text = Array.from({ length: 300 }, (_, i) => `${i},x`).join("\n");
    const t = csvTable(text);
    expect(t.header).toEqual(["0", "x"]);
    expect(t.body).toHaveLength(199);
    expect(t.more).toBe(true);
    expect(csvTable("a\n1").more).toBe(false);
    expect(csvTable("a\n1", true).more).toBe(true);
  });
  it("sizes and nests files", () => {
    expect(formatBytes(12)).toBe("12 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(3 * 1048576)).toBe("3.0 MB");
    expect(treeItem({ path: "src/app/page.tsx" })).toEqual({ depth: 2, name: "page.tsx" });
  });
});

describe("the bar", () => {
  it("names each status", () => {
    expect(["starting", "running", "stopped", "error", "other"].map((status) => statusLabel({ status }))).toEqual(["Starting", "Running", "Asleep", "Error", "other"]);
  });
  it("shows disk use against the limit, and when it is full", () => {
    expect(diskUsage(null, 10 * 2 ** 30)).toBeNull();
    expect(diskUsage(2 ** 30, 0)).toBeNull();
    expect(diskUsage(1.5 * 2 ** 30, 10 * 2 ** 30)).toEqual({ text: "1.5 / 10 GB", of: "1.5 of 10 GB used", full: false });
    expect(diskUsage(11 * 2 ** 30, 10 * 2 ** 30)?.full).toBe(true);
  });
  it("opens the terminal on the same host, over wss on https", () => {
    expect(terminalUrl("a b", { protocol: "https:", host: "chat.example.edu" })).toBe("wss://chat.example.edu/api/conversations/a%20b/workspace/terminal");
    expect(terminalUrl("c1", { protocol: "http:", host: "127.0.0.1:3104" })).toBe("ws://127.0.0.1:3104/api/conversations/c1/workspace/terminal");
  });
});
