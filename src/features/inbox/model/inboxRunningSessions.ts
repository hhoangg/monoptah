/** How many of an Inbox item's related sessions are running a turn right now. */
export function runningSessionCount(
  related: readonly { id: string }[],
  busySessionIds: ReadonlySet<string> | undefined,
): number {
  if (!busySessionIds || busySessionIds.size === 0) return 0;
  return related.filter((session) => busySessionIds.has(session.id)).length;
}

/** Row badge text and aria copy for an item's related threads. */
export function relatedThreadsLabel(count: number, running: number): string {
  const base = `${count} related ${count === 1 ? "thread" : "threads"}`;
  return running > 0 ? `${base}, ${running} running` : base;
}
