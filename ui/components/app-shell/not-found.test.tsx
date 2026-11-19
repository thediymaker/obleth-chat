import { render, screen } from "@testing-library/react";
import { ApiError } from "@/lib/api";
import { isNotFound, NotFound } from "./page";

test("a missing item is one block: title, one sentence, a way back, no Retry", () => {
  render(<NotFound what="project" to="/projects" back="all projects" />);
  expect(screen.getByRole("heading", { name: "Project not found" })).toBeInTheDocument();
  expect(screen.getByText("This project does not exist, or it is not shared with you.")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /Back to all projects/ })).toHaveAttribute("href", "/projects");
  expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument();
  expect(isNotFound(new ApiError("gone", 404))).toBe(true);
  expect(isNotFound(new ApiError("down", 503))).toBe(false);
  expect(isNotFound(null)).toBe(false);
});
