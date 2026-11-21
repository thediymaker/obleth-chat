// A table of results as a figure: when a reply's table is a list of things with a
// number each (a setting and its throughput, a model and its score), Chat can draw
// the numbers as bars. The table stays one click away, and the choice is kept.

export interface Figure {
  /** The column drawn as bars. */
  column: number;
  /** The other columns shown beside each bar, at most two. */
  others: number[];
  /** One per row: the number, or null where the cell has none yet. */
  values: (number | null)[];
  max: number;
}

export type TableView = "figure" | "table";

/** A cell as a number: "1,840", "4.2 s", "38%", "$12". Anything else is not one. */
export function cellNumber(text: string): number | null {
  const cell = text
    .trim()
    .replace(/^[~≈$€£]\s*/, "")
    .replace(/\s*(%|[a-zµ°/]{1,8}(?:\/[a-z]{1,4})?)$/i, "");
  if (!/^(?:\d[\d,]*(?:\.\d+)?|\.\d+)$/.test(cell)) return null;
  const value = Number(cell.replace(/,/g, ""));
  return Number.isFinite(value) ? value : null;
}

/** A cell that says its number is still on the way. */
export const pendingCell = (text: string) => /^(queued|pending|running|waiting|in progress|starting|tbd)\b/i.test(text.trim());

// Numbers that count nothing: drawing them as bars would mean nothing.
const NAMES = /^(#|no\.?|id|job ?id|pid|port|year|date|rank|version|code|zip|phone|line|page|step)$/i;
const years = (values: number[]) => values.every((v) => Number.isInteger(v) && v >= 1900 && v <= 2100);

/**
 * The figure a table can be drawn as, if it can: 2 to 16 rows, a first column that
 * names each row, and a column of numbers (a cell or two may still be on the way).
 */
export function figureOf(head: string[], rows: string[][]): Figure | null {
  if (head.length < 2 || rows.length < 2 || rows.length > 16) return null;
  if (rows.some((row) => !row[0]?.trim())) return null;
  for (let column = 1; column < head.length; column++) {
    if (NAMES.test(head[column]!.trim())) continue;
    const values = rows.map((row) => cellNumber(row[column] ?? ""));
    const known = values.filter((v): v is number => v != null);
    const missing = rows.filter((row, i) => values[i] == null && !pendingCell(row[column] ?? "")).length;
    if (known.length < 2 || missing > 1 || known.length < rows.length * 0.6) continue;
    const max = Math.max(...known);
    if (max <= 0 || years(known)) continue;
    const others = head.map((_, i) => i).filter((i) => i > 0 && i !== column).slice(0, 2);
    return { column, others, values, max };
  }
  return null;
}

/** What a table shows first: the figure for a short list of results, the table for anything wider. */
export const firstView = (head: string[]): TableView => (head.length <= 3 ? "figure" : "table");

// The view each table was switched to, by its heading row.
const views = new Map<string, TableView>();
export const tableKey = (head: string[]) => head.map((h) => h.trim()).join("\u001f");
export const tableView = (head: string[]): TableView => views.get(tableKey(head)) ?? firstView(head);
export function setTableView(key: string, view: TableView) {
  views.set(key, view);
  // Old choices go first once there are many.
  if (views.size > 200) views.delete(views.keys().next().value!);
}

/** The bars and the line under them, as HTML. `escape` makes text safe to place in it. */
export function figureHtml(head: string[], rows: string[][], figure: Figure, escape: (text: string) => string): string {
  const cells = rows
    .map((row, i) => {
      const value = figure.values[i],
        text = (row[figure.column] ?? "").trim(),
        pending = value == null && pendingCell(text);
      const bar =
        value != null
          ? `<span class="fb-bar" style="width:${Math.max(1, Math.round((value / figure.max) * 1000) / 10)}%"></span>`
          : pending
            ? '<span class="fb-bar fb-pending"></span>'
            : "";
      const more = figure.others.map((c) => `<span class="fb-more">${escape((row[c] ?? "").trim())}</span>`).join("");
      return `<span class="fb-label">${escape(row[0]!.trim())}</span><span class="fb-track">${bar}</span><span class="fb-value${pending ? " fb-live" : ""}">${escape(text || "—")}</span>${more}`;
    })
    .join("");
  const label = `${head[figure.column]!.trim()} by ${head[0]!.trim()}`;
  return `<div class="figure-bars" role="img" aria-label="${escape(`Bar chart: ${label}`)}" style="grid-template-columns:auto minmax(4rem,1fr) repeat(${1 + figure.others.length},auto)">${cells}</div>`;
}

export function captionHtml(head: string[], figure: Figure, view: TableView, escape: (text: string) => string): string {
  const button = (name: TableView, label: string) => `<button type="button" data-action="table-view" data-view="${name}" aria-pressed="${view === name}">${label}</button>`;
  return `<div class="figure-caption"><span class="figure-title">${escape(`${head[figure.column]!.trim()} by ${head[0]!.trim()}`)}</span><span class="figure-switch" role="group" aria-label="Show as">${button("figure", "Figure")}${button("table", "Table")}</span><button type="button" class="figure-copy" data-action="copy-table">Copy the data</button></div>`;
}

/** A table's cells as CSV, for Copy the data. */
export function tableCsv(rows: string[][]): string {
  const cell = (text: string) => (/[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text);
  return rows.map((row) => row.map((c) => cell(c.trim())).join(",")).join("\n");
}
