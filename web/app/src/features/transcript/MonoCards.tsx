// F-43 and F-72, exported for the Mono chat of WS-P as well as used by the session transcript: a Mono's
// signature header ("Thinking..." while it works), the cards of the sessions it launched, its suggested
// reply chips and the documents it wrote. Ported from mobile MonoCards.tsx, LaunchedSessionCard.tsx and
// the MonoHeader of TranscriptTurn.tsx.
import { SendCancelled } from "@/features/composer/useSendGuard";
import { useState } from "react";
import { cn } from "@/lib/utils";
import { safeHexColor } from "@/lib/color";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon, type IconName } from "@/ui/Icon";
import { useTranscriptActions, type TranscriptAgent } from "./context";
import { Shimmer } from "./Shimmer";
import type { DocumentItem, HeaderItem, LaunchedSession, LaunchedSessionState } from "./turnItems";

/** The Mono's pill: a colour tile with its initial (the web has no mascots), then its name. */
function Signature({ agent, name }: { agent: TranscriptAgent; name: string }) {
  const color = safeHexColor(agent.color, "var(--accent-color)");
  return (
    <span className="inline-flex max-w-full items-center gap-1.5 self-start rounded-pill bg-[var(--content-a07)] py-0.5 pr-3 pl-2" data-testid="mono-signature">
      <span aria-hidden="true" className="grid size-3.5 shrink-0 place-items-center rounded-[4px] text-[9px] leading-none font-bold text-white" style={{ background: color }}>
        {agent.name.trim().charAt(0).toUpperCase()}
      </span>
      <span className="min-w-0 truncate text-small font-medium text-[var(--content-a85)]">{name}</span>
    </span>
  );
}

/** A Mono signs its turns; live, its status ("Thinking...") runs beside the signature. */
export function MonoHeader({ item, agent }: { item: Pick<HeaderItem, "live" | "label">; agent: TranscriptAgent }) {
  if (!item.live) {
    return (
      <div className="flex items-center gap-1.5 px-5 py-1">
        <Signature agent={agent} name={item.label ?? agent.name} />
      </div>
    );
  }
  return (
    <div className="flex items-center gap-1.5 px-5 py-1" data-testid="mono-header-live">
      <span className="max-w-[45%] shrink-0">
        <Signature agent={agent} name={agent.name} />
      </span>
      <span aria-hidden="true" className="text-small text-[color:var(--content-a25)]">
        ·
      </span>
      <Shimmer className="min-w-0 flex-1 truncate text-small text-[color:var(--content-a50)]">{item.label ?? "Thinking..."}</Shimmer>
    </div>
  );
}

/**
 * A Mono's suggested replies: a card each with a ring. A tap sends it; once a reply follows, the one sent
 * is checked and the rest step back (F-72).
 */
export function MonoChoices({ options, chosen }: { options: string[]; chosen?: string }) {
  const { onChoose } = useTranscriptActions();
  // Answered at the tap, so a second tap cannot send another before the reply comes back.
  const [sent, setSent] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const reply = chosen ?? sent;
  const answered = reply != null;
  return (
    <div className="flex flex-col items-start gap-1.5 px-5 pt-1 pb-2" role="group" aria-label="Suggested replies">
      {options.map((option, index) => {
        const checked = answered && option.trim() === reply.trim();
        const disabled = answered || !onChoose;
        return (
          <button
            key={index}
            type="button"
            aria-label={option}
            aria-pressed={checked}
            disabled={disabled}
            onClick={() => {
              if (!onChoose) return;
              setSent(option);
              setFailed(null);
              onChoose(option).catch((reason: unknown) => {
                setSent(null);
                // The user said no to the send-guard: nothing went wrong, so nothing to show.
                if (reason instanceof SendCancelled) return;
                setFailed(reason instanceof Error ? reason.message : String(reason));
              });
            }}
            className={cn(
              "flex max-w-[80%] items-center gap-2.5 rounded-row border py-1.5 pr-3 pl-2.5 text-left text-small text-content",
              checked ? "border-[color:var(--content-a25)]" : "border-[color:var(--content-a07)]",
              answered && !checked && "opacity-45",
              !disabled && "hover:bg-content-a04 active:bg-content-a04",
            )}
          >
            {checked ? (
              <span aria-hidden="true" className="grid size-4 shrink-0 place-items-center rounded-full bg-additions text-[#062e22]">
                <Icon name="check" size={11} strokeWidth={2.5} />
              </span>
            ) : (
              <span aria-hidden="true" className="size-4 shrink-0 rounded-full border-[1.5px] border-[color:var(--content-a35)]" />
            )}
            <span className="min-w-0 wrap-anywhere">{option}</span>
          </button>
        );
      })}
      {failed ? (
        <p role="alert" className="text-caption text-deletions">
          Could not send: {failed}
        </p>
      ) : null}
    </div>
  );
}

/** A document a Mono wrote: its mark and title; a press opens the reader. */
export function DocumentCard({ document }: { document: DocumentItem }) {
  const { onOpenDocument } = useTranscriptActions();
  const visual = document.kind === "html" || document.kind === "visual";
  return (
    <div className="flex flex-col items-start gap-1.5 px-5 pt-1 pb-2">
      <button
        type="button"
        disabled={!onOpenDocument}
        aria-label={`Open ${visual ? "visual" : "document"} ${document.title}`}
        onClick={() => onOpenDocument?.(document)}
        className="flex min-h-[38px] max-w-full items-center gap-2.5 rounded-row border border-stroke bg-content-a04 py-2 pr-3.5 pl-3 text-left text-small text-content hover:bg-[var(--content-a08)]"
      >
        <Icon name={visual ? "metrics" : "note"} size={16} className="shrink-0 text-[color:var(--content-a55)]" />
        <span className="min-w-0 truncate">{document.title}</span>
      </button>
    </div>
  );
}

interface Status {
  label: string;
  icon: IconName;
  className: string;
}

const STATUS: Record<LaunchedSessionState, Status> = {
  working: { label: "Working", icon: "terminal", className: "text-[color:var(--content-a55)]" },
  input: { label: "Needs input", icon: "chat", className: "text-caution" },
  draft: { label: "Draft", icon: "edit", className: "text-[color:var(--content-a45)]" },
  failed: { label: "Failed", icon: "minus", className: "text-deletions" },
  stopped: { label: "Stopped", icon: "minus", className: "text-[color:var(--content-a45)]" },
  done: { label: "Done", icon: "check", className: "text-additions" },
};

/** The sessions a Mono started, as cards under its reply. */
export function LaunchedSessions({ sessions }: { sessions: LaunchedSession[] }) {
  return (
    <div className="flex flex-col items-start gap-2 px-5 pt-1 pb-3">
      {sessions.map((session) => (
        <LaunchedSessionCard key={session.id} session={session} />
      ))}
    </div>
  );
}

/**
 * A session the Mono started: a mark for how it stands, its title (animated while it works) and the project
 * it runs in. A press opens it, unless the session lives in a project outside this device's selection: then
 * the card says so and has no open action (F-72).
 */
export function LaunchedSessionCard({ session }: { session: LaunchedSession }) {
  const { onOpenSession, isProjectAvailable } = useTranscriptActions();
  const status = STATUS[session.state] ?? STATUS.done;
  // The desktop can refuse a session of a project outside this device's selection (`not_selected`) even when the
  // catalog did not say so: the card then reads the same as one whose project is not available.
  const [refused, setRefused] = useState(false);
  const available = !refused && (isProjectAvailable ? isProjectAvailable(session.projectId) : true);
  const open = async () => {
    try {
      await onOpenSession?.(session.projectId, session.id);
    } catch (error) {
      if (error && typeof error === "object" && "code" in error && (error as { code: unknown }).code === "not_selected") setRefused(true);
    }
  };
  const body = (
    <>
      <Icon name={status.icon} size={16} className={cn("shrink-0", status.className)} />
      {session.state === "working" ? (
        <Shimmer className="min-w-0 flex-1 truncate text-small text-content">{session.title}</Shimmer>
      ) : (
        <span className="min-w-0 flex-1 truncate text-small text-content">{session.title}</span>
      )}
      <span className="flex max-w-[45%] shrink-0 items-center gap-1 rounded-pill border border-stroke bg-[var(--content-a05)] px-2 py-0.5 text-caption text-[color:var(--content-a65)]">
        <HarnessIcon harness={session.harness} className="size-3.5" />
        <span className="min-w-0 truncate">{session.project}</span>
      </span>
    </>
  );
  const shell = "flex min-h-10 max-w-full items-center gap-2.5 rounded-row border border-stroke bg-content-a04 py-1.5 pr-1.5 pl-3";
  if (!available) {
    return (
      <div className={cn(shell, "opacity-80")} data-testid="launched-session-unavailable">
        {body}
        <span className="shrink-0 pr-1.5 text-caption text-muted-text">Not on this device's projects</span>
      </div>
    );
  }
  return (
    <button
      type="button"
      disabled={!onOpenSession}
      aria-label={`Open session ${session.title}, ${status.label.toLowerCase()}`}
      onClick={() => void open()}
      className={cn(shell, "text-left hover:bg-[var(--content-a07)]")}
    >
      {body}
    </button>
  );
}
