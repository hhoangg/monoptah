/** Creates a session and runs `place` with its id exactly once. Creation may
 *  be deferred (a provider prompt), in which case `create` returns nothing and
 *  calls the continuation later; if the cancelled prompt never fires it, no
 *  placement happens. A creator that only returns the id is also supported. */
export function createSessionThen(
  create: (onCreated: (id: string) => void) => string | void,
  place: (sessionId: string) => void,
): void {
  let placed = false;
  const once = (id: string) => {
    if (placed || !id) return;
    placed = true;
    place(id);
  };
  const id = create(once);
  if (id) once(id);
}
