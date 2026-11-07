import { describe, expect, it } from "vitest";
import { doneOf, duration, etaSecs, percentOf, stateOf, type Batch } from "./api";
import { detectDelimiter, parseCsv, renderPrompt, toSheet, unknownPlaceholders, usedColumns } from "./csv";

const NOW = Date.parse("2026-11-07T12:00:00Z");
const batch = (over: Partial<Batch> = {}): Batch => ({
  id: "batch_1",
  name: "Abstracts",
  model: "m",
  status: "in_progress",
  total: 400,
  completed: 0,
  failed: 0,
  createdAt: "2026-11-07T11:00:00Z",
  inProgressAt: "2026-11-07T11:00:00Z",
  finishedAt: null,
  deadlineAt: null,
  lastProgressAt: null,
  waiting: false,
  window: "24h",
  error: null,
  fileName: "a.csv",
  rows: 400,
  fromChat: true,
  ...over,
});

describe("csv", () => {
  it("reads quoted cells, line breaks and doubled quotes", () => {
    const text = '﻿title,abstract\r\n"A, B","Line one\nline two"\nC,"say ""hi"""\n\n';
    expect(parseCsv(text)).toEqual([
      ["title", "abstract"],
      ["A, B", "Line one\nline two"],
      ["C", 'say "hi"'],
    ]);
  });

  it("finds tab and semicolon files", () => {
    expect(detectDelimiter("a\tb\tc\n1\t2\t3")).toBe("\t");
    expect(detectDelimiter("a;b\n1;2")).toBe(";");
    expect(parseCsv("a\tb\n1\t2")).toEqual([["a", "b"], ["1", "2"]]);
  });

  it("names columns and pads short rows", () => {
    const sheet = toSheet([["id", "", "id", "{x}"], ["1"], ["2", "b", "c", "d"]], true);
    expect(sheet.columns).toEqual(["id", "column_2", "id_2", "x"]);
    expect(sheet.rows[0]).toEqual(["1", "", "", ""]);
    expect(toSheet([["1", "2"]], false).columns).toEqual(["column_1", "column_2"]);
  });

  it("fills prompts the way the server does", () => {
    const cols = ["title", "abstract"];
    expect(renderPrompt("Sum {title}: {abstract} {other}", cols, ["T", "A"])).toBe("Sum T: A {other}");
    expect(unknownPlaceholders("{title} {other} {other}", cols)).toEqual(["other"]);
    expect(usedColumns("{abstract}", cols)).toEqual(["abstract"]);
  });
});

describe("batch states", () => {
  it("counts progress and time left", () => {
    expect(doneOf(batch({ completed: 90, failed: 10 }))).toBe(100);
    expect(percentOf(batch({ completed: 100 }))).toBe(25);
    expect(etaSecs(batch({ completed: 100 }), NOW)).toBeCloseTo(3 * 3600);
    expect(duration(3 * 3600 + 60)).toBe("3 hours 1 minute");
    expect(duration(1)).toBe("1 second");
  });

  it("says what each batch is doing", () => {
    expect(stateOf(batch({ completed: 100 }), NOW)).toMatchObject({ label: "Running", detail: "About 3 hours left at this pace" });
    const waiting = stateOf(batch({ waiting: true, lastProgressAt: "2026-11-07T11:55:00Z" }), NOW);
    expect(waiting.label).toBe("Waiting for the model");
    expect(waiting.detail).toContain("5 minutes");
    expect(stateOf(batch({ status: "completed", completed: 398, failed: 2 }), NOW)).toMatchObject({ tone: "problem", detail: "2 rows failed. The results show why." });
    expect(stateOf(batch({ status: "expired", completed: 300 }), NOW).detail).toContain("100 rows didn't run");
    expect(stateOf(batch({ status: "failed", error: "Line 2: bad" }), NOW).detail).toBe("Line 2: bad");
  });
});
