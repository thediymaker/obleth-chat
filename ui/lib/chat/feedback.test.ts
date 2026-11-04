import { expect, test } from "vitest";
import { feedbackDetails, nextRating, ratable, REASONS } from "./feedback";

const reply = (extra = {}) => ({ id: "m1", role: "assistant", status: "completed", content: "Hi", ...extra });

test("only finished replies in your own chat can be rated", () => {
  expect(ratable(reply(), false)).toBe(true);
  expect(ratable(reply({ status: "interrupted" }), false)).toBe(true);
  expect(ratable(reply(), { ownerName: "Ada" })).toBe(false);
  expect(ratable(reply({ status: "pending" }), false)).toBe(false);
  expect(ratable(reply({ status: "error" }), false)).toBe(false);
  expect(ratable(reply({ id: undefined }), false)).toBe(false);
  expect(ratable(reply({ role: "user" }), false)).toBe(false);
});

test("clicking the chosen thumb again clears it", () => {
  expect(nextRating(-1, -1)).toBe(0);
  expect(nextRating(-1, 1)).toBe(1);
  expect(nextRating(0, -1)).toBe(-1);
});

test("every reason the server stores is offered", () => {
  expect(REASONS.map(([, label]) => label)).toEqual(["Wrong or made up", "Didn’t follow instructions", "Too long", "Too short", "Harmful or offensive", "Other"]);
});

test("the form sends a thumbs down with its reasons and a trimmed comment", () => {
  expect(feedbackDetails(["wrong", "other"], "  Made it up  ")).toEqual({ rating: -1, reasons: ["wrong", "other"], comment: "Made it up" });
  expect(feedbackDetails(["wrong", "other"], "   ")).toEqual({ rating: -1, reasons: ["wrong", "other"] });
  expect(feedbackDetails([], "x".repeat(2500)).comment?.length).toBe(2000);
});
