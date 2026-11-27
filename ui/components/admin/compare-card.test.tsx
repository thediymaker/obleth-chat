// Administration › System › Comparing answers.

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { vi } from "vitest";
const apiRequest = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api", async (original) => ({ ...(await original<typeof import("@/lib/api")>()), api: { request: apiRequest } }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
import { polyfillLayout } from "@/components/chat/test-queries";
import { CompareCard, compareBody, compareOn, type CompareSettings } from "./compare-card";

polyfillLayout();

const off: CompareSettings = { closeCall: false, closeGap: 0.05, hard: false, sampleEvery: 0, dailyLimit: 3 };

test("the form reads as the API takes it", () => {
  const form = new FormData();
  form.set("compare-close", "on");
  form.set("compare-gap", "0.1");
  form.set("compare-sample", "20");
  form.set("compare-daily", "5");
  expect(compareBody(form)).toEqual({ closeCall: true, closeGap: 0.1, hard: false, sampleEvery: 20, dailyLimit: 5 });
});

test("it is on when any ground is", () => {
  expect(compareOn(off)).toBe(false);
  expect(compareOn({ ...off, hard: true })).toBe(true);
  expect(compareOn({ ...off, sampleEvery: 10 })).toBe(true);
});

test("it shows what is saved, keeps a value that is none of the choices, and saves a change", async () => {
  apiRequest.mockReset();
  const saved = { ...off, closeCall: true, closeGap: 0.07, sampleEvery: 10 };
  apiRequest.mockResolvedValue({ ...saved, hard: true });
  const onSaved = vi.fn();
  render(<CompareCard compare={saved} onSaved={onSaved} />);
  expect(screen.getByText("On")).toBeTruthy();
  expect(screen.getByLabelText("How close counts").textContent).toContain("Within 0.07");
  expect(screen.getByLabelText("By chance").textContent).toContain("1 reply in 10");
  fireEvent.click(screen.getByLabelText("When the request is hard"));
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(onSaved).toHaveBeenCalled());
  expect(apiRequest).toHaveBeenCalledWith("/api/admin/compare", { method: "PATCH", body: { closeCall: true, closeGap: 0.07, hard: true, sampleEvery: 10, dailyLimit: 3 } });
});
