import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { citeHtml, KnowledgeSources, type ReplyLike } from "./chat-knowledge";
import { scopeLabel, size } from "./api";

const reply: ReplyLike = {
  role: "assistant",
  content: "Heat flows [1]. Both agree [1, 2]. Not a source [7].",
  activity: [
    {
      id: "knowledge",
      kind: "sources",
      collections: ["Thermo"],
      queries: ["heat"],
      items: [
        { n: 1, documentId: "d1", name: "Lecture.pdf", page: 4, excerpt: "Heat flows from hot to cold." },
        { n: 2, documentId: "d2", name: "Notes.md", page: null, section: "Laws › Second <law>", excerpt: "Entropy <rises>." },
        { n: 3, documentId: "d2", name: "Notes.md", page: null, excerpt: "Unused." },
      ],
    },
  ],
};

describe("citations", () => {
  it("link to the page they came from, and code is left alone", () => {
    const html = citeHtml("<p>Heat flows [1]. Both agree [1, 2]. Not a source [7].</p><pre><code>a[1]</code></pre>", reply);
    expect(html).toMatch(/<a class="cite" href="\/api\/knowledge\/documents\/d1\/file#page=4"[^>]*title="Lecture.pdf, page 4">1<\/a>/);
    expect(html).toMatch(/<a class="cite" href="\/api\/knowledge\/documents\/d2\/file"[^>]*title="Notes.md, Laws › Second &lt;law&gt;">2<\/a>/);
    expect(html).toMatch(/Not a source \[7\]/);
    expect(html).toMatch(/<code>a\[1\]<\/code>/);
    expect(citeHtml("<p>x [1]</p>", { role: "assistant", content: "x [1]" })).toBe("<p>x [1]</p>");
  });

  it("a saved item opens in Library, in the same tab", () => {
    const saved: ReplyLike = { role: "assistant", activity: [{ kind: "sources", items: [{ n: 1, documentId: "s1", name: "Note", excerpt: "", saved: true }] }] };
    const html = citeHtml("<p>See [1]</p>", saved);
    expect(html).toContain('href="/library/s1"');
    expect(html).toContain('title="Saved: Note"');
    expect(html).not.toContain("target=");
  });
});

describe("sources under a reply", () => {
  it("list cited passages first and keep excerpts as text", async () => {
    render(<KnowledgeSources message={reply} />);
    const toggle = screen.getByRole("button", { name: /Searched Thermo for “heat” · 3 passages from 2 documents/ });
    await userEvent.click(toggle);
    expect(screen.getByText("Entropy <rises>.")).toBeInTheDocument();
    expect(screen.getByText(/· Laws › Second <law>/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Open at page/ })).toHaveAttribute("href", "/api/knowledge/documents/d1/file#page=4");
    // The uncited passage waits behind its own toggle.
    expect(screen.queryByText("Unused.")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /1 more passage found but not cited/ }));
    expect(screen.getByText("Unused.")).toBeInTheDocument();
  });

  it("say what a reply is searching while it runs, and nothing without a search", () => {
    const live: ReplyLike = {
      role: "assistant",
      activity: [
        { kind: "sources", collections: ["Thermo"], queries: [], items: [] },
        { tool: "search_knowledge", status: "running", summary: "Searched for “entropy”" },
      ],
    };
    render(<KnowledgeSources message={live} />);
    expect(screen.getByText("Searching Thermo for “entropy”")).toBeInTheDocument();
    const { container } = render(<KnowledgeSources message={{ role: "assistant", content: "hi" }} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("a search that found nothing is one line", () => {
    render(<KnowledgeSources message={{ role: "assistant", activity: [{ kind: "sources", collections: ["Thermo"], queries: ["x"], items: [] }] }} />);
    expect(screen.getByText("Searched Thermo for “x” · nothing matched")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});

describe("collection helpers", () => {
  it("sizes and sharing labels", () => {
    expect(size(512)).toBe("512 B");
    expect(size(2048)).toBe("2 KB");
    expect(size(5 * 1048576)).toBe("5.0 MB");
    expect(size(3 * 1073741824)).toBe("3.00 GB");
    expect(scopeLabel({ scope: "group", groupName: "Lab" })).toBe("Lab");
    expect(scopeLabel({ scope: "everyone", groupName: null })).toBe("Everyone");
    expect(scopeLabel({ scope: "personal", groupName: null })).toBe("Personal");
  });
});
