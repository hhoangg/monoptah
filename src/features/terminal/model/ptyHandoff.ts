/**
 * A pane moved to another tab remounts its views, and the process behind it
 * must survive that. Two records make it work, and both are scoped to the one
 * React commit that performs the move, never to a clock:
 *
 * - a departure mark, set before the state change and consumed by the view
 *   that unmounts, which then leaves the process running;
 * - an arrival record, left by that unmount and claimed by the view that
 *   mounts for the same id in the same commit, which then attaches instead of
 *   spawning.
 *
 * `settlePtyHandoffs` drops whatever the commit did not use, so a mark that no
 * view consumed cannot spare a later, ordinary close from its kill, and a
 * mount that was never handed anything always spawns.
 */
const departing = new Set<string>();
const arriving = new Set<string>();
const arrivedExits = new Map<string, PtyHandoffExit>();

/** What a view had already told its owner about a process that ended. */
export type PtyHandoffExit = { code: number | null; error?: string };

/**
 * Says the panes for these PTY ids are about to move to a new view, so the
 * view that unmounts must leave the processes running. Call it synchronously
 * before the state change that moves them.
 */
export function markPtyHandoff(ids: Iterable<string>): void {
  for (const id of ids) departing.add(id);
}

/**
 * For a view that is unmounting: true once per mark. A true result also
 * records the arrival for the view that mounts next. Pass the exit the view
 * already reported, when it did: the process is gone, and the next view must
 * show that exit rather than fail to reattach to it.
 */
export function consumePtyHandoff(id: string, exit?: PtyHandoffExit): boolean {
  if (!departing.delete(id)) return false;
  arriving.add(id);
  if (exit) arrivedExits.set(id, exit);
  return true;
}

/**
 * For a view that is mounting: true when this id was just handed off, so its
 * process is still running and must be attached to rather than spawned.
 */
export function claimPtyArrival(id: string): boolean {
  return arriving.delete(id);
}

/**
 * For a view that is mounting: the exit the departing view had already
 * reported for this id, once. Undefined when the process was still running.
 */
export function takeArrivedExit(id: string): PtyHandoffExit | undefined {
  const exit = arrivedExits.get(id);
  arrivedExits.delete(id);
  return exit;
}

/**
 * Drops every unused record. Run it after the commit that carried the move has
 * run its effects: unmount cleanups and mount effects of one commit all run
 * before a parent's effect does.
 */
export function settlePtyHandoffs(): void {
  departing.clear();
  arriving.clear();
  arrivedExits.clear();
}
