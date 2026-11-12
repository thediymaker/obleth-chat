// What a console printed, as a person would read it on a screen: terminal escape
// codes (colours, cursor moves, titles) dropped, and lines redrawn in place with a
// carriage return or backspace (progress bars, spinners, apt's status line) kept as
// they last stood. The model reads device output through this; the monitor draws
// the raw output in a real terminal instead.

const ESC = "\x1b";
/** Longest unfinished escape held back for the next read; past this it is text. */
const HOLD_MAX = 4096;

/** Where the escape sequence at `i` (an ESC) ends, or -1 when it is not finished yet. */
function escapeEnd(text: string, i: number): number {
  const next = text[i + 1];
  if (next === undefined) return -1;
  if (next === "[") {
    // CSI: parameter and intermediate bytes, then one final byte.
    for (let j = i + 2; j < text.length; j++) {
      const c = text.charCodeAt(j);
      if (c >= 0x40 && c <= 0x7e) return j + 1;
      if (c < 0x20 || c > 0x3f) return j; // malformed; stop before it
    }
    return -1;
  }
  if (next === "]" || next === "P" || next === "X" || next === "^" || next === "_") {
    // OSC, DCS and the like: a string ended by BEL or ESC \.
    for (let j = i + 2; j < text.length; j++) {
      if (text[j] === "\x07") return j + 1;
      if (text[j] === ESC) return text[j + 1] === undefined ? -1 : text[j + 1] === "\\" ? j + 2 : j;
    }
    return -1;
  }
  // ESC, any intermediate bytes, one final byte: charset picks, keypad modes, save/restore cursor.
  let j = i + 1;
  while (j < text.length && text.charCodeAt(j) >= 0x20 && text.charCodeAt(j) <= 0x2f) j++;
  return j < text.length ? j + 1 : -1;
}

/**
 * How much of `text` can be cleaned now: everything before an escape sequence that
 * is still arriving at its end. The rest waits for the next read, so a code split
 * across two reads doesn't leak its tail as text.
 */
export function completeUpTo(text: string): number {
  const at = text.lastIndexOf(ESC);
  if (at < 0 || text.length - at > HOLD_MAX) return text.length;
  return escapeEnd(text, at) < 0 ? at : text.length;
}

/** `text` as plain lines. Finished lines lose trailing blanks; the last (a prompt, say) is kept as is. */
export function plainText(text: string): string {
  if (!/[\x00-\x08\x0b-\x1f\x7f-\x9f]/.test(text)) return text;
  const lines: string[] = [];
  let line: string[] = [];
  let col = 0;
  const put = (ch: string) => {
    while (line.length < col) line.push(" ");
    line[col++] = ch;
  };
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    const code = text.charCodeAt(i);
    if (ch === ESC) {
      const end = escapeEnd(text, i);
      if (end < 0) break; // unfinished at the very end: drop it
      if (text[i + 1] === "[") {
        const params = text.slice(i + 2, end - 1);
        const n = parseInt(params, 10);
        switch (text[end - 1]) {
          case "K": // erase in line
            if (!n) line.length = Math.min(line.length, col);
            else if (n === 1) for (let k = 0; k < Math.min(col + 1, line.length); k++) line[k] = " ";
            else if (n === 2) line = [];
            break;
          case "G": // to a column
            col = Math.max(0, (n || 1) - 1);
            break;
          case "C": // forward
            col += n || 1;
            break;
          case "D": // back
            col = Math.max(0, col - (n || 1));
            break;
        }
      }
      i = end - 1;
    } else if (ch === "\n") {
      lines.push(line.join("").trimEnd());
      line = [];
      col = 0;
    } else if (ch === "\r") {
      col = 0;
    } else if (ch === "\b") {
      col = Math.max(0, col - 1);
    } else if (ch === "\t" || (code >= 0x20 && code !== 0x7f && (code < 0x80 || code > 0x9f))) {
      put(ch);
    }
    // Other control characters (bell, NUL, C1 codes) print nothing.
  }
  lines.push(line.join(""));
  return lines.join("\n");
}
