import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { ModelsTab, modelBody, type AdminModel } from "./models-tab";
import { forgetKept } from "./state";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const models: AdminModel[] = [
  { id: "glm", name: "GLM", enabled: true, published: true, shortList: true },
  { id: "gemma", name: "Gemma", enabled: true, published: true, shortList: false },
  { id: "old", name: "Old", enabled: true, published: false, shortList: false },
];
const tab = (list: AdminModel[]) => {
  forgetKept();
  const calls = mockApi((path, options) => {
    if (options.method === "PUT") return options.body;
    if (path === "/api/admin/models") return { models: list };
    if (path === "/api/models") return { models: list };
    return { groups: [] };
  });
  renderPage(<ModelsTab />);
  return calls;
};

test("the models tab says what the short list shows and saves the models picked", async () => {
  const calls = tab(models);
  expect(await screen.findByText(/The model chooser shows these 1 first, with the other 1 under See all models/)).toBeInTheDocument();
  const status = screen.getAllByTestId("model-status")[0]!;
  expect(status).toHaveTextContent("Short list");
  await userEvent.click(screen.getByRole("button", { name: "Change" }));
  expect(screen.getByRole("checkbox", { name: "GLM" })).toBeChecked();
  expect(screen.queryByRole("checkbox", { name: "Old" })).toBeNull();
  await userEvent.click(screen.getByRole("checkbox", { name: "GLM" }));
  await userEvent.click(screen.getByRole("checkbox", { name: "Gemma" }));
  await userEvent.click(screen.getByRole("button", { name: "Save short list" }));
  await waitFor(() => expect(calls.find((c) => c.method === "PUT")).toEqual({ path: "/api/admin/model-short-list", method: "PUT", body: { models: ["gemma"] } }));
});

test("with no short list, the models tab says every model is listed", async () => {
  tab(models.map((m) => ({ ...m, shortList: false })));
  expect(await screen.findByText(/The model chooser lists all 2 models/)).toBeInTheDocument();
});

test("the Configure form saves the model's policy", () => {
  const form = new FormData();
  for (const [k, v] of Object.entries({
    "model-name": " GLM 5 ",
    "model-description": "Fast",
    "model-tags": "fast, code, fast",
    "model-order": "3",
    "model-enabled": "on",
    "model-visibility": "groups",
    "model-group-g2": "on",
    "model-systemPrompt": "Be brief",
    "model-reasoningEffort": "low",
    "model-temperature": "0.2",
    "model-topP": "1",
    "model-maxTokens": "",
    "allow-temperature": "on",
  }))
    form.set(k, v);
  expect(modelBody(form, [{ id: "g1", name: "A" }, { id: "g2", name: "B" }])).toEqual({
    name: "GLM 5",
    description: "Fast",
    tags: ["fast", "code"],
    order: 3,
    enabled: true,
    published: false,
    isDefault: false,
    worker: false,
    actions: "auto",
    visibility: "groups",
    groupIds: ["g2"],
    defaults: { systemPrompt: "Be brief", reasoningEffort: "low", temperature: 0.2, topP: 1, maxTokens: 0 },
    allowedOverrides: ["temperature"],
  });
});
