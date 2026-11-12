import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { PersonalSettings, shownPreferences } from "./personal-settings";
import { usageDays } from "./usage-card";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
const push = vi.fn();
vi.mock("next/navigation", () => ({ usePathname: () => "/settings", useRouter: () => ({ push, replace: vi.fn() }) }));

const models = [
  { id: "auto", name: "Auto" },
  { id: "glm", name: "GLM", description: "Fast", defaults: { temperature: 0.3 }, allowedOverrides: ["systemPrompt"] },
  { id: "gemma", name: "Gemma" },
];
const prefs = { preferences: { defaultModel: "glm", temperature: 0.9, topP: 1, maxTokens: 0, reasoningEffort: "medium", systemPrompt: "", density: "comfortable", focus: false, context: true, favorites: ["gemma"], agentInstructions: null }, overrides: { temperature: 0.9 }, defaultAgentInstructions: "Default agent text" };
function page(features: Record<string, boolean> = {}) {
  const calls = mockApi((path, options) => {
    if (path === "/api/models") return { models };
    if (path === "/api/preferences") return options.method === "PATCH" ? { ...prefs, preferences: { ...prefs.preferences, ...(options.body as object) } } : prefs;
    if (path === "/api/presets") return { presets: [{ id: "p1", name: "Short answers", model: "gone", parameters: { temperature: 0.2, reasoningEffort: "low", maxTokens: 800 } }] };
    if (path === "/api/auth/sessions") return { sessions: [{ id: "s1", current: true, userAgent: "Firefox" }, { id: "s2", current: false, userAgent: "Safari" }] };
    if (path === "/api/auth/tokens") return { tokens: [] };
    if (path === "/api/usage") return { periodDays: 30, today: { requests: 2, totalTokens: 900 }, totals: { requests: 5 }, limits: { dailyRequests: 100 }, daily: [], byModel: [{ model: "glm", totalTokens: 900, requests: 5 }] };
    return {};
  }, features);
  renderPage(<PersonalSettings />);
  return calls;
}

test("a locked control shows the model's value, and others the saved one", () => {
  const shown = shownPreferences(prefs as never, models as never);
  expect(shown.temperature).toBe(0.3);
  expect(shown.systemPrompt).toBe("");
});

test("thirty days are drawn, ending today, with zeros where nothing was used", () => {
  const days = usageDays([{ date: "2026-11-02", promptTokens: 5, completionTokens: 1, requests: 2 }], 30, new Date("2026-11-03T12:00:00Z"));
  expect(days).toHaveLength(30);
  expect(days.at(-1)!.date).toBe("2026-11-03");
  expect(days.at(-2)).toMatchObject({ input: 5, output: 1, requests: 2 });
});

test("personal settings save defaults, appearance and favorites to the account", async () => {
  const calls = page();
  expect(await screen.findByText("Your usage")).toBeInTheDocument();
  expect(screen.getAllByText("GLM").length).toBeGreaterThan(0);
  await waitFor(() => expect(screen.getByLabelText("Default model")).toHaveTextContent("GLM"));
  await userEvent.click(screen.getByRole("button", { name: "Save defaults" }));
  await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ defaultModel: "glm", systemPrompt: "", reasoningEffort: "medium", temperature: 0.3, topP: 1, maxTokens: 0 }));
  await userEvent.click(screen.getByRole("checkbox", { name: "Compact spacing" }));
  await userEvent.click(screen.getByRole("button", { name: "Save appearance" }));
  await waitFor(() => expect(calls.filter((c) => c.method === "PATCH").at(-1)?.body).toEqual({ density: "compact", focus: false, context: true, modelSuggestions: true }));
  expect(screen.getByRole("checkbox", { name: "Gemma" })).toBeChecked();
  await userEvent.click(screen.getByRole("checkbox", { name: /^GLM/ }));
  await userEvent.click(screen.getByRole("button", { name: "Save favorites" }));
  await waitFor(() => expect(calls.filter((c) => c.method === "PATCH").at(-1)?.body).toEqual({ favorites: ["glm", "gemma"] }));
});

test("signed-in devices can be revoked, but not this one", async () => {
  const calls = page();
  const other = (await screen.findByText("Safari")).closest("li")!;
  expect(within(screen.getByText("Firefox").closest("li")!).queryByRole("button", { name: "Revoke" })).toBeNull();
  await userEvent.click(within(other).getByRole("button", { name: "Revoke" }));
  await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/api/auth/sessions/s2")).toBe(true));
  expect(screen.getByRole("button", { name: "Sign out other devices" })).toBeInTheDocument();
});

test("a preset whose model is gone is not used", async () => {
  page();
  await userEvent.click(await screen.findByRole("button", { name: "Use" }));
  expect(push).not.toHaveBeenCalled();
  expect(screen.getByText("low effort · temperature 0.2 · 800 output tokens")).toBeInTheDocument();
});

test("the workspace agent saves its own instructions, and the default text saves as none", async () => {
  const calls = page({ workspaces: true });
  const box = await screen.findByLabelText("Instructions");
  expect(box).toHaveValue("Default agent text");
  await userEvent.click(screen.getByRole("button", { name: "Save instructions" }));
  await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ agentInstructions: null }));
  await userEvent.clear(box);
  await userEvent.type(box, "Work carefully");
  await userEvent.click(screen.getByRole("button", { name: "Save instructions" }));
  await waitFor(() => expect(calls.filter((c) => c.method === "PATCH").at(-1)?.body).toEqual({ agentInstructions: "Work carefully" }));
});
