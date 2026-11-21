import { render, screen } from "@testing-library/react";
import { ApiError } from "@/lib/api";
import { ErrorBox, FeatureOff, isFeatureOff } from "./page";

test("a feature that is off says so in one wording", () => {
  const { rerender } = render(<FeatureOff what="Memory" />);
  expect(screen.getByRole("heading", { name: "Memory is turned off on this server" })).toBeInTheDocument();
  expect(screen.getByText("Ask an administrator to turn it on.")).toBeInTheDocument();
  rerender(<FeatureOff what="Workspaces" plural />);
  expect(screen.getByRole("heading", { name: "Workspaces are turned off on this server" })).toBeInTheDocument();
  expect(screen.getByText("Ask an administrator to turn them on.")).toBeInTheDocument();
});

test("an API error with code feature_off is told apart, and offers no Retry", () => {
  const off = new ApiError("Connections are not configured", 503, "feature_off");
  expect(isFeatureOff(off)).toBe(true);
  expect(isFeatureOff(new ApiError("Gateway down", 503))).toBe(false);
  expect(isFeatureOff(null)).toBe(false);
  const { rerender } = render(<ErrorBox error={off} onRetry={() => {}} />);
  expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument();
  rerender(<ErrorBox error={new ApiError("Gateway down", 503)} onRetry={() => {}} />);
  expect(screen.getByRole("button", { name: /Retry/ })).toBeInTheDocument();
});
