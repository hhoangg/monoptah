// The virtualized transcript (F-32, F-33): turns and the optimistic sends of the data layer as rows of a
// TanStack Virtual list in one scroller. It opens at the end, follows the reply while the reader is at the
// end, shows the jump-to-bottom button 160 px away from it, pages older turns in when the reader reaches
// the top and keeps the reader's place while they land (prependAnchor.ts, useReplyReflowAnchor).
//
// Like mobile, a message the user sends is anchored: it rests near the top of the scroller with blank room
// below it for the reply (`blank`), and the reply grows into that room before the list scrolls. Opening an
// idle session still lands at the end of the true height; a session that is working at open anchors at once.
// The room is sized so the anchor position is the end of the scroll height, so the reader "follows the end"
// the whole time and only a reader who scrolled away keeps their place.
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Turn } from "@monoptah/protocol";
import type { OptimisticTurn } from "@/data";
import { sendErrorMessage } from "@/features/composer/sendErrors";
import { cn } from "@/lib/utils";
import { useStableValue } from "@/lib/useStableValue";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { TranscriptActionsContext, TranscriptOpened, type TranscriptActions, type TranscriptAgent } from "./context";
import { captureAnchor, restoreAnchor, type ScrollAnchor } from "./prependAnchor";
import { isAwayFromBottom, TranscriptJumpToBottom, useTranscriptJumpVisibility } from "./TranscriptJumpToBottom";
import { TranscriptTurn } from "./TranscriptTurn";
import { SessionErrorDialog } from "./TurnDialogs";
import { useReplyReflowAnchor } from "./useReplyReflowAnchor";
import type { NoticeItem } from "./turnItems";

/** The reader counts as following the reply while within this distance of the end. */
const FOLLOW_PX = 48;
/** Paging starts once the reader is within this distance of the top. */
const OLDER_TRIGGER_PX = 320;
const ESTIMATE_ROW_PX = 140;
const PADDING_PX = 16;
/** The anchored message rests this far under the top of the scroller. */
const ANCHOR_REST_PX = 8;

interface Row {
  key: string;
  turn: Turn;
  optimistic?: OptimisticTurn;
}

export interface TranscriptProps {
  turns: Turn[];
  /** Messages the user sent that the desktop transcript does not hold yet. */
  optimistic?: readonly OptimisticTurn[];
  /** The conversation has not delivered anything yet: a spinner shows after 250 ms. */
  loading?: boolean;
  hasOlder?: boolean;
  loadingOlder?: boolean;
  olderError?: string | null;
  loadOlder?: () => Promise<boolean | void>;
  /** "Not sent, retry": sends the message again. Call it from the user's press only. */
  onRetry?: (optimisticId: string) => void;
  agent?: TranscriptAgent;
  actions?: TranscriptActions;
  /** The session is working: announced politely once. */
  busy?: boolean;
  className?: string;
}

const rememberNothing = () => {};

const hasUser = (row: Row) => row.turn.items.some((item) => item.type === "user");

function lastUserRow(rows: Row[]): number {
  for (let index = rows.length - 1; index >= 0; index -= 1) if (hasUser(rows[index]!)) return index;
  return -1;
}

function sameRows(a: Row[], b: Row[]): boolean {
  return a.length === b.length && a.every((row, index) => row.key === b[index]!.key && row.turn === b[index]!.turn && row.optimistic === b[index]!.optimistic);
}

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function Transcript({
  turns,
  optimistic = [],
  loading = false,
  hasOlder = false,
  loadingOlder = false,
  olderError = null,
  loadOlder,
  onRetry,
  agent,
  actions,
  busy = false,
  className,
}: TranscriptProps) {
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const [selectedError, setSelectedError] = useState<NoticeItem | null>(null);
  const [errorOpen, setErrorOpen] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const [opened, setOpened] = useState(false);
  const following = useRef(true);
  const adjustments = useRef<number | undefined>(undefined);
  const visibility = useTranscriptJumpVisibility();

  const synthetic = useMemo(
    () =>
      optimistic.map((turn) => ({
        turn: {
          id: turn.id,
          items: [{ type: "user", id: turn.id, text: turn.text, at: turn.at, ...(turn.attachments ? { attachments: turn.attachments } : {}) }],
        } satisfies Turn,
        optimistic: turn,
      })),
    [optimistic],
  );
  const built = useMemo<Row[]>(
    () => [...turns.map((turn) => ({ key: turn.id, turn })), ...synthetic.map(({ turn, optimistic: source }) => ({ key: turn.id, turn, optimistic: source }))],
    [turns, synthetic],
  );
  const rows = useStableValue(built, sameRows);

  // Anchoring (F-32): switched on by a message sent here, by a newer user message from elsewhere, or by a
  // session that is working when its turns first show. The confirmation of a pending message (same place,
  // desktop id instead of `pending:`) does not anchor again. Decided during render, so the room under the
  // message exists in the same pass that adds it.
  const anchorIndex = lastUserRow(rows);
  const latestUserKey = anchorIndex >= 0 ? rows[anchorIndex]!.key : undefined;
  const [anchoring, setAnchoring] = useState(false);
  const [seen, setSeen] = useState<{ latest: string | undefined; optimistic: number; settled: boolean }>({ latest: undefined, optimistic: 0, settled: false });
  if (seen.latest !== latestUserKey || seen.optimistic !== optimistic.length || (!seen.settled && rows.length > 0)) {
    const sentHere = optimistic.length > seen.optimistic;
    const confirmation = seen.latest?.startsWith("pending:") === true && latestUserKey !== undefined && !latestUserKey.startsWith("pending:");
    const newer = seen.latest !== undefined && latestUserKey !== undefined && latestUserKey !== seen.latest && !confirmation;
    const openedBusy = !seen.settled && rows.length > 0 && busy && latestUserKey !== undefined;
    setSeen({ latest: latestUserKey, optimistic: optimistic.length, settled: seen.settled || rows.length > 0 });
    if (sentHere || newer || openedBusy) setAnchoring(true);
  }

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller,
    estimateSize: () => ESTIMATE_ROW_PX,
    overscan: 6,
    paddingStart: PADDING_PX,
    paddingEnd: PADDING_PX,
    getItemKey: (index) => rows[index]?.key ?? index,
    // jsdom has no layout: without a size the window would be empty.
    initialRect: { width: 800, height: 800 },
  });
  const measuredTotal = virtualizer.getTotalSize();
  const anchorStart = anchoring && anchorIndex >= 0 ? virtualizer.measurementsCache[anchorIndex]?.start : undefined;
  // The blank room under the anchored message: the anchor position is exactly the end of the scroll height.
  const blank = anchorStart !== undefined && scroller ? Math.max(0, Math.max(0, anchorStart - ANCHOR_REST_PX) + scroller.clientHeight - measuredTotal) : 0;
  const total = measuredTotal + blank;

  // Re-evaluates the end-follow state and the jump button from the scroller's geometry.
  const syncScroll = useCallback(() => {
    if (!scroller) return;
    following.current = !isAwayFromBottom(scroller, FOLLOW_PX);
    visibility.setVisible(isAwayFromBottom(scroller));
  }, [scroller, visibility]);

  const scrollToEnd = useCallback(
    (smooth: boolean) => {
      if (!scroller) return;
      const target = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
      adjustments.current = target;
      if (smooth && !prefersReducedMotion() && typeof scroller.scrollTo === "function") scroller.scrollTo({ top: target, behavior: "smooth" });
      else scroller.scrollTop = target;
    },
    [scroller],
  );

  // Open at the end, then keep to it while the reader follows the reply.
  useLayoutEffect(() => {
    if (!scroller || rows.length === 0) return;
    if (following.current) scrollToEnd(false);
    visibility.setVisible(isAwayFromBottom(scroller));
  }, [scroller, rows.length, total, scrollToEnd, visibility]);

  // Reveal after the first scroll to the end, so the list appears in place (design-system.md: 280 ms).
  useEffect(() => {
    if (revealed || rows.length === 0 || !scroller) return;
    const frame = requestAnimationFrame(() => setRevealed(true));
    return () => cancelAnimationFrame(frame);
  }, [revealed, rows.length, scroller]);

  // What is there on open shows as is; replies that arrive after it fade in.
  useEffect(() => {
    if (opened || rows.length === 0) return;
    const timer = setTimeout(() => setOpened(true), 0);
    return () => clearTimeout(timer);
  }, [opened, rows.length]);

  // A message the user sends goes to the end.
  const optimisticCount = optimistic.length;
  const previousOptimistic = useRef(optimisticCount);
  useEffect(() => {
    if (optimisticCount > previousOptimistic.current) {
      following.current = true;
      scrollToEnd(true);
    }
    previousOptimistic.current = optimisticCount;
  }, [optimisticCount, scrollToEnd]);

  // Older turns: capture the reader's row before asking, restore it once the rows above landed.
  const pending = useRef<{ anchor: ScrollAnchor; firstKey: string | undefined } | null>(null);
  // After a failed page nothing asks again by itself: only the Retry button (`manual`) does (no request loop).
  const requestOlder = useCallback((manual = false) => {
    if (!scroller || !loadOlder || !hasOlder || loadingOlder || !revealed || pending.current) return;
    if (olderError && !manual) return;
    const measured = virtualizer.measurementsCache.map((item) => ({ key: String(item.key), start: item.start }));
    const anchor = captureAnchor(measured, (index) => virtualizer.measurementsCache[index]?.size ?? 0, scroller.scrollTop);
    if (!anchor) return;
    const waiting = { anchor, firstKey: rows[0]?.key };
    pending.current = waiting;
    // `false`: nothing was asked (the link is not ready), so no anchor is left waiting.
    void loadOlder().then(
      (asked) => {
        if (asked === false && pending.current === waiting) pending.current = null;
      },
      () => undefined,
    );
  }, [scroller, loadOlder, hasOlder, loadingOlder, olderError, revealed, virtualizer, rows]);

  useLayoutEffect(() => {
    const waiting = pending.current;
    if (!waiting || !scroller || rows[0]?.key === waiting.firstKey) return;
    pending.current = null;
    const top = restoreAnchor(
      virtualizer.measurementsCache.map((item) => ({ key: String(item.key), start: item.start })),
      waiting.anchor,
    );
    if (top === null) return;
    adjustments.current = top;
    scroller.scrollTop = top;
  }, [rows, scroller, virtualizer]);

  // A page that failed or held nothing new leaves no anchor behind.
  useEffect(() => {
    if (!loadingOlder && pending.current && pending.current.firstKey === rows[0]?.key) pending.current = null;
  }, [loadingOlder, rows]);

  const onScroll = useCallback(() => {
    syncScroll();
    if (scroller && scroller.scrollTop < OLDER_TRIGGER_PX) requestOlder();
  }, [scroller, syncScroll, requestOlder]);

  // The history of a short transcript may not fill the viewport: ask for more without a scroll.
  useEffect(() => {
    if (revealed && scroller && scroller.scrollTop < OLDER_TRIGGER_PX && hasOlder && !loadingOlder && !olderError) requestOlder();
  }, [revealed, scroller, hasOlder, loadingOlder, olderError, requestOlder, rows.length]);

  useReplyReflowAnchor(scroller, true, following, rememberNothing, adjustments);

  const showError = useCallback((notice: NoticeItem) => {
    setSelectedError(notice);
    setErrorOpen(true);
  }, []);
  const hostActions = useMemo<TranscriptActions>(() => ({ ...actions, onError: showError }), [actions, showError]);

  const items = virtualizer.getVirtualItems();
  const empty = !loading && rows.length === 0;

  return (
    <div className={cn("relative min-h-0 flex-1", className)} data-testid="transcript">
      <div
        ref={setScroller}
        onScroll={onScroll}
        tabIndex={0}
        role="region"
        aria-label="Conversation"
        data-testid="transcript-scroller"
        className={cn(
          "absolute inset-0 overflow-y-auto overscroll-contain [overflow-anchor:none]",
          "transition-[opacity,translate] duration-[280ms] ease-out motion-reduce:transition-none",
          revealed || rows.length === 0 ? "translate-y-0 opacity-100" : "translate-y-[10px] opacity-0",
        )}
      >
        <TranscriptActionsContext value={hostActions}>
          <TranscriptOpened value={opened}>
            <div className="relative mx-auto w-full max-w-[768px]" style={{ height: total }}>
              {items.map((item) => {
                const row = rows[item.index];
                if (!row) return null;
                return (
                  <div
                    key={item.key}
                    ref={virtualizer.measureElement}
                    data-index={item.index}
                    className="absolute top-0 left-0 w-full"
                    style={{ transform: `translateY(${item.start}px)` }}
                  >
                    <div className="pb-1">
                      <TranscriptTurn turn={row.turn} {...(agent ? { agent } : {})} />
                      {row.optimistic ? <SendStatus turn={row.optimistic} onRetry={onRetry} /> : null}
                    </div>
                  </div>
                );
              })}
            </div>
          </TranscriptOpened>
        </TranscriptActionsContext>
      </div>

      {loading && rows.length === 0 ? (
        <div className="pointer-events-none absolute inset-0 grid place-items-center">
          <DelayedSpinner label="Loading the conversation" />
        </div>
      ) : null}
      {empty ? (
        <div className="pointer-events-none absolute inset-0 grid place-items-center px-5 text-center text-small text-muted-text">No messages yet.</div>
      ) : null}

      {loadingOlder || olderError ? (
        <div className="pointer-events-none absolute inset-x-0 top-2 z-20 flex justify-center">
          {loadingOlder ? (
            <span role="status" className="inline-flex items-center gap-2 rounded-pill bg-surface px-3 py-1 text-caption text-muted-text">
              <Icon name="spinner" size={14} className="animate-spin motion-reduce:animate-none" />
              Loading earlier messages
            </span>
          ) : (
            <span role="alert" className="pointer-events-auto inline-flex items-center gap-2 rounded-pill bg-surface px-3 py-1 text-caption text-muted-text">
              Could not load earlier messages.
              <button type="button" onClick={() => requestOlder(true)} className="font-semibold text-content underline">
                Retry
              </button>
            </span>
          )}
        </div>
      ) : null}

      <TranscriptJumpToBottom
        visibility={visibility}
        onJump={() => {
          following.current = true;
          scrollToEnd(true);
        }}
      />
      <p role="status" className="sr-only">
        {busy ? "Working" : ""}
      </p>
      {selectedError ? <SessionErrorDialog message={selectedError.text} open={errorOpen} onOpenChange={setErrorOpen} /> : null}
    </div>
  );
}

/** Under an optimistic bubble: it is on its way, or it was not sent and the user can send it again (F-09). */
function SendStatus({ turn, onRetry }: { turn: OptimisticTurn; onRetry: ((id: string) => void) | undefined }) {
  if (turn.status === "sent") return null;
  if (turn.status === "sending") {
    return (
      <p role="status" className="pr-5 pb-3 text-right text-caption text-[color:var(--content-a40)]">
        Sending...
      </p>
    );
  }
  return (
    <p className="pr-5 pb-3 text-right text-caption text-deletions" data-testid="not-sent">
      <span className="mr-2" data-testid="not-sent-reason">
        {sendErrorMessage(turn.error)}
      </span>
      <button
        type="button"
        disabled={!onRetry}
        title={turn.error}
        onClick={() => onRetry?.(turn.id)}
        className="-my-2 inline-flex min-h-11 items-center rounded-row px-2 font-semibold underline hover:bg-[var(--content-a08)] disabled:no-underline pointer-fine:my-0 pointer-fine:min-h-7"
      >
        Not sent, retry
      </button>
    </p>
  );
}
