import { describe, expect, it, vi } from "vitest";
import type { PendingStore } from "@/sw/pendingTarget";
import { consumePendingTarget, createTapGate, type LaunchDeps } from "./launchTarget";

function setup(saved: { url: string; at: number } | null, href = "/d/dk1/s/s1") {
  let value = saved ? JSON.stringify(saved) : null;
  const store: PendingStore = {
    put: () => Promise.resolve(),
    clearIf: () => Promise.resolve(),
    take: () => {
      const taken = value;
      value = null;
      return Promise.resolve(taken);
    },
  };
  const order: string[] = [];
  const navigate = vi.fn((url: string) => void order.push(`navigate ${url}`));
  const deps: LaunchDeps = {
    store,
    ready: () => Promise.resolve(void order.push("ready")),
    currentHref: () => href,
    navigate,
    now: () => 1000,
  };
  return { deps, navigate, order };
}

describe("consumePendingTarget", () => {
  it("waits for the first route, then goes to a Mono chat of another desktop", async () => {
    const { deps, order } = setup({ url: "/d/dk2/monos/m7", at: 900 });
    await expect(consumePendingTarget(deps)).resolves.toBe("/d/dk2/monos/m7");
    expect(order).toEqual(["ready", "navigate /d/dk2/monos/m7"]);
  });

  it("goes to a session on a cold start", async () => {
    const { deps, navigate } = setup({ url: "/d/dk1/s/s9", at: 900 }, "/d/dk1");
    await consumePendingTarget(deps);
    expect(navigate).toHaveBeenCalledWith("/d/dk1/s/s9", 900);
  });

  it("does nothing when there is no target, it is stale or it is where the page already is", async () => {
    for (const [saved, href] of [
      [null, "/"],
      [{ url: "/d/dk1/s/s9", at: 1000 - 3 * 60_000 }, "/"],
      [{ url: "/d/dk1/s/s9", at: 900 }, "/d/dk1/s/s9"],
    ] as const) {
      const { deps, navigate } = setup(saved, href);
      await consumePendingTarget(deps);
      expect(navigate).not.toHaveBeenCalled();
    }
  });

  it("takes the target only once", async () => {
    const { deps, navigate } = setup({ url: "/d/dk2/s/s9", at: 900 });
    await consumePendingTarget(deps);
    await consumePendingTarget(deps);
    expect(navigate).toHaveBeenCalledOnce();
  });

  it("never routes an unsafe saved URL", async () => {
    const { deps, navigate } = setup({ url: "//evil.example", at: 900 });
    await expect(consumePendingTarget(deps)).resolves.toBeNull();
    expect(navigate).not.toHaveBeenCalled();
  });
});

describe("createTapGate", () => {
  it("lets a tap through once, and a new tap to the same URL again", () => {
    const gate = createTapGate();
    expect(gate.first(100)).toBe(true);
    expect(gate.first(100)).toBe(false);
    expect(gate.first(250)).toBe(true);
  });

  it("always lets a tap without an id through and forgets the oldest", () => {
    const gate = createTapGate(2);
    expect(gate.first(undefined)).toBe(true);
    expect(gate.first(undefined)).toBe(true);
    gate.first(1);
    gate.first(2);
    gate.first(3);
    expect(gate.first(1)).toBe(true);
  });
});
