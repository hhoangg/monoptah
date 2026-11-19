// The model of the session tabs and split panes of the chat area (design-system.md section 11). Pure functions only:
// every operation takes a Workspace and returns a new one (the same object when nothing changes), so the store and the
// tests share one implementation.
//
// A Workspace is a tree of panes. A leaf names a pane; a split lays its children out in a row or a column with
// fractional sizes that add up to 1. Each pane has its own tab list and active tab, and exactly one pane is focused:
// the focused pane drives the URL. This follows the VS Code editor-group model; the native app keeps one pane tree per
// tab instead (crates/monocode-gpui/src/pane_layout.rs), see the "Native parity" note in design-system.md.

/** `s:<sessionId>` or `m:<monoId>`. */
export type TabKey = string;

export const sessionTab = (id: string): TabKey => `s:${id}`;
export const monoTab = (id: string): TabKey => `m:${id}`;

export function parseTab(key: TabKey): { kind: "session" | "mono"; id: string } | null {
  if (key.startsWith("s:") && key.length > 2) return { kind: "session", id: key.slice(2) };
  if (key.startsWith("m:") && key.length > 2) return { kind: "mono", id: key.slice(2) };
  return null;
}

export type SplitDir = "row" | "column";

export interface Pane {
  id: string;
  tabs: TabKey[];
  active: TabKey | null;
}

export type PaneNode = { type: "leaf"; id: string } | { type: "split"; dir: SplitDir; children: PaneNode[]; sizes: number[] };

export interface Workspace {
  root: PaneNode;
  panes: Record<string, Pane>;
  focused: string;
  /** Counter for new pane ids. */
  next: number;
}

/** A pane keeps at least this much room: the composer and the transcript column stay usable. */
export const MIN_PANE_WIDTH = 320;
export const MIN_PANE_HEIGHT = 280;
/** The most panes one workspace holds. */
export const MAX_PANES = 6;

export function emptyWorkspace(): Workspace {
  return { root: { type: "leaf", id: "p1" }, panes: { p1: { id: "p1", tabs: [], active: null } }, focused: "p1", next: 2 };
}

/** Leaf ids in reading order (left to right, top to bottom). */
export function paneOrder(node: PaneNode): string[] {
  return node.type === "leaf" ? [node.id] : node.children.flatMap(paneOrder);
}

export function findTab(ws: Workspace, key: TabKey): string | null {
  for (const id of paneOrder(ws.root)) if (ws.panes[id]?.tabs.includes(key)) return id;
  return null;
}

function setPane(ws: Workspace, pane: Pane): Workspace {
  return { ...ws, panes: { ...ws.panes, [pane.id]: pane } };
}

/** Opens `key`: focuses the pane that has it, else appends it to the focused pane. Both make it the active tab. */
export function openTab(ws: Workspace, key: TabKey): Workspace {
  const owner = findTab(ws, key);
  if (owner) return activateTab(ws, owner, key);
  const pane = ws.panes[ws.focused];
  if (!pane) return ws;
  return setPane(ws, { ...pane, tabs: [...pane.tabs, key], active: key });
}

/** Makes `key` the active tab of `paneId` and focuses the pane. */
export function activateTab(ws: Workspace, paneId: string, key: TabKey): Workspace {
  const pane = ws.panes[paneId];
  if (!pane || !pane.tabs.includes(key)) return ws;
  if (pane.active === key && ws.focused === paneId) return ws;
  return { ...setPane(ws, { ...pane, active: key }), focused: paneId };
}

export function focusPane(ws: Workspace, paneId: string): Workspace {
  return ws.panes[paneId] && ws.focused !== paneId ? { ...ws, focused: paneId } : ws;
}

/** Moves the focus `step` panes along the reading order (wraps around). */
export function focusAdjacentPane(ws: Workspace, step: 1 | -1): Workspace {
  const order = paneOrder(ws.root);
  if (order.length < 2) return ws;
  const at = Math.max(0, order.indexOf(ws.focused));
  return focusPane(ws, order[(at + step + order.length) % order.length] as string);
}

/** The tab that takes over when the tab at `index` leaves a list of `tabs`: the one on its right, else on its left. */
function neighbor(tabs: TabKey[], index: number): TabKey | null {
  return tabs[index + 1] ?? tabs[index - 1] ?? null;
}

function withoutTab(pane: Pane, key: TabKey): Pane {
  const index = pane.tabs.indexOf(key);
  if (index < 0) return pane;
  const tabs = pane.tabs.filter((tab) => tab !== key);
  return { ...pane, tabs, active: pane.active === key ? neighbor(pane.tabs, index) : pane.active };
}

/** Closes a tab. The last tab of a pane closes the pane too, unless it is the only pane. */
export function closeTab(ws: Workspace, paneId: string, key: TabKey): Workspace {
  const pane = ws.panes[paneId];
  if (!pane || !pane.tabs.includes(key)) return ws;
  const next = setPane(ws, withoutTab(pane, key));
  const left = next.panes[paneId];
  return left && left.tabs.length === 0 && paneOrder(next.root).length > 1 ? removePane(next, paneId) : next;
}

/** Removes the tabs for which `keep` is false, from every pane. */
export function pruneTabs(ws: Workspace, keep: (key: TabKey) => boolean): Workspace {
  let next = ws;
  for (const id of paneOrder(ws.root)) {
    for (const key of ws.panes[id]?.tabs ?? []) if (!keep(key)) next = closeTab(next, id, key);
  }
  return next;
}

function normalize(sizes: number[]): number[] {
  const total = sizes.reduce((sum, size) => sum + size, 0);
  return total > 0 ? sizes.map((size) => size / total) : sizes.map(() => 1 / sizes.length);
}

function dropLeaf(node: PaneNode, id: string): PaneNode | null {
  if (node.type === "leaf") return node.id === id ? null : node;
  const children: PaneNode[] = [];
  const sizes: number[] = [];
  let removedAt = -1;
  node.children.forEach((child, index) => {
    const kept = dropLeaf(child, id);
    if (kept) {
      children.push(kept);
      sizes.push(node.sizes[index] ?? 0);
    } else {
      removedAt = index;
    }
  });
  if (children.length === 0) return null;
  // The share of a removed child goes to its neighbour on the left, else on the right.
  if (removedAt >= 0) {
    const target = Math.min(Math.max(removedAt - 1, 0), sizes.length - 1);
    sizes[target] = (sizes[target] ?? 0) + (node.sizes[removedAt] ?? 0);
  }
  if (children.length === 1) return children[0] as PaneNode;
  return { ...node, children, sizes: normalize(sizes) };
}

/** Closes a pane with its tabs. The only pane cannot be removed. */
export function removePane(ws: Workspace, paneId: string): Workspace {
  const order = paneOrder(ws.root);
  if (order.length < 2 || !ws.panes[paneId]) return ws;
  const root = dropLeaf(ws.root, paneId);
  if (!root) return ws;
  const { [paneId]: _removed, ...panes } = ws.panes;
  void _removed;
  const at = order.indexOf(paneId);
  const focused = ws.focused === paneId ? (order[at + 1] ?? order[at - 1] ?? (Object.keys(panes)[0] as string)) : ws.focused;
  return { ...ws, root, panes, focused };
}

function insertBeside(node: PaneNode, targetId: string, fresh: string, dir: SplitDir, place: "before" | "after"): PaneNode {
  const leaf: PaneNode = { type: "leaf", id: fresh };
  if (node.type === "leaf") {
    if (node.id !== targetId) return node;
    const pair: PaneNode[] = place === "before" ? [leaf, node] : [node, leaf];
    return { type: "split", dir, children: pair, sizes: [0.5, 0.5] };
  }
  const at = node.children.findIndex((child) => child.type === "leaf" && child.id === targetId);
  if (at >= 0 && node.dir === dir) {
    const children = [...node.children];
    const sizes = [...node.sizes];
    const half = (sizes[at] ?? 0) / 2;
    sizes[at] = half;
    const insertAt = place === "before" ? at : at + 1;
    children.splice(insertAt, 0, leaf);
    sizes.splice(insertAt, 0, half);
    return { ...node, children, sizes };
  }
  return { ...node, children: node.children.map((child) => insertBeside(child, targetId, fresh, dir, place)) };
}

/** Adds an empty pane beside `paneId` and focuses it. Returns the same workspace at the pane limit. */
export function splitPane(ws: Workspace, paneId: string, dir: SplitDir, place: "before" | "after" = "after"): Workspace {
  if (!ws.panes[paneId] || paneOrder(ws.root).length >= MAX_PANES) return ws;
  const fresh = `p${ws.next}`;
  return {
    ...ws,
    root: insertBeside(ws.root, paneId, fresh, dir, place),
    panes: { ...ws.panes, [fresh]: { id: fresh, tabs: [], active: null } },
    focused: fresh,
    next: ws.next + 1,
  };
}

/**
 * Moves a tab to `toPane` (at `index`, default the end) and makes it active and focused there. Inside one pane it
 * reorders. The source pane closes when it ends up empty and is not the only one.
 */
export function moveTab(ws: Workspace, key: TabKey, toPane: string, index?: number): Workspace {
  const from = findTab(ws, key);
  const target = ws.panes[toPane];
  if (!from || !target) return ws;
  if (from === toPane) {
    const without = target.tabs.filter((tab) => tab !== key);
    const at = Math.min(Math.max(index ?? without.length, 0), without.length);
    const tabs = [...without.slice(0, at), key, ...without.slice(at)];
    const same = tabs.every((tab, i) => tab === target.tabs[i]);
    return same ? ws : { ...setPane(ws, { ...target, tabs }), focused: toPane };
  }
  const source = ws.panes[from] as Pane;
  let next = setPane(ws, withoutTab(source, key));
  const into = next.panes[toPane] as Pane;
  const at = Math.min(Math.max(index ?? into.tabs.length, 0), into.tabs.length);
  next = setPane(next, { ...into, tabs: [...into.tabs.slice(0, at), key, ...into.tabs.slice(at)], active: key });
  next = { ...next, focused: toPane };
  return (next.panes[from]?.tabs.length ?? 0) === 0 ? removePane(next, from) : next;
}

/** Sets the sizes of the children of the split at `path` (child indexes from the root). */
export function resizeSplit(ws: Workspace, path: number[], sizes: number[]): Workspace {
  const update = (node: PaneNode, rest: number[]): PaneNode => {
    if (node.type === "leaf") return node;
    if (rest.length === 0) return sizes.length === node.sizes.length ? { ...node, sizes: normalize(sizes) } : node;
    const [head, ...tail] = rest;
    return { ...node, children: node.children.map((child, index) => (index === head ? update(child, tail) : child)) };
  };
  return { ...ws, root: update(ws.root, path) };
}

/** The room a tree needs: a row adds the widths of its children, a column adds the heights. */
export function minSize(node: PaneNode): { width: number; height: number } {
  if (node.type === "leaf") return { width: MIN_PANE_WIDTH, height: MIN_PANE_HEIGHT };
  const parts = node.children.map(minSize);
  return node.dir === "row"
    ? { width: parts.reduce((sum, part) => sum + part.width, 0), height: Math.max(...parts.map((part) => part.height)) }
    : { width: Math.max(...parts.map((part) => part.width)), height: parts.reduce((sum, part) => sum + part.height, 0) };
}

/** True when the tree fits `size` (an unmeasured area fits anything). */
export function fits(root: PaneNode, size: { width: number; height: number } | null): boolean {
  if (!size) return true;
  const need = minSize(root);
  return size.width >= need.width && size.height >= need.height;
}

/** True when a split of the focused pane in `dir` would still fit `size`. */
export function canSplit(ws: Workspace, dir: SplitDir, size: { width: number; height: number } | null): boolean {
  const next = splitPane(ws, ws.focused, dir);
  return next !== ws && fits(next.root, size);
}

// --- persistence -----------------------------------------------------------------------------------------------

function validNode(node: unknown, panes: Record<string, Pane>, seen: Set<string>): node is PaneNode {
  if (typeof node !== "object" || node === null) return false;
  const value = node as Record<string, unknown>;
  if (value.type === "leaf") {
    if (typeof value.id !== "string" || !panes[value.id] || seen.has(value.id)) return false;
    seen.add(value.id);
    return true;
  }
  if (value.type !== "split" || (value.dir !== "row" && value.dir !== "column")) return false;
  const { children, sizes } = value;
  if (!Array.isArray(children) || !Array.isArray(sizes) || children.length < 2 || children.length !== sizes.length) return false;
  if (!sizes.every((size) => typeof size === "number" && Number.isFinite(size) && size > 0)) return false;
  return children.every((child) => validNode(child, panes, seen));
}

/** A stored workspace, or an empty one when the text is missing or not a consistent workspace. */
export function parseWorkspace(raw: string | null): Workspace {
  if (!raw) return emptyWorkspace();
  try {
    const value = JSON.parse(raw) as { v?: unknown; ws?: Partial<Workspace> };
    const ws = value.ws;
    if (value.v !== 1 || !ws || typeof ws.panes !== "object" || ws.panes === null) return emptyWorkspace();
    const panes: Record<string, Pane> = {};
    for (const [id, pane] of Object.entries(ws.panes)) {
      if (!pane || pane.id !== id || !Array.isArray(pane.tabs) || !pane.tabs.every((tab) => typeof tab === "string" && parseTab(tab))) return emptyWorkspace();
      const tabs = [...new Set(pane.tabs)];
      panes[id] = { id, tabs, active: pane.active && tabs.includes(pane.active) ? pane.active : (tabs[0] ?? null) };
    }
    const seen = new Set<string>();
    if (!validNode(ws.root, panes, seen) || seen.size !== Object.keys(panes).length) return emptyWorkspace();
    // A tab lives in one pane only.
    const owned = new Set<string>();
    for (const id of paneOrder(ws.root)) {
      const pane = panes[id] as Pane;
      const tabs = pane.tabs.filter((tab) => !owned.has(tab));
      tabs.forEach((tab) => owned.add(tab));
      panes[id] = { ...pane, tabs, active: pane.active && tabs.includes(pane.active) ? pane.active : (tabs[0] ?? null) };
    }
    const focused = typeof ws.focused === "string" && panes[ws.focused] ? ws.focused : (paneOrder(ws.root)[0] as string);
    const highest = Math.max(0, ...Object.keys(panes).map((id) => Number(/^p(\d+)$/.exec(id)?.[1] ?? 0)));
    const next = Math.max(highest + 1, typeof ws.next === "number" && Number.isInteger(ws.next) ? ws.next : 0);
    return { root: ws.root, panes, focused, next };
  } catch {
    return emptyWorkspace();
  }
}

export function serializeWorkspace(ws: Workspace): string {
  return JSON.stringify({ v: 1, ws });
}
