// `conversation.older` paging (F-32), owned by WS-T. The data layer (WS-L) opens a conversation on its
// latest turns and tells whether earlier ones exist (`state.older`); it has no call that prepends a page.
// Until it does, pages are kept here and merged in front of the watched turns at render time.
// Seam for WS-L: a `ConversationWatcher.loadOlder()` that prepends to `state.turns` (and keeps `from`
// of the reopen params at the first turn held) would replace this hook; see the Task 4 report.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Turn } from "@monoptah/protocol";
import { useLinkConnection } from "@/link";

/** How many turns one page asks for (F-32: older 20 at a time). */
export const OLDER_PAGE_SIZE = 20;

export interface OlderTurns {
  /** The older pages followed by the watched turns, without duplicates. */
  turns: Turn[];
  hasOlder: boolean;
  loading: boolean;
  error: string | null;
  /**
   * Fetches the page before the first turn held. Does nothing (false) while one is in flight, while the link is
   * not ready (a request then rejects at once) or when there is no older page.
   */
  loadOlder: () => Promise<boolean>;
}

/** Merges older pages in front of `turns`: a turn the watch also holds wins, ids are never repeated. */
export function mergeOlder(pages: Turn[], turns: Turn[]): Turn[] {
  if (pages.length === 0) return turns;
  const held = new Set(turns.map((turn) => turn.id));
  const seen = new Set<string>();
  const older: Turn[] = [];
  for (const turn of pages) {
    if (held.has(turn.id) || seen.has(turn.id)) continue;
    seen.add(turn.id);
    older.push(turn);
  }
  return older.length === 0 ? turns : [...older, ...turns];
}

interface PageState {
  scope: object;
  pages: Turn[];
  /** What the last page said; null until a page was fetched (then `older` of the watch decides). */
  more: boolean | null;
  loading: boolean;
  error: string | null;
}

const fresh = (scope: object): PageState => ({ scope, pages: [], more: null, loading: false, error: null });

export function useOlderTurns(conversationId: string, turns: Turn[], older: boolean): OlderTurns {
  const client = useLinkConnection();
  // One token per conversation and link (compared by identity): another conversation, or a new link, carries nothing over.
  const scope = useMemo(() => ({ conversationId, client }), [conversationId, client]);
  const [data, setData] = useState<PageState>(() => fresh(scope));
  const scopeRef = useRef(scope);
  useEffect(() => {
    scopeRef.current = scope;
  }, [scope]);
  const inFlight = useRef<object | null>(null);

  const current = data.scope === scope ? data : fresh(scope);
  const patch = useCallback(
    (mine: object, change: (state: PageState) => Partial<PageState>) =>
      setData((previous) => {
        const base = previous.scope === mine ? previous : fresh(mine);
        return { ...base, ...change(base) };
      }),
    [],
  );

  const merged = useMemo(() => mergeOlder(current.pages, turns), [current.pages, turns]);
  const hasOlder = current.more ?? older;
  const firstId = merged[0]?.id;

  const loadOlder = useCallback(async () => {
    if (client.store.getState().status !== "ready" || inFlight.current === scope || !hasOlder || !firstId) return false;
    inFlight.current = scope;
    patch(scope, () => ({ loading: true, error: null }));
    try {
      const result = await client.request("conversation.older", { conversationId, before: firstId, limit: OLDER_PAGE_SIZE });
      if (scopeRef.current !== scope) return true;
      patch(scope, (state) => ({ pages: [...result.turns, ...state.pages], more: result.older && result.turns.length > 0 }));
    } catch (reason) {
      if (scopeRef.current !== scope) return true;
      patch(scope, () => ({ error: reason instanceof Error ? reason.message : String(reason) }));
    } finally {
      if (inFlight.current === scope) inFlight.current = null;
      if (scopeRef.current === scope) patch(scope, () => ({ loading: false }));
    }
    return true;
  }, [client, conversationId, firstId, hasOlder, patch, scope]);

  return { turns: merged, hasOlder, loading: current.loading, error: current.error, loadOlder };
}
