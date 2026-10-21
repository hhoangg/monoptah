import { describe, expect, it } from "vitest";
import {
  EMPTY_STARTED_RAIL,
  startedRailOpen,
  startedRailSets,
  syncStartedRail,
  toggleStartedRail,
  type StartedRailState,
} from "./startedRail";

const sync = (
  current: StartedRailState,
  {
    auto = [],
    latch = auto,
    parents = ["a", "b"],
    entered = [],
  }: {
    auto?: string[];
    latch?: string[];
    parents?: string[];
    entered?: string[];
  },
) =>
  syncStartedRail(current, {
    auto: new Set(auto),
    latch: new Set(latch),
    parents,
    entered: new Set(entered),
  });
const state = (open: string[], closed: string[]): StartedRailState => ({
  open: new Set(open),
  closed: new Set(closed),
});

describe("startedRailOpen", () => {
  it("is closed by default and open while the thread is in play", () => {
    expect(startedRailOpen(EMPTY_STARTED_RAIL, "a", false)).toBe(false);
    expect(startedRailOpen(EMPTY_STARTED_RAIL, "a", true)).toBe(true);
  });

  it("stays closed while in play once the user collapsed it", () => {
    expect(startedRailOpen(state([], ["a"]), "a", true)).toBe(false);
  });

  it("stays open when no longer in play once opened", () => {
    expect(startedRailOpen(state(["a"], []), "a", false)).toBe(true);
  });
});

describe("syncStartedRail", () => {
  it("keeps an auto-opened rail open when another thread becomes active", () => {
    let current = sync(EMPTY_STARTED_RAIL, { auto: ["a"] });
    current = sync(current, { auto: ["b"] });
    expect(startedRailOpen(current, "a", false)).toBe(true);
    expect(startedRailOpen(current, "b", true)).toBe(true);
  });

  it("keeps a user collapse while the thread is still in play", () => {
    let current = toggleStartedRail(EMPTY_STARTED_RAIL, "a", true);
    current = sync(current, { auto: ["a"], parents: ["a"] });
    expect(startedRailOpen(current, "a", true)).toBe(false);
    expect(current.open.has("a")).toBe(false);
  });

  it("clears the collapse once the thread is out of play so it reopens", () => {
    let current = toggleStartedRail(EMPTY_STARTED_RAIL, "a", true);
    current = sync(current, { parents: ["a"] });
    expect(current.closed.has("a")).toBe(false);
    expect(startedRailOpen(current, "a", false)).toBe(false);
    expect(startedRailOpen(current, "a", true)).toBe(true);
  });

  it("returns the identical object when nothing changes", () => {
    const first = sync(EMPTY_STARTED_RAIL, { auto: ["a"] });
    expect(sync(first, { auto: ["a"] })).toBe(first);
    expect(sync(EMPTY_STARTED_RAIL, { parents: ["a"] })).toBe(
      EMPTY_STARTED_RAIL,
    );
  });

  it("leaves unrelated ids untouched", () => {
    const current = state(["x"], ["y"]);
    const next = sync(current, { auto: ["a"], parents: ["a"] });
    expect(next.open.has("x")).toBe(true);
    expect(next.open.has("a")).toBe(true);
    expect(next.closed.has("y")).toBe(true);
  });

  it("shows a busy-only parent open but does not latch it", () => {
    const current = sync(EMPTY_STARTED_RAIL, { auto: ["a"], latch: [] });
    expect(current).toBe(EMPTY_STARTED_RAIL);
    expect(startedRailOpen(current, "a", true)).toBe(true);
    // Busy ends: no longer auto, and it was never latched, so it collapses.
    expect(startedRailOpen(sync(current, {}), "a", false)).toBe(false);
  });

  it("reopens a collapsed in-play rail when focus enters its group", () => {
    let current = toggleStartedRail(EMPTY_STARTED_RAIL, "a", true);
    current = sync(current, { auto: ["a"], parents: ["a"] });
    expect(startedRailOpen(current, "a", true)).toBe(false);
    current = sync(current, {
      auto: ["a"],
      parents: ["a"],
      entered: ["a"],
    });
    expect(startedRailOpen(current, "a", true)).toBe(true);
    expect(current.open.has("a")).toBe(true);
    expect(current.closed.has("a")).toBe(false);
  });

  it("does not reopen a collapsed rail without an entry", () => {
    const collapsed = state([], ["a"]);
    const next = sync(collapsed, { auto: ["a"], parents: ["a"] });
    expect(next).toBe(collapsed);
  });

  it("does not mutate the previous state", () => {
    const current = state([], ["a"]);
    sync(current, { parents: ["a"] });
    expect(current.closed.has("a")).toBe(true);
  });
});

describe("toggleStartedRail", () => {
  it("opens a collapsed rail and collapses it again on a second toggle", () => {
    const opened = toggleStartedRail(EMPTY_STARTED_RAIL, "a", false);
    expect(startedRailOpen(opened, "a", false)).toBe(true);
    const closed = toggleStartedRail(opened, "a", false);
    expect(startedRailOpen(closed, "a", false)).toBe(false);
    expect(closed.closed.has("a")).toBe(true);
  });

  it("collapses an auto-open rail and reopens it on the next toggle", () => {
    const closed = toggleStartedRail(EMPTY_STARTED_RAIL, "a", true);
    expect(startedRailOpen(closed, "a", true)).toBe(false);
    const reopened = toggleStartedRail(closed, "a", true);
    expect(startedRailOpen(reopened, "a", true)).toBe(true);
  });

  it("does not touch other rails", () => {
    const next = toggleStartedRail(state(["x"], ["y"]), "a", false);
    expect(next.open.has("x")).toBe(true);
    expect(next.closed.has("y")).toBe(true);
  });
});

describe("startedRailSets", () => {
  const byParent = new Map([
    ["a", [{ id: "a1" }]],
    ["o", [{ id: "o1" }]],
  ]);
  const sets = (over: Partial<Parameters<typeof startedRailSets>[1]>) =>
    startedRailSets(byParent, {
      skip: (id) => id === "o",
      inView: () => false,
      busy: () => false,
      highlighted: undefined,
      previousHighlighted: undefined,
      ...over,
    });

  it("leaves skipped parents out of every set", () => {
    const result = sets({ inView: () => true, highlighted: "o1" });
    expect([...result.parents]).toEqual(["a"]);
    expect(result.latch.has("o")).toBe(false);
    expect(result.entered.has("o")).toBe(false);
  });

  it("latches in-view parents but only shows busy ones", () => {
    const busy = sets({ busy: (id) => id === "a1" });
    expect(busy.auto.has("a")).toBe(true);
    expect(busy.latch.has("a")).toBe(false);
    const view = sets({ inView: (id) => id === "a1" });
    expect(view.latch.has("a")).toBe(true);
  });

  it("enters a group only when the focused id changes", () => {
    const moved = sets({ highlighted: "a1", previousHighlighted: "x" });
    expect(moved.entered.has("a")).toBe(true);
    const same = sets({ highlighted: "a1", previousHighlighted: "a1" });
    expect(same.entered.size).toBe(0);
    const selectionOnly = sets({
      inView: (id) => id === "a1",
      highlighted: "x",
      previousHighlighted: "x",
    });
    expect(selectionOnly.entered.size).toBe(0);
  });
});
