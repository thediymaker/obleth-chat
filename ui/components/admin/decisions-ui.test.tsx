import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { choose, mockApi, renderPage } from "@/components/manage/test-utils";
import { DecisionsCard, checkResult, decisionsBody, type Decisions } from "./system-tab";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const saved: Decisions = { endpoint: "gateway", url: null, hasKey: false, keyUnreadable: false, model: null, checkRuns: true, checkTree: true, checkReports: false, pickSections: true, checkMemories: true, checkActions: true, canStoreKey: true, gateway: true, models: ["judge-31b"] };
function card(d: Decisions = saved, answer: object = { ok: true, model: "judge-31b", yes: 0.98, confidence: 0.9, ms: 412 }) {
  const calls = mockApi((path, options) => (path === "/api/admin/decisions/check" ? answer : { ...d, ...(options.body as object) }));
  renderPage(<DecisionsCard decisions={d} onSaved={() => {}} />);
  return calls;
}
const form = (values: Record<string, string>) => {
  const f = new FormData();
  for (const [k, v] of Object.entries(values)) f.set(k, v);
  return f;
};

test("the card says where decisions go and whose usage they count toward", () => {
  const { container } = (card(), { container: document.body });
  expect(screen.getByLabelText("Ask")).toHaveTextContent("This gateway");
  expect(container.textContent).toContain("count toward that person’s usage");
  expect(screen.getByTestId("decisions-custom")).not.toBeVisible();
  expect(container.querySelector('datalist#decisions-models option[value="judge-31b"]')).not.toBeNull();
  expect(screen.getByRole("checkbox", { name: "Check where new experiments go" })).toBeChecked();
  expect(screen.getByRole("checkbox", { name: "Check research reports" })).not.toBeChecked();
  expect(screen.getByRole("checkbox", { name: "Pick sections of papers" })).toBeChecked();
});

test("saving the gateway sends no URL or key", () => {
  expect(decisionsBody(form({ "decisions-endpoint": "gateway", "decisions-url": "https://left.over", "decisions-key": "sk-old", "decisions-check-runs": "on", "decisions-check-reports": "on" }))).toEqual({
    endpoint: "gateway",
    model: null,
    checkRuns: true,
    checkTree: false,
    checkReports: true,
    pickSections: false,
    checkMemories: false,
    checkActions: false,
  });
});

test("another endpoint sends its URL, and a blank key keeps the saved one", () => {
  const values = { "decisions-endpoint": "custom", "decisions-url": " https://d.example.com ", "decisions-model": "judge-31b" };
  expect(decisionsBody(form(values))).toEqual({ endpoint: "custom", model: "judge-31b", checkRuns: false, checkTree: false, checkReports: false, pickSections: false, checkMemories: false, checkActions: false, url: "https://d.example.com" });
  expect(decisionsBody(form({ ...values, "decisions-key": "sk-new" })).key).toBe("sk-new");
  expect(decisionsBody(form({ ...values, "decisions-remove-key": "on" })).key).toBeNull();
});

test("Check asks with the form as it is and says what came back", async () => {
  const calls = card();
  await choose(screen.getByLabelText("Ask"), "Another endpoint");
  await userEvent.type(screen.getByLabelText("Endpoint URL"), "https://d.example.com");
  await userEvent.click(screen.getByRole("button", { name: "Check" }));
  expect(await screen.findByText("Working. judge-31b answered a sample true statement correctly (98% yes) in 412 ms.")).toBeInTheDocument();
  expect(calls[0]).toEqual({ path: "/api/admin/decisions/check", method: "POST", body: { endpoint: "custom", model: null, checkRuns: true, checkTree: true, checkReports: false, pickSections: true, checkMemories: true, checkActions: true, url: "https://d.example.com" } });
  expect(checkResult({ ok: true, model: "tiny", yes: 0.2, ms: 90 })).toBe("tiny answered a sample true statement wrongly (20% yes). Try another model.");
  expect(checkResult({ ok: false, error: "The endpoint answered 404", ms: 30 })).toBe("No answer: The endpoint answered 404");
});

test("choosing another endpoint shows its fields and who its usage counts against", async () => {
  card();
  await choose(screen.getByLabelText("Ask"), "Another endpoint");
  expect(screen.getByTestId("decisions-custom")).toBeVisible();
  expect(screen.getByText(/counts against the key, not against people/)).toBeInTheDocument();
});

test("without a secret key no key can be entered", async () => {
  card({ ...saved, endpoint: "custom", url: "https://d.example.com/v1/verdicts", canStoreKey: false });
  expect(screen.getByText(/Saving a key needs OBLETH_CHAT_SECRET_KEY_FILE/)).toBeInTheDocument();
  expect(screen.getByLabelText("Key")).toBeDisabled();
  await waitFor(() => expect(screen.getByTestId("decisions-custom")).toBeVisible());
});
