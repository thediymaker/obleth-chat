import { describe, expect, it } from "vitest";
import { cellNumber, figureOf, firstView, setTableView, tableCsv, tableKey, tableView } from "./figure";
import { markdown } from "./markdown";

describe("cellNumber", () => {
  it("reads plain numbers, thousands, units and percent", () => {
    expect(cellNumber("1,840")).toBe(1840);
    expect(cellNumber(" 4.2 s ")).toBe(4.2);
    expect(cellNumber("38%")).toBe(38);
    expect(cellNumber("$12")).toBe(12);
    expect(cellNumber("4920 tok/s")).toBe(4920);
  });
  it("leaves words, ranges, negatives and blanks alone", () => {
    for (const text of ["queued", "—", "", "3-4", "-2", "×1.80", "v1.2.3"]) expect(cellNumber(text)).toBeNull();
  });
});

describe("figureOf", () => {
  const head = ["max-num-seqs", "decode tok/s", "gain"];
  const rows = [
    ["32", "1,840", "—"],
    ["64", "3,310", "×1.80"],
    ["128", "4,920", "×1.49"],
    ["256", "queued", "—"],
  ];
  it("draws the first column of numbers, and keeps a cell that is still on the way", () => {
    expect(figureOf(head, rows)).toEqual({ column: 1, others: [2], values: [1840, 3310, 4920, null], max: 4920 });
  });
  it("needs two rows, a name for each row and at least two numbers", () => {
    expect(figureOf(head, rows.slice(0, 1))).toBeNull();
    expect(figureOf(head, [["", "1"], ["b", "2"]])).toBeNull();
    expect(figureOf(["Name", "Role"], [["Ada", "Lead"], ["Lin", "Review"]])).toBeNull();
  });
  it("does not draw years, ids or other numbers that count nothing", () => {
    expect(figureOf(["Event", "Year"], [["Launch", "2019"], ["Rewrite", "2024"]])).toBeNull();
    expect(figureOf(["Job", "ID"], [["sweep-32", "4821970"], ["sweep-64", "4821971"]])).toBeNull();
    expect(figureOf(["Paper", "Published"], [["A", "2019"], ["B", "2024"]])).toBeNull();
  });
  it("skips a column of ids for the numbers after it", () => {
    expect(figureOf(["Job", "ID", "Minutes"], [["a", "101", "9"], ["b", "102", "12"]])?.column).toBe(2);
  });
  it("leaves long tables as tables", () => {
    expect(figureOf(["n", "v"], Array.from({ length: 17 }, (_, i) => [`r${i}`, String(i + 1)]))).toBeNull();
  });
});

describe("the view a table shows", () => {
  it("starts as a figure for a short list of results, and as a table when wider", () => {
    expect(firstView(["a", "b", "c"])).toBe("figure");
    expect(firstView(["a", "b", "c", "d"])).toBe("table");
  });
  it("keeps the view it was switched to", () => {
    const head = ["Setting", "Speed"];
    expect(tableView(head)).toBe("figure");
    setTableView(tableKey(head), "table");
    expect(tableView(head)).toBe("table");
    setTableView(tableKey(head), "figure");
  });
});

describe("a reply's table", () => {
  const source = "| max-num-seqs | tok/s |\n|---|---|\n| 32 | 1,840 |\n| 64 | 3,310 |\n| 256 | queued |\n";
  it("comes with bars, the table itself and the switch", () => {
    const html = markdown(source);
    expect(html).toContain('class="table-figure" data-view="figure"');
    expect(html).toContain('class="fb-bar" style="width:100%"');
    expect(html).toContain("fb-pending");
    expect(html).toContain("<table>");
    expect(html).toContain('data-action="table-view" data-view="table" aria-pressed="false"');
    expect(html).toContain("tok/s by max-num-seqs");
  });
  it("stays a plain table when it has nothing to draw", () => {
    const html = markdown("| Name | Role |\n|---|---|\n| Ada | Lead |\n| Lin | Review |\n");
    expect(html).not.toContain("table-figure");
    expect(html).toContain('<div class="table-scroll"');
  });
  it("escapes what the model wrote", () => {
    const html = markdown('| <b>x</b> | "n" |\n|---|---|\n| <i>a</i> | 1 |\n| b | 2 |\n');
    expect(html).not.toContain("<b>");
    expect(html).not.toContain("<i>");
  });
});

describe("tableCsv", () => {
  it("quotes cells with commas or quotes", () => {
    expect(tableCsv([["a", "b"], ["1,840", 'say "hi"']])).toBe('a,b\n"1,840","say ""hi"""');
  });
});
