import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { choose, mockApi, renderPage } from "@/components/manage/test-utils";
import { ActivityTab } from "./activity-tab";
import { auditDay, auditDetails, auditSentence, sentenceText, type AuditEvent } from "./audit";
import { forgetKept } from "./state";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));

const e = (action: string, more: Partial<AuditEvent> = {}): AuditEvent => ({ action, kind: "accounts", createdAt: "2026-11-02T10:00:00Z", actorName: "Ana", targetName: "Bo", ...more });

test("each event reads as one sentence, with names in bold", () => {
  expect(auditSentence(e("user_created"))).toEqual([{ bold: "Ana" }, " created the account ", { bold: "Bo" }]);
  expect(sentenceText(auditSentence(e("group_created", { targetName: null })))).toBe("Ana created the group");
  expect(sentenceText(auditSentence(e("group_member_added", { memberName: "Cy", targetName: "Lab" })))).toBe("Ana added Cy to Lab");
  expect(sentenceText(auditSentence(e("user_updated", { metadata: { enabled: false } })))).toBe("Ana turned off the account Bo");
  expect(sentenceText(auditSentence(e("models.short_list", { metadata: { models: ["a", "b"] } })))).toBe("Ana set the short list of models to 2");
  expect(sentenceText(auditSentence(e("app.published", { metadata: { name: "Grader", version: 3 } })))).toBe("Ana published the app “Grader” as v3");
  expect(sentenceText(auditSentence(e("app.deleted", { metadata: {} })))).toBe("Ana deleted the app");
  expect(sentenceText(auditSentence(e("workspaces.stop", { metadata: { kind: "browser" } })))).toBe("Ana stopped Bo’s browser");
  expect(sentenceText(auditSentence(e("retention.cleanup", { actorName: null, metadata: { conversations: 2, auditEvents: 1, at: "x" } })))).toBe("Automatic cleanup removed 2 inactive conversations, 1 old activity entry");
  expect(sentenceText(auditSentence(e("retention.cleanup", { metadata: {} })))).toBe("Automatic cleanup ran");
  expect(sentenceText(auditSentence(e("session_revoked", { actorName: null, metadata: { current: true } })))).toBe("Chat signed out");
  expect(sentenceText(auditSentence(e("something_new")))).toBe("Ana · Something New");
});

test("details list what was stored, and cleanup leaves out its time", () => {
  expect(auditDetails(e("retention.cleanup", { metadata: { at: "x", unusedFiles: 2, dryRun: false, list: [] }, targetType: "user", targetId: "u9" }))).toEqual([
    ["Unused Files", "2"],
    ["Dry Run", "No"],
    ["List", "None"],
    ["User ID", "u9"],
  ]);
  const now = new Date("2026-11-03T15:00:00");
  expect(auditDay("2026-11-03T09:00:00", now)).toBe("Today");
  expect(auditDay("2026-11-02T09:00:00", now)).toBe("Yesterday");
});

test("the Activity tab filters by kind and loads older pages", async () => {
  forgetKept();
  const calls = mockApi((path) => (path.includes("before=") ? { events: [e("user_invited", { createdAt: "2026-10-01T10:00:00Z" })], nextCursor: null } : { events: [e("user_created")], nextCursor: "c1" }));
  renderPage(<ActivityTab />);
  expect(await screen.findByText(/created the account/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Load older activity" }));
  expect(await screen.findByText(/invited/)).toBeInTheDocument();
  expect(calls.some((c) => c.path === "/api/admin/audit?limit=50&before=c1")).toBe(true);
  await choose(screen.getByLabelText("Kind of activity"), "Groups");
  await waitFor(() => expect(calls.at(-1)!.path).toBe("/api/admin/audit?limit=50&kind=groups"));
});

test("with filters and nothing found, the filters can be cleared", async () => {
  forgetKept();
  mockApi((path) => (path.includes("q=") ? { events: [] } : { events: [e("user_created")] }));
  renderPage(<ActivityTab />);
  await userEvent.type(await screen.findByLabelText("Search activity"), "zzz{Enter}");
  await userEvent.click(await screen.findByRole("button", { name: "Clear filters" }));
  expect(await screen.findByText(/created the account/)).toBeInTheDocument();
});
