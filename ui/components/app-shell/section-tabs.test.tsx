import { render, screen } from "@testing-library/react";
import { vi } from "vitest";
import { SectionTabs } from "./section-tabs";

let path = "/projects/x/code/history";
vi.mock("next/navigation", () => ({ usePathname: () => path }));

const tabs = [
  { href: "/projects/x", label: "Overview" },
  { href: "/projects/x/code", label: "Code", prefix: true },
  { href: "/projects/x/research", label: "Research" },
];

test("a prefix tab stays active under its path, and the bar scrolls instead of the page", () => {
  render(<SectionTabs label="Project" tabs={tabs} />);
  expect(screen.getByRole("link", { name: "Code" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("link", { name: "Overview" })).not.toHaveAttribute("aria-current");
  const nav = screen.getByRole("navigation", { name: "Project" });
  expect(nav.className).toContain("overflow-x-auto");
  expect(screen.getByRole("link", { name: "Research" }).className).toContain("whitespace-nowrap");
});

test("the page may name the active tab itself", () => {
  path = "/settings/admin";
  render(<SectionTabs label="Administration sections" current="/settings/admin/users" tabs={[{ href: "/settings/admin/users", label: "People" }, { href: "/settings/admin/groups", label: "Groups" }]} />);
  expect(screen.getByRole("link", { name: "People" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("link", { name: "Groups" })).not.toHaveAttribute("aria-current");
});

test("a compact bar keeps every tab's name for a screen reader and shows only the active one's on a phone", () => {
  path = "/projects/x/code";
  const Icon = () => <svg aria-hidden="true" />;
  render(<SectionTabs label="Project" compact tabs={tabs.map((t) => ({ ...t, icon: Icon }))} />);
  const overview = screen.getByRole("link", { name: "Overview" });
  expect(overview).toHaveAttribute("title", "Overview");
  expect(overview.className).toContain("max-sm:flex-1");
  expect(overview.querySelector("span")?.className).toContain("max-sm:sr-only");
  const code = screen.getByRole("link", { name: "Code" });
  expect(code).toHaveAttribute("aria-current", "page");
  expect(code.querySelector("span")).toBeNull();
});
