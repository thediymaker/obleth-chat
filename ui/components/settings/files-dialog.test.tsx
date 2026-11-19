import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { FILES_PAGE, FilesDialog } from "./files-dialog";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

const file = (n: number) => ({ id: `f${n}`, name: `notes-${n}.txt`, kind: "text", size: 10 });

test("the Files dialog shows a page and loads the next on request", async () => {
  const calls = mockApi((path) => {
    const limit = Number(new URL(path, "http://x").searchParams.get("limit"));
    const items = Array.from({ length: Math.min(limit, 150) }, (_, i) => file(i + 1));
    return { items, more: items.length < 150 };
  });
  renderPage(<FilesDialog open onOpenChange={() => {}} />);
  expect(await screen.findByText("notes-1.txt")).toBeInTheDocument();
  expect(screen.queryByText("notes-101.txt")).not.toBeInTheDocument();
  expect(calls.at(-1)!.path).toBe(`/api/attachments?limit=${FILES_PAGE}`);
  await userEvent.click(screen.getByRole("button", { name: "Show more files" }));
  expect(await screen.findByText("notes-150.txt")).toBeInTheDocument();
  expect(calls.at(-1)!.path).toBe(`/api/attachments?limit=${2 * FILES_PAGE}`);
  expect(screen.queryByRole("button", { name: "Show more files" })).not.toBeInTheDocument();
});
