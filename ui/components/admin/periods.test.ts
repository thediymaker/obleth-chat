import { USAGE_PERIODS } from "./usage";
import { WORKSPACE_PERIODS } from "./workspaces";

// The API's usage and workspace reports take these periods only (PERIODS in
// api/src/operations.rs and api/src/workspace_usage.rs); any other button is an error.
const API_PERIODS = [1, 7, 28, 90];

test("the usage and workspace period buttons are the periods the API takes, named for their days", () => {
  for (const periods of [USAGE_PERIODS, WORKSPACE_PERIODS]) {
    expect(periods.map(([days]) => days)).toEqual(API_PERIODS);
    for (const [days, label] of periods) expect(label).toBe(days === 1 ? "Today" : `${days} days`);
  }
});
