/**
 * Open state of the rails of started sessions in the inset sidebar.
 *
 * A thread's rail opens on its own while the thread or one of its sessions is
 * in play (in view, chosen or working), and then stays open until the user
 * collapses it, so several rails can be open at once. `open` holds rails that
 * stay open on their own; `closed` holds user collapses, which only suppress
 * auto-opening while the thread is still in play.
 */
export type StartedRailState = {
  open: ReadonlySet<string>;
  closed: ReadonlySet<string>;
};

export const EMPTY_STARTED_RAIL: StartedRailState = {
  open: new Set(),
  closed: new Set(),
};

export function startedRailOpen(
  state: StartedRailState,
  id: string,
  auto: boolean,
): boolean {
  return state.open.has(id) || (auto && !state.closed.has(id));
}

export type StartedRailSync = {
  /** Parents whose rail shows open for now: latch plus working ones. */
  auto: ReadonlySet<string>;
  /** Parents whose own row or a child is in view or chosen: kept open once the
   * user moves away. A working session alone does not latch. */
  latch: ReadonlySet<string>;
  /** Every parent with a rail. */
  parents: Iterable<string>;
  /** Parents whose group the focused session just moved into: reopened even
   * if the user had collapsed them. */
  entered: ReadonlySet<string>;
};

/** Latches in-view rails open, reopens entered ones and forgets collapses of
 * threads that are no longer in play. Returns `state` itself when nothing
 * changes. */
export function syncStartedRail(
  state: StartedRailState,
  { auto, latch, parents, entered }: StartedRailSync,
): StartedRailState {
  let open: Set<string> | null = null;
  let closed: Set<string> | null = null;
  for (const id of entered) {
    if (!state.open.has(id)) (open ??= new Set(state.open)).add(id);
    if (state.closed.has(id)) (closed ??= new Set(state.closed)).delete(id);
  }
  for (const id of latch) {
    if ((closed ?? state.closed).has(id) || (open ?? state.open).has(id))
      continue;
    (open ??= new Set(state.open)).add(id);
  }
  for (const id of parents) {
    if (auto.has(id) || !(closed ?? state.closed).has(id)) continue;
    (closed ??= new Set(state.closed)).delete(id);
  }
  if (!open && !closed) return state;
  return { open: open ?? state.open, closed: closed ?? state.closed };
}

/** The sets `syncStartedRail` takes, from what the sidebar shows. `skip`
 * leaves out parents with a rail of their own kind (orchestrators). `entered`
 * needs the previous render's focused id: only a change of it counts. */
export function startedRailSets(
  byParent: ReadonlyMap<string, readonly { id: string }[]>,
  {
    skip,
    inView,
    busy,
    highlighted,
    previousHighlighted,
  }: {
    skip: (id: string) => boolean;
    inView: (id: string) => boolean;
    busy: (id: string) => boolean;
    highlighted: string | null | undefined;
    previousHighlighted: string | null | undefined;
  },
): StartedRailSync {
  const parents = [...byParent.keys()].filter((id) => !skip(id));
  const auto = new Set<string>();
  const latch = new Set<string>();
  const entered = new Set<string>();
  for (const id of parents) {
    const children = byParent.get(id) ?? [];
    if (inView(id) || children.some((child) => inView(child.id))) latch.add(id);
    if (latch.has(id) || busy(id) || children.some((c) => busy(c.id)))
      auto.add(id);
    if (
      highlighted != null &&
      highlighted !== previousHighlighted &&
      (id === highlighted || children.some((child) => child.id === highlighted))
    )
      entered.add(id);
  }
  return { auto, latch, parents, entered };
}

export function toggleStartedRail(
  state: StartedRailState,
  id: string,
  auto: boolean,
): StartedRailState {
  const open = new Set(state.open);
  const closed = new Set(state.closed);
  if (startedRailOpen(state, id, auto)) {
    open.delete(id);
    closed.add(id);
  } else {
    open.add(id);
    closed.delete(id);
  }
  return { open, closed };
}
