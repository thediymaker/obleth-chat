import { expect, test } from "vitest";
import { contextUse } from "./context";

const chat = (usage: any) => ({ id: "c", title: "", model: "m", messages: [{ role: "user", content: "a".repeat(400), status: "completed" }, { role: "assistant", content: "b".repeat(400), status: "completed", usage }] });

test("the window share comes from the last request of the last reply", () => {
  const use = contextUse(chat({ prompt_tokens: 9000, completion_tokens: 900, last_prompt_tokens: 3000, last_completion_tokens: 200, steps: 3 }), { windowSize: 8000 });
  expect([use.input, use.used, use.output, use.steps]).toEqual([3000, 3200, 900, 3]);
  expect(use.share).toBe(0.4);
  expect(use.messages).toBe(200);
  expect(use.rest).toBe(2800);
});

test("an older reply whose sum passes the window has no known size", () => {
  const use = contextUse(chat({ prompt_tokens: 9000, completion_tokens: 900 }), { windowSize: 8000 });
  expect(use.unknown).toBe(true);
  expect(use.used).toBe(0);
});

test("a new chat uses nothing yet", () => {
  const use = contextUse(chat({ prompt_tokens: 10 }), { windowSize: 8000, open: false });
  expect(use.used).toBe(0);
});
