import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { FeedbackTab } from "./feedback-tab";
import { forgetKept } from "./state";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const SENT = {
  items: [
    { id: "f1", kind: "problem", message: "The preview <stays> blank", page: "/chat/abc", version: "1.0.2", done: false, at: "2026-11-04T12:00:00Z", from: { username: "bo", name: "Bo" } },
    { id: "f2", kind: "idea", message: "Voice input", page: null, version: "1.0.2", done: false, at: "2026-11-04T11:00:00Z", from: null },
  ],
  open: { problem: 1, idea: 1, other: 0 },
};

test("the Feedback tab lists sent feedback, filters it, marks it done and sets the button", async () => {
  forgetKept();
  const calls = mockApi((path, options) => {
    if (path === "/api/admin/feedback/button") return options.method === "PUT" ? options.body : { mode: "sidebar" };
    if (path.startsWith("/api/admin/feedback/sent?")) return SENT;
    return undefined;
  });
  renderPage(<FeedbackTab />);
  const problem = (await screen.findByText("The preview <stays> blank")).closest("li")!;
  expect(problem).toHaveTextContent("Bo (bo) · /chat/abc · v1.0.2");
  expect((await screen.findByText("Voice input")).closest("li")).toHaveTextContent("Anonymous");
  expect(screen.getByText(/Open: 1 problem, 1 idea, 0 other/)).toBeInTheDocument();
  expect(calls.some((c) => c.path === "/api/admin/feedback/sent?done=false")).toBe(true);

  await userEvent.click(screen.getByRole("button", { name: "An idea" }));
  await waitFor(() => expect(calls.at(-1)!.path).toBe("/api/admin/feedback/sent?done=false&kind=idea"));

  await userEvent.click(within(problem).getByRole("button", { name: "Mark done" }));
  await waitFor(() => expect(calls.find((c) => c.method === "PATCH")).toEqual({ path: "/api/admin/feedback/sent/f1", method: "PATCH", body: { done: true } }));

  expect(await screen.findByRole("button", { name: "In the sidebar" })).toHaveAttribute("aria-pressed", "true");
  await userEvent.click(screen.getByRole("button", { name: "Highlighted" }));
  await waitFor(() => expect(calls.find((c) => c.method === "PUT")).toEqual({ path: "/api/admin/feedback/button", method: "PUT", body: { mode: "highlighted" } }));
  expect(await screen.findByRole("button", { name: "Highlighted" })).toHaveAttribute("aria-pressed", "true");
});

test("the Feedback tab shows models, reasons and comments for the chosen period", async () => {
  forgetKept();
  const calls = mockApi((path) =>
    path.startsWith("/api/admin/feedback?")
      ? {
          days: 30,
          models: [{ model: "model-a", up: 3, down: 1, upShare: 0.75 }],
          reasons: [{ reason: "too_long", count: 1 }],
          comments: [{ model: "model-a", rating: -1, reasons: ["too_long"], comment: "Too <wordy>", at: "2026-10-01T12:00:00Z" }],
        }
      : path === "/api/admin/feedback/button"
        ? { mode: "sidebar" }
        : SENT,
  );
  renderPage(<FeedbackTab />);
  await userEvent.click(screen.getByRole("button", { name: "Reply ratings" }));
  const row = (await screen.findByText("model-a", { selector: "strong" })).closest("tr")!;
  expect(row).toHaveTextContent("model-a3175%");
  expect(calls.filter((c) => c.path.startsWith("/api/admin/feedback?")).map((c) => c.path)).toEqual(["/api/admin/feedback?days=30"]);
  expect(screen.getByRole("link", { name: /Download CSV/ })).toHaveAttribute("href", "/api/admin/feedback/models.csv?days=30");
  expect(screen.getAllByText(/Too long/).length).toBeGreaterThan(0);
  expect(screen.getByText("Too <wordy>")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "90 days" }));
  await waitFor(() => expect(calls.at(-1)!.path).toBe("/api/admin/feedback?days=90"));
  expect(screen.getByRole("button", { name: "90 days" })).toHaveAttribute("aria-pressed", "true");
});
