// The Allow card says where a change runs: a device is changed by the person's
// browser, not a cluster or an account.

import { describe, expect, it, vi } from "vitest";
vi.mock("@/lib/session", () => ({ useSession: () => ({ phase: "ready", isAdmin: false, user: { username: "pat" } }), useUser: () => ({ id: "u1", role: "user" }), onBeforeSignOut: () => () => {} }));
import { changeNote } from "./turn-view";

describe("changeNote", () => {
  it("names the device for a device, and the cluster only for clusters", () => {
    expect(changeNote("device")).toBe("It changes the device, so it waits for you. Your browser sends it.");
    expect(changeNote("slurm")).toContain("the cluster");
    expect(changeNote("kubernetes")).toContain("the cluster");
    expect(changeNote("mcp")).toContain("something outside Chat");
    expect(changeNote(undefined)).toContain("something outside Chat");
  });
});
