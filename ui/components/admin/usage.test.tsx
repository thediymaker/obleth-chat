import { render, screen, within } from "@testing-library/react";
import { vi } from "vitest";
import { UsageView, type UsageState } from "./usage-tab";
import { buckets } from "./usage";
import { change, changeText } from "./insights";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const counts = (requests: number, totalTokens: number, people = 1) => ({
  requests,
  promptTokens: totalTokens * 0.8,
  completionTokens: totalTokens * 0.2,
  totalTokens,
  errors: 0,
  activeGenerations: 0,
  incompleteUsage: 0,
  reservedTokens: 0,
  people,
});
const data = {
  days: 7,
  start: "2026-10-27T00:00:00Z",
  groups: [{ id: "g1", name: "Lab" }],
  models: ["glm", "gemma"],
  totals: { ...counts(120, 600000, 6), errors: 6, activeGenerations: 2 },
  previous: counts(100, 500000, 5),
  series: [
    { at: "2026-10-29", ...counts(40, 200000, 4) },
    { at: "2026-11-02", ...counts(80, 400000, 5) },
  ],
  byModel: [
    { name: "glm", ...counts(90, 500000, 5) },
    { name: null, ...counts(30, 100000, 2) },
  ],
  byPurpose: [
    { name: "chat", ...counts(100, 550000, 6) },
    { name: "follow_up", ...counts(20, 50000, 6) },
  ],
  people: [
    {
      id: "p1",
      username: "ana",
      displayName: "Ana <i>",
      enabled: true,
      lastActive: "2026-11-02T10:00:00Z",
      ...counts(60, 400000),
      today: { requests: 9, totalTokens: 950000, reservedTokens: 0, fraction: 0.95 },
      limits: { dailyRequests: null, dailyTokens: 1000000 },
      override: true,
    },
    {
      id: "p2",
      username: "bo",
      displayName: "Bo",
      enabled: true,
      lastActive: "2026-11-02T12:00:00Z",
      ...counts(60, 200000),
      today: { requests: 1, totalTokens: 10, reservedTokens: 0, fraction: 0 },
      limits: { dailyRequests: null, dailyTokens: null },
    },
  ],
  limits: { dailyRequests: null, dailyTokens: 1000000, concurrentRequests: 3 },
  overrides: 1,
  atLimit: 0,
  nearLimit: 1,
  limited: [{ id: "p1", username: "ana", displayName: "Ana <i>", atLimit: false }],
  mix: {
    enabled: true,
    sessions: 30,
    people: 12,
    tokens: 900,
    minPeople: 5,
    kinds: [
      { name: "research", sessions: 20, tokens: 600 },
      { name: "play", sessions: 10, tokens: 300 },
    ],
    byModel: [{ model: "glm", sessions: 30, tokens: 900, kinds: [{ name: "research", sessions: 20, tokens: 600 }] }],
    heavy: { top: { people: 5, tokens: 600, kinds: [{ name: "research", sessions: 10, tokens: 600 }] }, rest: { people: 7, tokens: 300, kinds: [{ name: "play", sessions: 10, tokens: 300 }] } },
  },
};
const state: UsageState = { days: 7, group: "g1", model: "", query: "", sort: "totalTokens", metric: "tokens" };
const on = { setDays: vi.fn(), setGroup: vi.fn(), setModel: vi.fn(), setQuery: vi.fn(), setSort: vi.fn(), setMetric: vi.fn(), editLimits: vi.fn() };
const view = (d: object = data, s: Partial<UsageState> = {}) => render(<UsageView data={d as typeof data} state={{ ...state, ...s }} on={on} />);

test("usage shows totals against the period before, models, people and limits, with names as text", () => {
  const { container } = view();
  expect(container.textContent).toContain("↑ 20% on the 7 days before");
  expect(container.textContent).toContain("5% of requests");
  expect(screen.getByRole("button", { name: "glm" })).toHaveAttribute("title", "Show only this model");
  expect(screen.getByText("Earlier chats")).toBeInTheDocument();
  expect(screen.getByText("Suggested follow-ups")).toBeInTheDocument();
  // A name is text, never markup.
  expect(screen.getAllByText("Ana <i>").length).toBeGreaterThan(0);
  expect(screen.getByText("Near limit")).toBeInTheDocument();
  expect(container.querySelector('[data-level="near"]')).not.toBeNull();
  expect(screen.getByText("No daily limit")).toBeInTheDocument();
  expect(screen.getByText("The 5 who used the most")).toBeInTheDocument();
  expect(screen.getByText("Everyone else (7)")).toBeInTheDocument();
  expect(screen.getByRole("img", { name: /^Tokens per day: 600K in all/ })).toBeInTheDocument();
});

test("people sort by tokens by default, and by last use on request", () => {
  const order = () => within(screen.getByTestId("usage-people")).getAllByText(/^(ana|bo)$/).map((n) => n.textContent);
  const first = view();
  expect(order()).toEqual(["ana", "bo"]);
  first.unmount();
  view(data, { sort: "lastActive" });
  expect(order()).toEqual(["bo", "ana"]);
});

test("usage explains when Insights are off or the group is too small", () => {
  const text = (d: object) => {
    const { container, unmount } = view(d);
    const t = container.textContent;
    unmount();
    return t;
  };
  expect(text({ ...data, mix: { enabled: false } })).toContain("Turn on Insights");
  expect(text({ ...data, mix: { enabled: true, tooFew: true, people: 2, minPeople: 5 } })).toContain("2 people in the last 7 days. Kinds of use show from 5 people");
  expect(text({ ...data, mix: { ...data.mix, heavy: undefined } })).toContain("shows once 10 people");
  expect(text({ ...data, people: [] })).toContain("Nobody used Chat");
  expect(text({ ...data, limited: [] })).toContain("Nobody is near a daily limit today");
});

test("every day or hour of the period is drawn, with zeros where nothing was used", () => {
  const days = buckets(data.series, 7, data.start);
  expect(days).toHaveLength(7);
  expect(days[2]!.requests).toBe(40);
  expect(days[0]!.requests).toBe(0);
  expect(days[6]!.key).toBe("2026-11-02");
  const hours = buckets([{ at: "2026-11-02T13", requests: 3 }], 1, "2026-11-02T00:00:00Z");
  expect(hours).toHaveLength(24);
  expect(hours[13]!.requests).toBe(3);
  expect(hours[13]!.label).toBe("13:00");
  view(data, { metric: "people" });
  expect(screen.getByRole("img", { name: "People per day: 9 in all" })).toBeInTheDocument();
  expect(change(5, 0, 7)).toBeNull();
  expect(changeText(change(90, 100, 1))).toBe("↓ 10% on yesterday");
});
