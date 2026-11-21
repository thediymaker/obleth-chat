import { render, screen, within } from "@testing-library/react";
import { DataTable } from "./kit";

test("a data table draws its rows under its headings, and one row saying it is empty", () => {
  const { rerender } = render(<DataTable label="Jobs" columns={["Job", { head: "State", className: "hidden sm:table-cell" }]} rows={[{ key: 1, cells: ["42", "Running"] }]} />);
  const table = screen.getByRole("table", { name: "Jobs" });
  expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Job", "State"]);
  expect(within(table).getByText("Running")).toHaveClass("hidden", "sm:table-cell");
  rerender(<DataTable label="Jobs" columns={["Job", "State"]} rows={[]} empty="No jobs yet." />);
  const empty = screen.getByText("No jobs yet.");
  expect(empty.tagName).toBe("TD");
  expect(empty).toHaveAttribute("colspan", "2");
});
