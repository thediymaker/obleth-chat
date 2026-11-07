// Reads a spreadsheet saved as CSV or TSV into rows of cells, in the browser, so a
// batch can show its columns and prompts before anything is sent.

/** The delimiter the first line uses most: comma, tab or semicolon. */
export function detectDelimiter(text: string): string {
  const firstLine = text.slice(0, text.search(/\r?\n/) === -1 ? text.length : text.search(/\r?\n/));
  let best = ",";
  let count = -1;
  for (const d of [",", "\t", ";"]) {
    let n = 0;
    let quoted = false;
    for (const c of firstLine) {
      if (c === '"') quoted = !quoted;
      else if (c === d && !quoted) n += 1;
    }
    if (n > count) {
      best = d;
      count = n;
    }
  }
  return best;
}

/**
 * Rows of cells from CSV text (RFC 4180: quoted cells may hold delimiters, line
 * breaks and doubled quotes). A leading byte order mark is dropped, and so are
 * empty lines at the end.
 */
export function parseCsv(input: string, delimiter = detectDelimiter(input)): string[][] {
  const text = input.charCodeAt(0) === 0xfeff ? input.slice(1) : input;
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"') {
        if (text[i + 1] === '"') {
          cell += '"';
          i++;
        } else quoted = false;
      } else cell += c;
    } else if (c === '"' && cell === "") {
      quoted = true;
    } else if (c === delimiter) {
      row.push(cell);
      cell = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
    } else cell += c;
  }
  if (cell !== "" || row.length) {
    row.push(cell);
    rows.push(row);
  }
  while (rows.length && rows[rows.length - 1].every((c) => c === "")) rows.pop();
  return rows;
}

export interface Sheet {
  columns: string[];
  rows: string[][];
}

/**
 * Columns and rows of a parsed file. With `headers`, the first row names the
 * columns (blank or repeated names get a number); without, they are named
 * column_1, column_2 and so on. Every row is padded to the same width.
 */
export function toSheet(cells: string[][], headers: boolean): Sheet {
  const width = cells.reduce((w, r) => Math.max(w, r.length), 0);
  const first = headers ? cells[0] ?? [] : [];
  const seen = new Map<string, number>();
  const columns = Array.from({ length: width }, (_, i) => {
    let name = (first[i] ?? "").trim().replace(/[{}]/g, "") || `column_${i + 1}`;
    const n = seen.get(name) ?? 0;
    seen.set(name, n + 1);
    if (n > 0) name = `${name}_${n + 1}`;
    return name;
  });
  const rows = (headers ? cells.slice(1) : cells).map((r) => (r.length === width ? r : [...r, ...Array(width - r.length).fill("")]));
  return { columns, rows };
}

/** `template` with each `{column}` replaced by the row's value, as the server does. */
export function renderPrompt(template: string, columns: string[], row: string[]): string {
  const index = new Map(columns.map((c, i) => [c, i]));
  return template.replace(/\{([^{}]+)\}/g, (whole, name: string) => (index.has(name) ? row[index.get(name)!] ?? "" : whole));
}

/** Placeholders in `template` that name no column. */
export function unknownPlaceholders(template: string, columns: string[]): string[] {
  const known = new Set(columns);
  const found = new Set<string>();
  for (const m of template.matchAll(/\{([^{}]+)\}/g)) if (!known.has(m[1])) found.add(m[1]);
  return [...found];
}

/** Columns `template` uses. */
export function usedColumns(template: string, columns: string[]): string[] {
  return columns.filter((c) => template.includes(`{${c}}`));
}
