import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { SystemTab, embeddingBody, nextPrefixes, type Embedding } from "./system-tab";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const defaults = { model: "qwen3-embedding-8b", queryPrefix: "Instruct: find\nQuery: ", documentPrefix: "" };
const settings: Embedding = {
  chosen: { model: null, queryPrefix: null, documentPrefix: null },
  defaults,
  effective: defaults,
  models: ["embeddinggemma-2", "qwen3-embedding-8b"],
  prefixes: {
    "qwen3-embedding-8b": { queryPrefix: defaults.queryPrefix, documentPrefix: "" },
    "embeddinggemma-2": { queryPrefix: "task: search result | query: ", documentPrefix: "title: none | text: " },
  },
  index: { documents: 12, pendingDocuments: 0, savedItems: 3, pendingSavedItems: 0 },
};
function tab() {
  const calls = mockApi(
    (path, options) => {
      if (path === "/api/admin/retention") return { conversationDays: null, unusedFileDays: null, auditDays: null };
      if (path === "/api/admin/knowledge/embedding") return options.method === "PATCH" ? { ...settings, reindexed: true } : settings;
      if (path === "/api/admin/memory") return { enabled: false, defaultOn: true, model: null, people: 0, memories: 0, gateway: true, embeddings: true };
      return {};
    },
    { knowledge: true },
  );
  renderPage(<SystemTab />);
  return calls;
}
const patch = (calls: { path: string; method: string }[]) => calls.filter((c) => c.method === "PATCH" && c.path.includes("embedding"));

test("changing the embedding model fills in its prefixes and re-indexes only after confirmation", async () => {
  const calls = tab();
  await userEvent.selectOptions(await screen.findByLabelText("Embedding model"), "embeddinggemma-2");
  expect(screen.getByLabelText("Search prefix")).toHaveValue("task: search result | query: ");
  expect(screen.getByLabelText("Passage prefix")).toHaveValue("title: none | text: ");
  await userEvent.click(screen.getAllByRole("button", { name: "Save" })[0]!);
  expect(await screen.findByText(/index all 12 documents and 3 saved items again with embeddinggemma-2/)).toBeInTheDocument();
  expect(patch(calls)).toHaveLength(0);
  await userEvent.click(screen.getByRole("button", { name: "Change and re-index" }));
  // The model's own prefixes are stored as defaults, so they follow the model.
  await waitFor(() => expect(patch(calls)[0]).toMatchObject({ body: { model: "embeddinggemma-2", queryPrefix: null, documentPrefix: null } }));
});

test("a custom prefix is kept as a choice", () => {
  expect(embeddingBody(settings, { model: "embeddinggemma-2", queryPrefix: "task: question answering | query: ", documentPrefix: "title: none | text: " })).toEqual({
    model: "embeddinggemma-2",
    queryPrefix: "task: question answering | query: ",
    documentPrefix: null,
  });
});

test("choosing another model fills in its prefixes unless they were edited", () => {
  const filled = nextPrefixes(settings.prefixes, "qwen3-embedding-8b", "embeddinggemma-2", { queryPrefix: defaults.queryPrefix, documentPrefix: "" });
  expect(filled).toEqual({ queryPrefix: "task: search result | query: ", documentPrefix: "title: none | text: " });
  const own = nextPrefixes(settings.prefixes, "embeddinggemma-2", "qwen3-embedding-8b", { ...filled, queryPrefix: "my own: " });
  expect(own).toEqual({ queryPrefix: "my own: ", documentPrefix: "" });
});

test("values equal to the defaults follow the defaults without a re-index warning", async () => {
  expect(embeddingBody(settings, { model: "qwen3-embedding-8b", queryPrefix: "Instruct: find\r\nQuery: ", documentPrefix: "" })).toEqual({ model: null, queryPrefix: null, documentPrefix: null });
  const calls = tab();
  await screen.findByLabelText("Embedding model");
  await userEvent.click(screen.getAllByRole("button", { name: "Save" })[0]!);
  await waitFor(() => expect(patch(calls)).toHaveLength(1));
  expect(screen.queryByText(/Index everything again/)).toBeNull();
});

test("the system tab warns that a change indexes everything again", async () => {
  tab();
  expect(await screen.findByText(/index every document and saved item again/)).toBeInTheDocument();
  expect(screen.getByText("12 documents and 3 saved items are indexed with qwen3-embedding-8b.")).toBeInTheDocument();
});
