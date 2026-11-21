import { render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { DiffView } from "./diff-view";
import { diffRows } from "@/lib/code/model";

vi.mock("@/lib/code/model", async (original) => {
  const model = await original<typeof import("@/lib/code/model")>();
  return { ...model, diffRows: vi.fn(model.diffRows) };
});

it("reads a diff once, not on every repaint of the pane around it", () => {
  const text = "@@ -1,1 +1,2 @@\n-old\n+new\n+more";
  const { rerender } = render(<DiffView text={text} />);
  rerender(<DiffView text={text} />);
  expect(diffRows).toHaveBeenCalledTimes(1);
  expect(screen.getAllByRole("row").length).toBeGreaterThan(1);
  rerender(<DiffView text={`${text}\n+again`} />);
  expect(diffRows).toHaveBeenCalledTimes(2);
});
