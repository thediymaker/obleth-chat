"use client";

// A reply's Markdown, drawn: math, highlighted code, diagrams, molecules, SVG and
// CSV blocks fill in as their libraries load (lib/chat/rendered.ts), and each code
// block has Copy and Save.

import "katex/dist/katex.min.css";
import "./chat-prose.css";
import { memo, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { setTableView, tableCsv } from "@/lib/chat/figure";
import { copyText } from "@/lib/clipboard";
import { markdown } from "@/lib/chat/markdown";
import { downloadDrawing, hydrate, toggleDrawing } from "@/lib/chat/rendered";
import { cn } from "@/lib/utils";

export { markdown as markdownHtml } from "@/lib/chat/markdown";

/** What a code block's Save button hands on, for the Library's save dialog. */
export interface SaveCodeItem {
  kind: "code";
  body: string;
  language: string;
  title: string;
}

/** A title for saved code: the first name it defines, else its first line. */
export function codeTitle(body: string): string {
  return (
    (body.match(/(?:def|function|fn|func|class|const|let)\s+([A-Za-z_$][\w$]*)/) || [])[1] ||
    body
      .split("\n")
      .find((l) => l.trim())
      ?.trim()
      .slice(0, 80) ||
    "Code"
  );
}

// Diagrams and molecules follow the theme, which next-themes keeps on <html>.
function subscribeTheme(change: () => void) {
  const observer = new MutationObserver(change);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => observer.disconnect();
}
const currentTheme = () => document.documentElement.dataset.theme ?? "";

export interface MarkdownProps {
  text: string | null | undefined;
  /** Changes the HTML after it is made, such as linking [n] citations to sources. */
  cite?: (html: string) => string;
  className?: string;
  /** Saves a code block to the Library; without it the Save buttons are hidden. */
  onSaveCode?: (item: SaveCodeItem) => void;
}

export const Markdown = memo(function Markdown({ text, cite, className, onSaveCode }: MarkdownProps) {
  const ref = useRef<HTMLDivElement>(null);
  // Bumped when a library or drawing finished, so the HTML is made again with it.
  const [version, setVersion] = useState(0);
  const theme = useSyncExternalStore(subscribeTheme, currentTheme, () => "");
  const source = String(text ?? "");
  // eslint-disable-next-line react-hooks/exhaustive-deps -- version and theme change
  // what markdown() returns
  const made = useMemo(() => markdown(source), [source, version, theme]);
  const html = useMemo(() => (cite ? cite(made) : made), [made, cite]);

  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    let live = true;
    hydrate(root).then((waited) => {
      // Only draw again when something changed: a language no grammar reads would
      // otherwise ask forever.
      if (live && waited && markdown(source) !== made) setVersion((v) => v + 1);
    });
    return () => {
      live = false;
    };
  }, [html, made, source]);

  const onClick = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const el = (event.target as HTMLElement).closest<HTMLElement>("[data-action]");
      if (!el || !ref.current?.contains(el)) return;
      const action = el.dataset.action;
      // A table that can show as a figure: switch its view, or copy its numbers.
      const table = el.closest<HTMLElement>(".table-figure");
      if (table && (action === "table-view" || action === "copy-table")) {
        event.preventDefault();
        if (action === "table-view") {
          setTableView(table.dataset.key || "", el.dataset.view === "table" ? "table" : "figure");
          setVersion((v) => v + 1);
        } else void copyText(tableCsv([...table.querySelectorAll("tr")].map((row) => [...row.children].map((cell) => cell.textContent || ""))));
        return;
      }
      const block = el.closest<HTMLElement>(".code-block");
      if (!block) return;
      const code = block.querySelector("pre code")?.textContent ?? "";
      if (action === "copy-code") {
        event.preventDefault();
        void copyText(code);
      } else if (action === "save-code" && onSaveCode) {
        event.preventDefault();
        const label = block.querySelector(".code-label span")?.textContent || "";
        onSaveCode({ kind: "code", body: code, language: label === "text" ? "" : label, title: codeTitle(code) });
      } else if (action === "toggle-drawing" && block.dataset.kind) {
        event.preventDefault();
        toggleDrawing(block.dataset.kind, code);
        setVersion((v) => v + 1);
      } else if (action === "download-drawing") {
        event.preventDefault();
        downloadDrawing(block);
      }
    },
    [onSaveCode],
  );

  return (
    // The buttons inside are real buttons; the div only delegates their clicks.
    // eslint-disable-next-line jsx-a11y/click-events-have-key-events,
    // jsx-a11y/no-static-element-interactions
    <div ref={ref} className={cn("chat-prose", !onSaveCode && "no-save", className)} onClick={onClick} dangerouslySetInnerHTML={{ __html: html }} />
  );
});
