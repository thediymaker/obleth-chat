import { expect, test } from "vitest";
import { pastesAsFile, promptCount } from "./composer";

const box = (value: string, selectionStart = value.length, selectionEnd = value.length) => ({ value, selectionStart, selectionEnd });

test("long pastes, or ones that would pass the box's limit, are attached as files", () => {
  expect(pastesAsFile("a".repeat(4000), box(""))).toBe(false);
  expect(pastesAsFile("a".repeat(4001), box(""))).toBe(true);
  expect(pastesAsFile("a".repeat(3000), box("b".repeat(9500)))).toBe(true);
  // Replacing a selection frees what it covered.
  expect(pastesAsFile("a".repeat(3000), box("b".repeat(9500), 0, 1000))).toBe(false);
  expect(promptCount(10799)).toBe("");
  expect(promptCount(12000)).toBe(`${(12000).toLocaleString()} / ${(12000).toLocaleString()}`);
});
