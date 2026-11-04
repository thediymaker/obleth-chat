import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { fakeStore } from "@/components/workspace/test-store";
import { BrowserCard } from "./browser-card";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("@/components/chat/chat-provider", () => import("@/components/workspace/test-store"));
vi.mock("@/lib/branding-context", () => ({ useBranding: () => ({ assistantName: "Sparky" }) }));

const request = { id: "m1", role: "assistant", content: "", status: "completed", activity: [{ kind: "browser-request", reason: "To read the paper" }] };
const handoff = { id: "m2", role: "assistant", content: "", status: "completed", activity: [{ kind: "browser-handoff", reason: "Sign in to the library" }] };

beforeEach(() => {
  fakeStore();
});

describe("the browser cards", () => {
  it("turns the browser on and tells the model to go ahead", async () => {
    const store = fakeStore();
    const calls = mockApi((_p, o) => ({ on: !!(o.body as any)?.on }));
    renderPage(<BrowserCard m={request} index={0} isLast streaming={false} />);
    expect(screen.getByText("This needs the web")).toBeInTheDocument();
    expect(screen.getByText("To read the paper")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Turn on browser" }));
    await waitFor(() => expect(store.send).toHaveBeenCalledWith("The browser is on. Go ahead."));
    expect(calls).toContainEqual({ path: "/api/conversations/c1/browser", method: "PUT", body: { on: true } });
    expect(await screen.findByText("Turned on for this chat.")).toBeInTheDocument();
  });

  it("closes the request with Not now, and on older messages", () => {
    const { rerender } = renderPage(<BrowserCard m={request} index={0} isLast streaming={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.getByText("Not turned on. Use the Browser button below whenever you want it.")).toBeInTheDocument();
    rerender(<BrowserCard m={{ ...request, id: "m9" }} index={0} isLast={false} streaming={false} />);
    expect(screen.queryByRole("button", { name: "Turn on browser" })).not.toBeInTheDocument();
  });

  it("asks the person to take their turn, and hands back", async () => {
    const store = fakeStore();
    store.browser.update("c1", { on: true, controller: "user" });
    const calls = mockApi(() => ({}));
    renderPage(<BrowserCard m={handoff} index={0} isLast streaming={false} />);
    expect(screen.getByText("Your turn in the browser")).toBeInTheDocument();
    expect(screen.getByText(/Sparky is paused and sees nothing/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show browser" }));
    expect(store.showTab).toHaveBeenCalledWith("c1", "browser");
    fireEvent.click(screen.getByRole("button", { name: "Hand back" }));
    await waitFor(() => expect(store.send).toHaveBeenCalledWith("I’m done in the browser. Carry on."));
    expect(calls).toContainEqual({ path: "/api/conversations/c1/browser/control", method: "POST", body: { controller: "model" } });
    expect(await screen.findByText("Handed back.")).toBeInTheDocument();
  });

  it("shows nothing for replies without browser cards", () => {
    const { container } = renderPage(<BrowserCard m={{ role: "assistant", content: "hi" }} index={0} isLast streaming={false} />);
    expect(container).toBeEmptyDOMElement();
  });
});
