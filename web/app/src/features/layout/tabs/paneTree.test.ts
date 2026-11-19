import { describe, expect, it } from "vitest";
import {
  MAX_PANES,
  activateTab,
  canSplit,
  closeTab,
  emptyWorkspace,
  findTab,
  fits,
  focusAdjacentPane,
  minSize,
  moveTab,
  openTab,
  paneOrder,
  parseWorkspace,
  pruneTabs,
  removePane,
  resizeSplit,
  serializeWorkspace,
  sessionTab,
  splitPane,
  type Workspace,
} from "./paneTree";

const A = sessionTab("a");
const B = sessionTab("b");
const C = sessionTab("c");

function opened(...keys: string[]): Workspace {
  return keys.reduce((ws, key) => openTab(ws, key), emptyWorkspace());
}

describe("tabs", () => {
  it("opens a tab at the end of the focused pane and makes it active; opening it again only activates it", () => {
    const ws = opened(A, B);
    expect(ws.panes.p1).toMatchObject({ tabs: [A, B], active: B });
    expect(openTab(activateTab(ws, "p1", A), B).panes.p1?.active).toBe(B);
    expect(openTab(ws, B)).toBe(ws);
  });

  it("closes the active tab onto its right neighbour, else its left one, and the last tab leaves an empty pane", () => {
    let ws = activateTab(opened(A, B, C), "p1", B);
    ws = closeTab(ws, "p1", B);
    expect(ws.panes.p1).toMatchObject({ tabs: [A, C], active: C });
    ws = closeTab(ws, "p1", C);
    expect(ws.panes.p1).toMatchObject({ tabs: [A], active: A });
    ws = closeTab(ws, "p1", A);
    expect(ws.panes.p1).toMatchObject({ tabs: [], active: null });
    expect(closeTab(ws, "p1", A)).toBe(ws);
  });

  it("reorders inside a pane by the index in the list without the tab", () => {
    const ws = opened(A, B, C);
    expect(moveTab(ws, C, "p1", 0).panes.p1?.tabs).toEqual([C, A, B]);
    expect(moveTab(ws, A, "p1", 2).panes.p1?.tabs).toEqual([B, C, A]);
    expect(moveTab(ws, A, "p1", 0)).toBe(ws);
  });

  it("drops the tabs a predicate rejects", () => {
    const ws = pruneTabs(opened(A, B, C), (key) => key !== B);
    expect(ws.panes.p1?.tabs).toEqual([A, C]);
  });
});

describe("panes", () => {
  it("splits the focused pane right or down, focuses the new empty pane, and divides the space equally", () => {
    const row = splitPane(opened(A), "p1", "row");
    expect(row.root).toMatchObject({ type: "split", dir: "row", sizes: [0.5, 0.5] });
    expect(row.focused).toBe("p2");
    expect(row.panes.p2).toEqual({ id: "p2", tabs: [], active: null });
    const three = splitPane(row, "p2", "row");
    expect(three.root).toMatchObject({ type: "split", dir: "row", sizes: [0.5, 0.25, 0.25] });
    const nested = splitPane(three, "p3", "column");
    expect(paneOrder(nested.root)).toEqual(["p1", "p2", "p3", "p4"]);
    const placed = splitPane(opened(A), "p1", "row", "before");
    expect(paneOrder(placed.root)).toEqual(["p2", "p1"]);
  });

  it("stops at the pane limit", () => {
    let ws = opened(A);
    for (let i = 0; i < MAX_PANES + 2; i++) ws = splitPane(ws, ws.focused, "row");
    expect(paneOrder(ws.root)).toHaveLength(MAX_PANES);
  });

  it("moves a tab to another pane, activates and focuses it there, and closes a source pane left empty", () => {
    let ws = splitPane(opened(A, B), "p1", "row");
    ws = moveTab(ws, B, "p2");
    expect(ws.panes.p1).toMatchObject({ tabs: [A], active: A });
    expect(ws.panes.p2).toMatchObject({ tabs: [B], active: B });
    expect(ws.focused).toBe("p2");
    ws = moveTab(ws, A, "p2", 0);
    expect(paneOrder(ws.root)).toEqual(["p2"]);
    expect(ws.panes.p2?.tabs).toEqual([A, B]);
    expect(ws.root).toEqual({ type: "leaf", id: "p2" });
  });

  it("closing the last tab of a pane removes the pane unless it is the only one", () => {
    const ws = splitPane(opened(A), "p1", "row");
    const withTab = openTab(ws, B);
    expect(withTab.panes.p2?.tabs).toEqual([B]);
    const closed = closeTab(withTab, "p2", B);
    expect(paneOrder(closed.root)).toEqual(["p1"]);
    expect(closed.focused).toBe("p1");
  });

  it("removePane gives the freed size to the neighbour and keeps the only pane", () => {
    let ws = splitPane(splitPane(opened(A), "p1", "row"), "p2", "row");
    ws = resizeSplit(ws, [], [0.5, 0.3, 0.2]);
    const removed = removePane(ws, "p2");
    expect(removed.root).toMatchObject({ sizes: [0.8, 0.2] });
    expect(removePane(removePane(removed, "p3"), "p1")).toEqual(removePane(removed, "p3"));
  });

  it("finds the pane of a tab and moves the focus along the reading order", () => {
    const ws = openTab(splitPane(opened(A), "p1", "row"), B);
    expect(findTab(ws, A)).toBe("p1");
    expect(findTab(ws, B)).toBe("p2");
    expect(focusAdjacentPane(ws, 1).focused).toBe("p1");
    expect(focusAdjacentPane(focusAdjacentPane(ws, 1), -1).focused).toBe("p2");
  });
});

describe("room", () => {
  it("needs the sum of the minimums along a row and along a column", () => {
    const row = splitPane(opened(A), "p1", "row");
    expect(minSize(row.root).width).toBe(640);
    const column = splitPane(opened(A), "p1", "column");
    expect(minSize(column.root).height).toBe(560);
    expect(fits(row.root, { width: 700, height: 400 })).toBe(true);
    expect(fits(row.root, { width: 600, height: 400 })).toBe(false);
    expect(fits(row.root, null)).toBe(true);
  });

  it("allows a split only when the result fits", () => {
    const ws = opened(A);
    expect(canSplit(ws, "row", { width: 700, height: 500 })).toBe(true);
    expect(canSplit(ws, "row", { width: 500, height: 500 })).toBe(false);
    expect(canSplit(ws, "column", { width: 500, height: 500 })).toBe(false);
    expect(canSplit(ws, "column", { width: 500, height: 600 })).toBe(true);
  });
});

describe("persistence format", () => {
  it("round trips a workspace", () => {
    const ws = openTab(splitPane(opened(A, B), "p1", "column"), C);
    expect(parseWorkspace(serializeWorkspace(ws))).toEqual(ws);
  });

  it.each([null, "", "nope", "{}", '{"v":2}', '{"v":1,"ws":{"panes":{}}}'])("starts empty from %s", (raw) => {
    expect(parseWorkspace(raw)).toEqual(emptyWorkspace());
  });

  it("starts empty when the tree and the panes disagree, and drops tabs it cannot read or that repeat", () => {
    const ws = splitPane(opened(A), "p1", "row");
    const broken = JSON.parse(serializeWorkspace(ws));
    delete broken.ws.panes.p2;
    expect(parseWorkspace(JSON.stringify(broken))).toEqual(emptyWorkspace());
    const bad = JSON.parse(serializeWorkspace(opened(A)));
    bad.ws.panes.p1.tabs = ["x:1"];
    expect(parseWorkspace(JSON.stringify(bad))).toEqual(emptyWorkspace());
    const twice = JSON.parse(serializeWorkspace(openTab(splitPane(opened(A), "p1", "row"), B)));
    twice.ws.panes.p2.tabs = [A, B];
    expect(parseWorkspace(JSON.stringify(twice)).panes.p2?.tabs).toEqual([B]);
  });
});

describe("drops and room", () => {
  it("a tab dropped on its own pane with no place in the strip stays where it is", () => {
    const ws = opened(A, B, C);
    expect(moveTab(ws, A, "p1")).toBe(ws);
    expect(moveTab(ws, B, "p1")).toBe(ws);
    // With a place it still reorders.
    expect(moveTab(ws, A, "p1", 2).panes.p1?.tabs).toEqual([B, C, A]);
  });

  it("canSplit measures the pane it is asked about, not only the focused one", () => {
    // A row of p1 and a column of p2 over p3: splitting p2 downwards stacks three, splitting p1 downwards does not.
    let ws = splitPane(opened(A, B, C), "p1", "row");
    ws = splitPane(ws, "p2", "column");
    expect(ws.focused).toBe("p3");
    const size = { width: 2000, height: 700 };
    expect(canSplit(ws, "column", size, "p1")).toBe(true);
    expect(canSplit(ws, "column", size, "p2")).toBe(false);
    expect(canSplit(ws, "column", size)).toBe(false);
    expect(canSplit(ws, "column", size, "missing")).toBe(false);
  });
});
