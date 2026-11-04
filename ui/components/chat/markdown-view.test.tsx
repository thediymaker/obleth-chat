import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { codeTitle, Markdown } from "@/components/chat/markdown-view";

describe("Markdown", () => {
  it("renders the reply as HTML", () => {
    const { container } = render(<Markdown text={"# Title\n\nSome **bold** text."} />);
    expect(container.querySelector("h1")).toHaveTextContent("Title");
    expect(container.querySelector("strong")).toHaveTextContent("bold");
  });

  it("copies a code block through onCopy", () => {
    const onCopy = vi.fn();
    render(<Markdown text={"```js\nconst a = 1;\n```"} onCopy={onCopy} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    expect(onCopy).toHaveBeenCalledWith("const a = 1;\n");
  });

  it("hides Save without onSaveCode and saves with it", () => {
    const { container, rerender } = render(<Markdown text={"```python\ndef area(r):\n    return r\n```"} />);
    expect(container.firstElementChild).toHaveClass("no-save");
    const onSaveCode = vi.fn();
    rerender(<Markdown text={"```python\ndef area(r):\n    return r\n```"} onSaveCode={onSaveCode} />);
    expect(container.firstElementChild).not.toHaveClass("no-save");
    fireEvent.click(screen.getByRole("button", { name: "Save code to Library" }));
    expect(onSaveCode).toHaveBeenCalledWith({ kind: "code", body: "def area(r):\n    return r\n", language: "python", title: "area" });
  });

  it("applies the cite transform after the Markdown", () => {
    const cite = (html: string) => html.replace("[1]", '<a class="cite">1</a>');
    const { container } = render(<Markdown text="Fact [1]." cite={cite} />);
    expect(container.querySelector("a.cite")).toHaveTextContent("1");
  });

  it("switches a drawn table to its code and back", async () => {
    const { container } = render(<Markdown text={"```csv\na,b\n1,2\n```"} />);
    const block = () => container.querySelector(".code-block.drawn")!;
    expect(block()).not.toHaveClass("show-code");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /Code/ })));
    expect(block()).toHaveClass("show-code");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /Table/ })));
    expect(block()).not.toHaveClass("show-code");
  });

  it("highlights code once the highlighter has loaded", async () => {
    const { container } = render(<Markdown text={"```python\nreturn 1\n```"} />);
    await waitFor(() => expect(container.querySelector("code.hljs")).not.toBeNull());
    expect(container.querySelector("[data-render]")).toBeNull();
  });

  it("names saved code by what it defines, else its first line", () => {
    expect(codeTitle("function draw() {}")).toBe("draw");
    expect(codeTitle("\n  echo hi\n")).toBe("echo hi");
    expect(codeTitle("")).toBe("Code");
  });
});
