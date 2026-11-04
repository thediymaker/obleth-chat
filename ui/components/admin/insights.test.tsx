import { render, screen, within } from "@testing-library/react";
import { vi } from "vitest";
import { renderPage } from "@/components/manage/test-utils";
import { InsightsSettingsDialog, InsightsView, type InsightsState } from "./insights-tab";
import { percent, tokens, type Insights } from "./insights";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const report: NonNullable<Insights["workstreams"]> = {
  state: { status: "ready", building: false, model: "big", foundAt: "2026-11-01T00:00:00Z", runs: true },
  waiting: 0,
  list: [
    {
      name: "Lab data analysis",
      description: "Fitting assay data",
      private: false,
      sessions: 3,
      tokens: 9000,
      kinds: { research: 3 },
      outputs: { analysis: 3 },
      rows: [{ topic: "Fitting a <b>curve</b>", kind: "research", output: "analysis", field: "life_health", turns: 2, tokens: 3000 }],
    },
    { name: "Personal and unlisted chats", description: "Counted, never listed.", private: true, sessions: 1, tokens: 100000, kinds: { personal: 1 }, outputs: { answer: 1 }, rows: [] },
  ],
};
const data: Insights = {
  settings: { enabled: true, model: "judge" },
  gateway: true,
  groups: [{ id: "g1", name: "Lab" }],
  sessions: 4,
  people: 6,
  tokens: 109000,
  kinds: [
    { name: "research", sessions: 3, tokens: 9000 },
    { name: "personal", sessions: 1, tokens: 100000 },
  ],
  fields: [{ name: "life_health", sessions: 3 }],
  activities: [],
  outputs: [],
  flags: { secrets: 1 },
  workstreams: report,
  previous: { sessions: 2, people: 6, tokens: 50000 },
  largestChat: 100000,
  trend: {
    unit: "week",
    points: [
      { at: "2026-10-19", kinds: { research: 2 } },
      { at: "2026-10-26", kinds: { research: 1, personal: 1 } },
    ],
  },
};
const base: InsightsState = { days: 28, group: "", sort: "sessions", open: new Set(), all: new Set() };
const on = { setSort: vi.fn(), toggle: vi.fn(), showAll: vi.fn(), openSettings: vi.fn(), findAgain: vi.fn() };
const view = (d: Insights = data, s: Partial<InsightsState> = {}) => render(<InsightsView data={d} state={{ ...base, ...s }} on={on} />);
const text = (d: Insights, s: Partial<InsightsState> = {}) => {
  const { container, unmount } = view(d, s);
  const t = container.textContent;
  unmount();
  return t;
};

test("insights show labels, workstreams and flags, with topics as text", () => {
  const { container } = view(data, { open: new Set(["Lab data analysis"]) });
  expect(container.textContent).toContain("75% research and learning");
  expect(screen.getByText("Fitting a <b>curve</b>")).toBeInTheDocument();
  expect(container.querySelector("b")).toBeNull();
  const flag = screen.getByText("Pasted keys or passwords").parentElement!;
  expect(within(flag).getByText("1")).toHaveAttribute("data-flagged");
  // Sorted by chats by default; by tokens, the unlisted chats come first.
  const names = () => within(screen.getByTestId("workstreams")).getAllByText(/^(Lab data analysis|Personal and unlisted chats)$/).map((n) => n.textContent);
  expect(names()).toEqual(["Lab data analysis", "Personal and unlisted chats"]);
  // One chat holds most tokens: said once in the tiles and once over the workstreams.
  expect(container.textContent).toContain("One chat holds 92%");
  expect(container.textContent).toContain("One chat used 92% of this period’s tokens (100K)");
  expect(container.textContent).toContain("↑ 100% on the 28 days before");
  expect(screen.getByRole("img", { name: "Labelled chats by kind of use, by week" })).toBeInTheDocument();
});

test("by tokens the unlisted chats come first, and they are never listed", () => {
  view(data, { sort: "tokens", open: new Set(["Personal and unlisted chats"]) });
  const names = within(screen.getByTestId("workstreams")).getAllByText(/^(Lab data analysis|Personal and unlisted chats)$/).map((n) => n.textContent);
  expect(names).toEqual(["Personal and unlisted chats", "Lab data analysis"]);
  expect(screen.getByText(/counted but never listed/)).toBeInTheDocument();
});

test("insights explain when they are off, too small or still building", () => {
  expect(text({ ...data, settings: { enabled: false } })).toContain("Usage insights are off");
  expect(text({ ...data, settings: { enabled: false }, gateway: false })).toContain("no gateway connected");
  expect(text({ ...data, tooFew: true, people: 2, minPeople: 5 })).toContain("2 people. A group shows from 5 people");
  const empty = { state: { status: "none", building: false, runs: true }, waiting: 0, list: [] };
  expect(text({ ...data, workstreams: { ...empty, state: { ...empty.state, status: "building", building: true, model: "big" } } })).toContain("Finding workstreams with big…");
  expect(text({ ...data, workstreams: empty })).toContain("appears on its own once about 20 chats");
  expect(text({ ...data, workstreams: { ...empty, state: { status: "failed", building: false, error: "No list", runs: true } } })).toContain("No list");
  expect(text({ ...data, workstreams: { ...report, waiting: 3 } })).toContain("3 chats are still being placed");
  expect(text({ ...data, workstreams: { ...empty, state: { ...empty.state, runs: false } } })).toContain("Save the Insights settings");
  expect(text({ ...data, sessions: 0 })).toContain("No labelled chats yet");
});

test("insight numbers read plainly, and the settings show what is saved", () => {
  expect(percent(0.004)).toBe("<1%");
  expect(percent(0)).toBe("0%");
  expect(tokens(1_250_000)).toBe("1.3M");
  expect(tokens(48_000)).toBe("48K");
  expect(tokens(950)).toBe("950");
  renderPage(<InsightsSettingsDialog settings={{ enabled: true, model: "judge" }} models={["judge", "big"]} onClose={() => {}} onSaved={() => {}} />);
  expect(screen.getByRole("checkbox", { name: "Label chats" })).toBeChecked();
  expect(screen.getByRole("checkbox", { name: "Name new chats" })).not.toBeChecked();
  expect(screen.getByLabelText("Labelling model")).toHaveValue("judge");
});
