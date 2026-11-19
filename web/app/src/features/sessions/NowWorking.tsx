// F-28: the "Now working" pill, ported from mobile/src/features/sessions/LiveSessions.tsx. One glass pill
// rests (sticky) at the bottom of the session list column with the sessions at work: the one in front's agent mark,
// its title and status ("Needs your approval", "Has a question for you"), a Stop button and, with more
// than one, "2 of 3" with a Skip button. A tap opens the session when there is one, or fans the sessions
// out above the pill as a row each. Stop is `session.cancel` and is disabled for read-only devices.
import { Link } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { SessionCard } from "@monoptah/protocol";
import { catalogKey, useLinkMutation } from "@/data";
import { cn } from "@/lib/utils";
import { Glass } from "@/ui/Glass";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon } from "@/ui/Icon";
import { TerminalSpinner } from "@/features/transcript/TerminalSpinner";
import { markStoppedHere, unmarkStoppedHere, useSessionActivity } from "./activityStore";
import { FinishedDeck } from "./FinishedDeck";
import { statusLabel } from "./SessionCard";
import { Ticker } from "./Ticker";

export const READ_ONLY_STOP_REASON = "This device is read-only";

function detailOf(session: SessionCard, arrived: boolean): string {
  if (session.waiting) return statusLabel(session);
  const model = session.modelName || session.model;
  return [arrived ? "From desktop" : null, model, session.branch].filter(Boolean).join(" · ") || "Working";
}

function useStop(desktopId: string) {
  const cancel = useLinkMutation("session.cancel");
  const queryClient = useQueryClient();
  return {
    pending: cancel.isPending,
    error: cancel.isError,
    stop: (session: SessionCard) => {
      markStoppedHere(session.id);
      cancel.mutate(
        { conversationId: session.id },
        {
          onSuccess: () => void queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) }),
          // A failed stop leaves the session running: it must still reach the finished shelf when it ends.
          onError: () => unmarkStoppedHere(session.id),
        },
      );
    },
  };
}

function StopButton({ onClick, disabled, label }: { onClick: () => void; disabled: boolean; label: string }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={disabled ? READ_ONLY_STOP_REASON : undefined}
      disabled={disabled}
      onClick={onClick}
      className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection disabled:pointer-events-none disabled:opacity-40 pointer-fine:size-9"
    >
      <span aria-hidden="true" className="size-3 rounded-[3px] bg-current" />
    </button>
  );
}

function Artwork({ session, compact = false }: { session: SessionCard; compact?: boolean }) {
  return (
    <span aria-hidden="true" className={cn("relative flex size-10 shrink-0 items-center justify-center rounded-pill bg-fill", compact && "@max-[20rem]:hidden")}>
      <HarnessIcon harness={session.harness} className="size-5" />
      {session.waiting ? null : <span className="absolute inset-0 rounded-pill border border-brand opacity-50 motion-safe:animate-pulse" />}
    </span>
  );
}

export function NowWorking({ desktopId, projectId, readOnly }: { desktopId: string; projectId: string; readOnly: boolean }) {
  const { working: live, finished, arrivals } = useSessionActivity(projectId);
  const [active, setActive] = useState(0);
  const [open, setOpen] = useState(false);
  const { stop, error } = useStop(desktopId);

  // A session that starts working once the pill is up is brought to the front; the first ones are no news.
  const ids = live.map((session) => session.id).join(",");
  const [knownIds, setKnownIds] = useState<string | null>(null);
  if (ids !== knownIds) {
    setKnownIds(ids);
    if (knownIds !== null) {
      const before = new Set(knownIds.split(",").filter(Boolean));
      const arrived = live.findIndex((session) => !before.has(session.id));
      if (arrived >= 0) setActive(arrived);
    }
  }

  if (live.length === 0 && finished.length === 0) return null;
  if (live.length === 0) {
    return (
      <div data-testid="now-working" className="pointer-events-none sticky mt-auto bottom-[calc(env(safe-area-inset-bottom)+0.75rem)] z-30 flex flex-col items-center gap-2 px-3 pt-3">
        <FinishedDeck desktopId={desktopId} entries={finished} />
      </div>
    );
  }
  const index = Math.min(active, live.length - 1);
  const current = live[index] as SessionCard;
  const many = live.length > 1;
  const expanded = open && many;
  const subtitle = (
    <span className="flex min-w-0 items-center gap-1.5 text-meta text-muted-text">
      {current.waiting ? null : <TerminalSpinner className="text-brand-text" />}
      <span className={cn("truncate", current.waiting && "text-caution")}>{detailOf(current, current.id in arrivals)}</span>
      {many ? (
        <span className="ml-auto shrink-0 tabular text-muted-text" aria-label={`${index + 1} of ${live.length}`}>
          <span className="@max-[20rem]:hidden">
            {index + 1} of {live.length}
          </span>
          <span aria-hidden="true" className="hidden @max-[20rem]:inline">
            {index + 1}/{live.length}
          </span>
        </span>
      ) : null}
    </span>
  );

  return (
    <div
      data-testid="now-working"
      className="@container pointer-events-none sticky mt-auto bottom-[calc(env(safe-area-inset-bottom)+0.75rem)] z-30 flex flex-col items-center gap-2 px-3 pt-3"
    >
      <FinishedDeck desktopId={desktopId} entries={finished} />
      {expanded ? (
        <ul aria-label="Working sessions" className="pointer-events-auto flex w-full max-w-[560px] flex-col gap-2">
          {live.map((session) => (
            <li key={session.id}>
              <Glass className="flex items-center gap-3 rounded-pill py-1.5 pr-1.5 pl-2">
                <Artwork session={session} />
                <Link
                  to="/d/$desktopId/s/$sessionId"
                  params={{ desktopId, sessionId: session.id }}
                  className="flex min-w-0 flex-1 flex-col"
                >
                  <span className="truncate text-label font-semibold">{session.title}</span>
                  <span className={cn("truncate text-meta text-muted-text", session.waiting && "text-caution")}>{detailOf(session, session.id in arrivals)}</span>
                </Link>
                <StopButton label={`Stop ${session.title}`} disabled={readOnly} onClick={() => stop(session)} />
              </Glass>
            </li>
          ))}
        </ul>
      ) : null}
      <Glass className="pointer-events-auto flex w-full max-w-[560px] items-center gap-3 rounded-pill py-2 pr-2 pl-2.5 @max-[20rem]:gap-2 @max-[20rem]:pl-3.5 motion-safe:animate-[fade-in_180ms_var(--ease-out-cubic)]">
        <Artwork session={current} compact />
        {many ? (
          <button
            type="button"
            aria-expanded={expanded}
            aria-label={`${live.length} working sessions. ${expanded ? "Hide" : "Show"} them`}
            onClick={() => setOpen((was) => !was)}
            className="flex min-w-0 flex-1 flex-col text-left"
          >
            <Ticker text={current.title} className="text-label font-semibold" />
            {subtitle}
          </button>
        ) : (
          <Link
            to="/d/$desktopId/s/$sessionId"
            params={{ desktopId, sessionId: current.id }}
            aria-label={`${current.title}, ${statusLabel(current).toLowerCase()}. Open`}
            className="flex min-w-0 flex-1 flex-col"
          >
            <Ticker text={current.title} className="text-label font-semibold" />
            {subtitle}
          </Link>
        )}
        <StopButton label="Stop" disabled={readOnly} onClick={() => stop(current)} />
        {many ? (
          <button
            type="button"
            aria-label="Skip to the next session"
            onClick={() => setActive((index + 1) % live.length)}
            className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection pointer-fine:size-9"
          >
            <Icon name="chevronRight" size={18} />
          </button>
        ) : null}
      </Glass>
      {error ? (
        <p role="alert" className="pointer-events-auto rounded-pill bg-sheet px-3 py-1 text-meta text-deletions">
          Could not stop the session.
        </p>
      ) : null}
    </div>
  );
}
