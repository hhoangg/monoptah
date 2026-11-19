// F-29: the finished sessions deck (ported from mobile/src/features/sessions/FinishedSessions.tsx). Sessions
// that finished since they were seen at work stack as cards over the now-working pill, newest on top with
// the next two peeking out behind it. With one, a click opens it; with more, a click spreads them out and a
// click on one then opens it. Each card has an X to put it away (mobile uses a long press), and the spread
// deck has Clear and Stack buttons. Opening a session takes it off the deck. The motion is a CSS transition
// that reduced motion turns off.
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { cn } from "@/lib/utils";
import { Glass } from "@/ui/Glass";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon } from "@/ui/Icon";
import { dismissAllFinished, dismissFinished, type FinishedSession } from "./activityStore";
import { relativeTime } from "./relativeTime";

const CARD = 48;
/** How far each card behind the top one peeks out above it. */
export const PEEK = 7;
/** Cards that peek out from behind the top one; the rest hide behind them. */
export const PEEKING = 2;
const GAP = 8;
/** The row over the spread-out cards, with the buttons to clear them. */
const HEADER = 30;

export const collapsedHeight = (count: number): number => (count === 0 ? 0 : CARD + PEEK * Math.min(count - 1, PEEKING));
export const spreadHeight = (count: number): number => HEADER + count * (CARD + GAP) - GAP;

export interface FinishedDeckProps {
  desktopId: string;
  entries: FinishedSession[];
  now?: number;
}

export function FinishedDeck({ desktopId, entries, now }: FinishedDeckProps) {
  const [openState, setOpen] = useState(false);
  const count = entries.length;
  if (count === 0) return null;
  // One card cannot spread.
  const open = openState && count > 1;
  return (
    <section
      aria-label="Finished sessions"
      data-testid="finished-deck"
      data-open={open}
      className="pointer-events-none relative mx-7 w-[calc(100%-3.5rem)] max-w-[504px] motion-safe:transition-[height] motion-safe:duration-300"
      style={{ height: open ? spreadHeight(count) : collapsedHeight(count) }}
    >
      {open ? (
        <div className="pointer-events-auto absolute inset-x-1 top-0 flex items-center gap-1" style={{ height: HEADER - GAP }}>
          <h2 className="flex-1 text-meta font-semibold text-muted-text">Finished</h2>
          <button
            type="button"
            aria-label="Clear finished sessions"
            onClick={() => {
              const projectId = entries[0]?.session.projectId;
              if (projectId) dismissAllFinished(projectId);
            }}
            className="inline-flex h-6 min-w-6 items-center justify-center rounded-pill bg-fill px-2 text-small font-semibold text-muted-text hover:bg-selection"
          >
            Clear
          </button>
          <button
            type="button"
            aria-label="Stack finished sessions"
            onClick={() => setOpen(false)}
            className="inline-flex h-6 min-w-6 items-center justify-center rounded-pill bg-fill text-muted-text hover:bg-selection"
          >
            <Icon name="chevronDown" size={14} />
          </button>
        </div>
      ) : null}
      {entries.map((entry, index) => (
        <DeckCard
          key={entry.session.id}
          desktopId={desktopId}
          entry={entry}
          index={index}
          count={count}
          open={open}
          now={now}
          onSpread={() => setOpen(true)}
          onOpened={() => setOpen(false)}
        />
      ))}
    </section>
  );
}

function DeckCard({
  desktopId,
  entry,
  index,
  count,
  open,
  now,
  onSpread,
  onOpened,
}: {
  desktopId: string;
  entry: FinishedSession;
  index: number;
  count: number;
  open: boolean;
  now?: number;
  onSpread: () => void;
  onOpened: () => void;
}) {
  const { session } = entry;
  const behind = Math.min(index, PEEKING);
  const front = open || index === 0;
  const translate = open ? -index * (CARD + GAP) : -PEEK * behind;
  const scale = open ? 1 : 1 - 0.05 * behind;
  const stacked = !open && count > 1;
  const face = (
    <>
      <span aria-hidden="true" className="relative flex size-8 shrink-0 items-center justify-center rounded-pill bg-fill">
        <HarnessIcon harness={session.harness} className="size-4" />
        <span className="absolute -right-0.5 -bottom-0.5 flex size-3.5 items-center justify-center rounded-pill bg-additions text-inverse-surface">
          <Icon name="check" size={9} strokeWidth={3} />
        </span>
      </span>
      <span className="min-w-0 flex-1 truncate text-label font-semibold">{session.title}</span>
      {stacked ? (
        <span className="mr-1 inline-flex h-[22px] items-center rounded-pill bg-fill px-2 text-small font-semibold tabular text-muted-text">+{count - 1}</span>
      ) : (
        <span className="mr-1 text-small tabular text-muted-text">{relativeTime(entry.at, now)}</span>
      )}
    </>
  );
  const faceClass = cn("flex h-full min-w-0 flex-1 items-center gap-2.5 px-2 text-left text-content", !front && "opacity-0");
  return (
    <div
      data-testid="finished-card"
      data-session-id={session.id}
      // Only the top card takes clicks while stacked.
      inert={!front}
      className={cn("absolute inset-x-0 bottom-0 origin-bottom motion-safe:transition-transform motion-safe:duration-300", front ? "pointer-events-auto" : "pointer-events-none")}
      style={{ zIndex: count - index, transform: `translateY(${translate}px) scale(${scale})` }}
    >
      <Glass className="flex items-center rounded-pill pr-1" style={{ height: CARD }}>
        {!front ? (
          // A card behind the top one is only its edge: nothing to click or read.
          <div aria-hidden="true" className={faceClass}>
            {face}
          </div>
        ) : stacked ? (
          <button type="button" aria-label={`${count} finished sessions. Show them`} onClick={onSpread} className={faceClass}>
            {face}
          </button>
        ) : (
          <Link
            to="/d/$desktopId/s/$sessionId"
            params={{ desktopId, sessionId: session.id }}
            aria-label={`${session.title}, finished. Open`}
            onClick={() => {
              dismissFinished(session.id);
              onOpened();
            }}
            className={faceClass}
          >
            {face}
          </Link>
        )}
        {stacked || !front ? null : (
          <button
            type="button"
            aria-label={`Put away ${session.title}`}
            onClick={() => dismissFinished(session.id)}
            className="inline-flex size-8 shrink-0 items-center justify-center rounded-pill text-muted-text hover:bg-selection"
          >
            <Icon name="close" size={14} />
          </button>
        )}
      </Glass>
    </div>
  );
}
