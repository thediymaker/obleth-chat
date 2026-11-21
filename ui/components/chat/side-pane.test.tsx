import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SidePane } from "./side-pane";

const ended = vi.fn();
vi.mock("@/components/workspace/workspace-panel", () => ({
  WorkspacePanel: ({ tab }: { tab: string }) => {
    useEffect(() => ended, []);
    return (
      <div>
        <iframe title="Preview" src="https://preview.example/app" />
        <span>Workspace {tab}</span>
      </div>
    );
  },
}));
vi.mock("@/components/code/code-panel", () => ({ CodePanel: () => <p>Git panel</p>, CodeTabLabel: () => <>Git</> }));
vi.mock("@/components/browser/browser-panel", () => ({ BrowserPanel: () => <p>Browser panel</p> }));

const pane = { open: true, tab: "terminal" };
const store = {
  paneVisible: () => pane.open,
  pane: () => pane,
  paneTabs: () => ["preview", "files", "terminal", "code"],
  showTab: vi.fn((_id: string, tab: string) => {
    pane.tab = tab;
    rerender();
  }),
  closePane: vi.fn(),
};
vi.mock("./chat-provider", () => ({ useChat: () => store }));

let rerender = () => {};
function show() {
  const view = render(<SidePane id="c1" />);
  rerender = () => view.rerender(<SidePane id="c1" />);
  return view;
}

beforeEach(() => {
  pane.open = true;
  pane.tab = "terminal";
  ended.mockClear();
  store.showTab.mockClear();
});

describe("the chat's side pane", () => {
  it("keeps the workspace mounted behind the Git tab, so the terminal and preview carry on", () => {
    show();
    const frame = screen.getByTitle("Preview");
    fireEvent.mouseDown(screen.getByRole("tab", { name: "Git" }));
    expect(store.showTab).toHaveBeenLastCalledWith("c1", "code");
    expect(screen.getByText("Git panel")).toBeVisible();
    expect(screen.getByText("Workspace terminal").closest("[role=tabpanel]")).toHaveAttribute("hidden");
    fireEvent.mouseDown(screen.getByRole("tab", { name: "Terminal" }));
    expect(screen.getByText("Workspace terminal")).toBeVisible();
    expect(ended).not.toHaveBeenCalled();
    expect(screen.getByTitle("Preview")).toBe(frame);
    expect(frame).toHaveAttribute("src", "https://preview.example/app");
  });

  it("moves between tabs with the arrow keys", async () => {
    show();
    const terminal = screen.getByRole("tab", { name: "Terminal" });
    terminal.focus();
    fireEvent.keyDown(terminal, { key: "ArrowRight" });
    await waitFor(() => expect(screen.getByRole("tab", { name: "Git" })).toHaveFocus());
    expect(store.showTab).toHaveBeenLastCalledWith("c1", "code");
  });

  it("ends the workspace when the pane closes", () => {
    show();
    pane.open = false;
    rerender();
    expect(ended).toHaveBeenCalledTimes(1);
  });
});
