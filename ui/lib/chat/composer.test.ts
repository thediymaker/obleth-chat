import { expect, test } from "vitest";
import { clipboardFiles, pastesAsFile, pastesAsPicture, promptCount } from "./composer";

const box = (value: string, selectionStart = value.length, selectionEnd = value.length) => ({ value, selectionStart, selectionEnd });

test("long pastes, or ones that would pass the box's limit, are attached as files", () => {
  expect(pastesAsFile("a".repeat(4000), box(""))).toBe(false);
  expect(pastesAsFile("a".repeat(4001), box(""))).toBe(true);
  expect(pastesAsFile("a".repeat(3000), box("b".repeat(9500)))).toBe(true);
  // Replacing a selection frees what it covered.
  expect(pastesAsFile("a".repeat(3000), box("b".repeat(9500), 0, 1000))).toBe(false);
  expect(promptCount(10799)).toBe("");
  expect(promptCount(12000)).toBe(`${(12000).toLocaleString()} / ${(12000).toLocaleString()}`);
});

const png = new File(["x"], "image.png", { type: "image/png" });
const clip = (o: { files?: File[]; items?: File[]; text?: string; html?: string }) => ({
  files: o.files ?? [],
  items: (o.items ?? []).map((f) => ({ kind: "file", getAsFile: () => f })),
  getData: (type: string) => (type === "text/plain" ? (o.text ?? "") : type === "text/html" ? (o.html ?? "") : ""),
});

test("a pasted picture is found whether the browser lists it as a file or only as an item", () => {
  expect(clipboardFiles(clip({ files: [png] }))).toEqual([png]);
  expect(clipboardFiles(clip({ items: [png] }))).toEqual([png]);
  expect(clipboardFiles(clip({ files: [png], items: [png] }))).toEqual([png]);
  expect(clipboardFiles(clip({ files: [new File([], "empty.png", { type: "image/png" })] }))).toEqual([]);
  expect(clipboardFiles(null)).toEqual([]);
});

test("a copied picture is the picture even with its address beside it; text that carries a picture of itself stays text", () => {
  expect(pastesAsPicture(clip({ files: [png] }))).toBe(true);
  expect(pastesAsPicture(clip({ files: [png], text: "https://example.org/a/chart.png" }))).toBe(true);
  expect(pastesAsPicture(clip({ files: [png], text: "chart", html: '<meta charset="utf-8"><img src="https://example.org/chart.png" alt="chart">' }))).toBe(true);
  // Cells copied from a spreadsheet: the text is what was copied.
  expect(pastesAsPicture(clip({ files: [png], text: "a\tb\n1\t2", html: "<table><tr><td>a</td></tr></table>" }))).toBe(false);
  expect(pastesAsPicture(clip({ files: [png], text: "Two words" }))).toBe(false);
  expect(pastesAsPicture(clip({ text: "https://example.org/chart.png" }))).toBe(false);
  expect(pastesAsPicture(clip({ files: [new File(["x"], "notes.pdf", { type: "application/pdf" })], text: "https://example.org/notes.pdf" }))).toBe(false);
});
