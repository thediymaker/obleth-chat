import { describe, expect, it } from "vitest";
import { ago, codeHref, codeRoute, conflictRegions, diffRows, resolvePrompt, slug, syncSays } from "./model";

describe("diffRows", () => {
  it("numbers a diff's lines by the new file", () => {
    const rows = diffRows("diff --git a/p.py b/p.py\nindex 1..2 100644\n--- a/p.py\n+++ b/p.py\n@@ -1,3 +1,3 @@ def f():\n a\n-b\n+c\n d\n");
    expect(rows.map((r) => [r.kind, r.n, r.text])).toEqual([
      ["hunk", undefined, "line 1 · def f():"],
      ["ctx", 1, "a"],
      ["del", "", "b"],
      ["add", 2, "c"],
      ["ctx", 3, "d"],
    ]);
  });
  it("notes binary files and stops at the limit", () => {
    expect(diffRows("Binary files a/x.png and b/x.png differ")).toEqual([{ kind: "note", text: "A binary file changed." }]);
    const long = "@@ -1 +1,9 @@\n" + Array.from({ length: 9 }, (_, i) => `+${i}`).join("\n");
    const rows = diffRows(long, 4);
    expect(rows).toHaveLength(5);
    expect(rows.at(-1)).toEqual({ kind: "more", text: "The rest of this diff is not shown." });
  });
});

describe("conflictRegions", () => {
  it("splits a conflict into each side, up to a limit", () => {
    const text = "x\n<<<<<<< HEAD\nmine\n=======\ntheirs\nmore\n>>>>>>> project\ny\n<<<<<<< HEAD\n||||||| base\nold\n=======\n>>>>>>> project\n";
    expect(conflictRegions(text)).toEqual([
      { line: 2, mine: ["mine"], theirs: ["theirs", "more"] },
      { line: 9, mine: [], theirs: [] },
    ]);
    expect(conflictRegions(text, 1)).toHaveLength(1);
  });
});

describe("the Git tab's words", () => {
  it("says what Sync will do", () => {
    expect(syncSays({ behind: 2, ahead: 0 }, 1)).toBe("Sync saves your changes, brings in the project’s 2 new commits, and shares yours with everyone.");
    expect(syncSays({ behind: 1 }, 0)).toBe("Sync brings in the project’s 1 new commit.");
    expect(syncSays({ ahead: 1 }, 0)).toBe("Sync saves your changes and shares them with everyone in the project.");
    expect(syncSays({}, 0)).toBe("Everything here is in the project.");
  });
  it("asks the model to combine the conflicted files", () => {
    expect(resolvePrompt(["a.py"])).toMatch(/^Sync paused because `a.py` was changed/);
    expect(resolvePrompt(["a.py", "b.py"])).toMatch(/^Sync paused because `a.py`, `b.py` were changed/);
  });
  it("tells time from seconds or dates", () => {
    const now = Date.UTC(2026, 10, 3, 12);
    expect(ago(now / 1000 - 20, now)).toBe("just now");
    expect(ago(now / 1000 - 600, now)).toBe("10 min ago");
    expect(ago(new Date(now - 3 * 3600_000).toISOString(), now)).toBe("3 h ago");
    expect(ago(now / 1000 - 30 * 3600, now)).toBe("yesterday");
    expect(ago(now / 1000 - 5 * 86400, now)).toBe("5 d ago");
    expect(ago(null)).toBe("");
  });
  it("names a repository after the project", () => {
    expect(slug("My Lab's Site!")).toBe("my-lab-s-site");
    expect(slug("")).toBe("project");
  });
});

describe("the Code tab's paths", () => {
  it("reads the view from the path", () => {
    expect(codeRoute([])).toEqual({ view: "overview" });
    expect(codeRoute(["history"])).toEqual({ view: "history", sha: "" });
    expect(codeRoute(["history", "abc123"])).toEqual({ view: "history", sha: "abc123" });
    expect(codeRoute(["files", "src%2Fapp.py"])).toEqual({ view: "files", path: "src/app.py" });
    expect(codeRoute(["files", "src", "app.py"])).toEqual({ view: "files", path: "src/app.py" });
    expect(codeRoute(["compare", "abc...main"])).toEqual({ view: "compare", from: "abc", to: "main" });
    expect(codeRoute(["compare", "abc"])).toEqual({ view: "compare", from: "abc", to: "main" });
  });
  it("writes paths the view reads back", () => {
    expect(codeHref("p 1")).toBe("/projects/p%201/code");
    expect(codeHref("p", "history")).toBe("/projects/p/code/history");
    expect(codeHref("p", { view: "files", path: "src/app.py" })).toBe("/projects/p/code/files/src%2Fapp.py");
    const compare = codeHref("p", { view: "compare", from: "abc", to: "main" });
    expect(compare).toBe("/projects/p/code/compare/abc...main");
    expect(codeRoute(compare.split("/code/")[1]!.split("/"))).toEqual({ view: "compare", from: "abc", to: "main" });
  });
});
