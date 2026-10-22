// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createDriverLease,
  DRIVER_CHECK_MS,
  DRIVER_LEASE_KEY,
  DRIVER_STALE_MS,
  useWorkflowDriver,
} from "./workflowDriver";

function memoryStorage(onSet?: (key: string) => void) {
  const values = new Map<string, string>();
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
      onSet?.(key);
    },
    removeItem: (key: string) => values.delete(key),
  };
}

describe("createDriverLease", () => {
  it("the first window acquires the lease and the second does not", () => {
    const storage = memoryStorage();
    const a = createDriverLease({ storage, now: () => 1_000, id: "a" });
    const b = createDriverLease({ storage, now: () => 1_000, id: "b" });
    expect(a.claim()).toBe(true);
    expect(b.claim()).toBe(false);
    // The holder keeps it by renewing, however often others look.
    expect(a.claim()).toBe(true);
    expect(b.claim()).toBe(false);
  });

  it("a lease that is not renewed goes stale and is taken over", () => {
    const storage = memoryStorage();
    let now = 1_000;
    const a = createDriverLease({ storage, now: () => now, id: "a" });
    const b = createDriverLease({ storage, now: () => now, id: "b" });
    expect(a.claim()).toBe(true);
    now += DRIVER_STALE_MS;
    expect(b.claim()).toBe(false);
    now += 1;
    expect(b.claim()).toBe(true);
    // The old holder finds out when it renews.
    expect(a.claim()).toBe(false);
  });

  it("releasing hands the lease over at once, and only the holder can release", () => {
    const storage = memoryStorage();
    const a = createDriverLease({ storage, now: () => 1_000, id: "a" });
    const b = createDriverLease({ storage, now: () => 1_000, id: "b" });
    a.claim();
    b.release();
    expect(b.claim()).toBe(false);
    a.release();
    expect(storage.values.has(DRIVER_LEASE_KEY)).toBe(false);
    expect(b.claim()).toBe(true);
  });

  it("two windows never both hold it, even when they write at the same moment", () => {
    // B writes between A's write and A's read-back.
    let a!: ReturnType<typeof createDriverLease>;
    let b!: ReturnType<typeof createDriverLease>;
    let racing = true;
    const storage = memoryStorage((key) => {
      if (racing && key === DRIVER_LEASE_KEY) {
        racing = false;
        storage.values.set(
          DRIVER_LEASE_KEY,
          JSON.stringify({ id: "b", at: 1_000 }),
        );
      }
    });
    a = createDriverLease({ storage, now: () => 1_000, id: "a" });
    b = createDriverLease({ storage, now: () => 1_000, id: "b" });
    expect(a.claim()).toBe(false);
    expect(b.claim()).toBe(true);
    expect(a.claim()).toBe(false);
  });

  it("does not hold when storage fails", () => {
    const storage = {
      getItem: () => null,
      setItem: () => {
        throw new Error("full");
      },
      removeItem: () => undefined,
    };
    expect(createDriverLease({ storage }).claim()).toBe(false);
  });
});

describe("useWorkflowDriver", () => {
  beforeEach(() => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    localStorage.clear();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("elects one window, hands over when it closes, and renews while it lives", async () => {
    const seen: Record<string, boolean> = {};
    const roots = ["first", "second"].map((name) => {
      function Probe() {
        seen[name] = useWorkflowDriver();
        return null;
      }
      return { name, Probe, root: createRoot(document.createElement("div")) };
    });
    const [first, second] = roots;
    act(() => first.root.render(createElement(first.Probe)));
    act(() => second.root.render(createElement(second.Probe)));
    expect(seen).toEqual({ first: true, second: false });

    // Long past the stale mark, the holder is still the holder because it renews.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(DRIVER_STALE_MS * 3);
    });
    expect(seen).toEqual({ first: true, second: false });

    // Closing the holder frees the lease; the other takes it when it next looks.
    act(() => first.root.unmount());
    expect(localStorage.getItem(DRIVER_LEASE_KEY)).toBeNull();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(DRIVER_CHECK_MS);
    });
    expect(seen.second).toBe(true);
    act(() => second.root.unmount());
  });

  it("frees the lease when the page is going away", () => {
    function Probe() {
      useWorkflowDriver();
      return null;
    }
    const root = createRoot(document.createElement("div"));
    act(() => root.render(createElement(Probe)));
    expect(localStorage.getItem(DRIVER_LEASE_KEY)).not.toBeNull();
    window.dispatchEvent(new Event("pagehide"));
    expect(localStorage.getItem(DRIVER_LEASE_KEY)).toBeNull();
    act(() => root.unmount());
  });
});
