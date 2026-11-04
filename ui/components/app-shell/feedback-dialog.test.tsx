import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { FeedbackDialog } from "./feedback-dialog";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ usePathname: () => "/projects/p1" }));

test("feedback is sent with its kind, the page and the contact choice, then the dialog closes", async () => {
  const calls = mockApi(() => ({ id: "f1" }));
  const onOpenChange = vi.fn();
  renderPage(<FeedbackDialog open onOpenChange={onOpenChange} />);
  const send = screen.getByRole("button", { name: "Send" });
  expect(send).toBeDisabled();
  await userEvent.click(screen.getByRole("button", { name: "An idea" }));
  await userEvent.type(screen.getByLabelText("What would you like to see?"), "  Voice input  ");
  await userEvent.click(screen.getByLabelText(/You can contact me about this/));
  await userEvent.click(send);
  await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  expect(calls).toEqual([{ path: "/api/feedback", method: "POST", body: { kind: "idea", message: "Voice input", contact: true, page: "/projects/p1" } }]);
});

test("a refusal from the server stays in the dialog", async () => {
  mockApi(() => new Error("You can send 20 a day. Try again tomorrow."));
  const onOpenChange = vi.fn();
  renderPage(<FeedbackDialog open onOpenChange={onOpenChange} />);
  await userEvent.type(screen.getByLabelText("What happened?"), "The preview stays blank");
  await userEvent.click(screen.getByRole("button", { name: "Send" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("You can send 20 a day");
  expect(onOpenChange).not.toHaveBeenCalled();
});
