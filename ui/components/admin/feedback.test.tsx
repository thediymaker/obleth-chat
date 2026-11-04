import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { FeedbackTab } from "./feedback-tab";
import { forgetKept } from "./state";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

test("the Feedback tab shows models, reasons and comments for the chosen period", async () => {
  forgetKept();
  const calls = mockApi(() => ({
    days: 30,
    models: [{ model: "model-a", up: 3, down: 1, upShare: 0.75 }],
    reasons: [{ reason: "too_long", count: 1 }],
    comments: [{ model: "model-a", rating: -1, reasons: ["too_long"], comment: "Too <wordy>", at: "2026-10-01T12:00:00Z" }],
  }));
  renderPage(<FeedbackTab />);
  const row = (await screen.findByText("model-a", { selector: "strong" })).closest("tr")!;
  expect(row).toHaveTextContent("model-a3175%");
  expect(calls.map((c) => c.path)).toEqual(["/api/admin/feedback?days=30"]);
  expect(screen.getByRole("link", { name: /Download CSV/ })).toHaveAttribute("href", "/api/admin/feedback/models.csv?days=30");
  expect(screen.getAllByText(/Too long/).length).toBeGreaterThan(0);
  expect(screen.getByText("Too <wordy>")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "90 days" }));
  await waitFor(() => expect(calls.at(-1)!.path).toBe("/api/admin/feedback?days=90"));
  expect(screen.getByRole("button", { name: "90 days" })).toHaveAttribute("aria-pressed", "true");
});
