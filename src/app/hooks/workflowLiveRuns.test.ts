import { describe, expect, it } from "vitest";
import {
  createLiveRunRegistry,
  LIVE_RUN_STALE_MS,
  LIVE_RUNS_KEY,
} from "./workflowLiveRuns";

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
}

describe("live run registry", () => {
  it("a run is live for every window until its owner stops refreshing it", () => {
    const storage = memoryStorage();
    let now = 1_000;
    const a = createLiveRunRegistry({ storage, owner: "a", now: () => now });
    const b = createLiveRunRegistry({ storage, owner: "b", now: () => now });
    a.touch("run-1", "wf-1");
    expect(b.isFresh("run-1")).toBe(true);
    expect(b.freshRunOf("wf-1")).toBe("run-1");
    expect(b.freshRunOf("wf-2")).toBeUndefined();

    now += LIVE_RUN_STALE_MS;
    expect(b.isFresh("run-1")).toBe(true);
    now += 1;
    expect(b.isFresh("run-1")).toBe(false);
    expect(b.freshRunOf("wf-1")).toBeUndefined();

    // Refreshing makes it live again.
    a.touch("run-1", "wf-1");
    expect(b.isFresh("run-1")).toBe(true);
  });

  it("only the owner removes an entry, and stale entries are dropped on write", () => {
    const storage = memoryStorage();
    let now = 1_000;
    const a = createLiveRunRegistry({ storage, owner: "a", now: () => now });
    const b = createLiveRunRegistry({ storage, owner: "b", now: () => now });
    a.touch("run-1", "wf-1");
    b.remove("run-1");
    expect(a.isFresh("run-1")).toBe(true);
    a.remove("run-1");
    expect(b.isFresh("run-1")).toBe(false);

    a.touch("old", "wf-1");
    now += LIVE_RUN_STALE_MS + 1;
    b.touch("new", "wf-2");
    expect(Object.keys(JSON.parse(storage.values.get(LIVE_RUNS_KEY)!))).toEqual(
      ["new"],
    );
  });

  it("copes with a broken or missing store", () => {
    const storage = memoryStorage();
    storage.values.set(LIVE_RUNS_KEY, "{not json");
    const registry = createLiveRunRegistry({ storage, owner: "a" });
    expect(registry.isFresh("run-1")).toBe(false);
    registry.touch("run-1", "wf-1");
    expect(registry.isFresh("run-1")).toBe(true);
    const failing = createLiveRunRegistry({
      storage: {
        getItem: () => null,
        setItem: () => {
          throw new Error("full");
        },
      },
    });
    expect(() => failing.touch("run-1", "wf-1")).not.toThrow();
    expect(failing.isFresh("run-1")).toBe(false);
  });
});
