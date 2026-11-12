import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { mockApi, renderPage } from "@/components/manage/test-utils";
import { SecretsPage, type Secret } from "./secrets-page";

vi.mock("@/lib/session", () => import("@/components/manage/test-utils"));
vi.mock("next/navigation", () => ({ usePathname: () => "/settings/secrets", useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/components/app-shell/chrome", () => ({ usePageChrome: () => {} }));

test("secrets are listed by name and note, added with a value, and removed", async () => {
  let secrets: Secret[] = [{ name: "bb-pass", note: "BeagleBone console", updatedAt: "2026-11-11T20:00:00Z" }];
  const calls = mockApi(
    (path, o) => {
      if (path === "/api/secrets") return { secrets };
      if (o.method === "PUT") {
        const name = decodeURIComponent(path.split("/")[3]);
        secrets = [...secrets, { name, note: (o.body as any).note }];
        return { name, note: (o.body as any).note };
      }
      if (o.method === "DELETE") secrets = [];
      return {};
    },
    { connections: true },
  );
  renderPage(<SecretsPage />);
  expect(await screen.findByText("bb-pass")).toBeInTheDocument();
  expect(screen.getByText(/BeagleBone console/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Add secret" }));
  await userEvent.type(screen.getByLabelText("Name"), "wifi-psk");
  await userEvent.type(screen.getByLabelText("Value"), "hunter22");
  await userEvent.type(screen.getByLabelText("What it is for"), "Lab Wi-Fi");
  await userEvent.click(screen.getByRole("button", { name: "Add secret" }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/secrets/wifi-psk", method: "PUT", body: { note: "Lab Wi-Fi", value: "hunter22" } }));
  expect(await screen.findByText("wifi-psk")).toBeInTheDocument();
  await userEvent.click(screen.getAllByRole("button", { name: "Remove" })[0]);
  await userEvent.click(screen.getByRole("button", { name: "Remove" }));
  await waitFor(() => expect(calls).toContainEqual({ path: "/api/secrets/bb-pass", method: "DELETE", body: undefined }));
});
