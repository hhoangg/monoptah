// Port of mobile/src/features/sessions/TranscriptTurn.tsx to the DOM (F-34 to F-38, F-41). One turn,
// laid out as AgentTranscript does: the prompt in a bubble on the right, the work folded behind one
// status line, the answer as full-width Markdown, and when it landed underneath. A Mono's turn signs
// with its name in a pill and keeps its work in that line.
import { memo, use, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import type { Turn } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon, type IconName } from "@/ui/Icon";
import { Markdown } from "@/features/markdown/Markdown";
import { Collapsible } from "./Collapsible";
import { TranscriptOpened, useTranscriptActions, type TranscriptAgent } from "./context";
import { useModelName } from "./modelNames";
import { dayStamp, formatClockTime, formatWorkingDuration, hasTurnMetrics } from "./format";
import { DocumentCard, LaunchedSessions, MonoChoices, MonoHeader } from "./MonoCards";
import { sessionErrorContent } from "./sessionError";
import { Shimmer } from "./Shimmer";
import { CopyFallbackDialog, TurnMetricsDialog } from "./TurnDialogs";
import { useCollapsedToolCalls } from "./toolCallsSetting";
import { readItems, readMetrics, type HeaderItem, type NoticeItem, type UserItem, type WorkKind, type WorkPhase } from "./turnItems";
import { safeClipboardText } from "@/lib/clipboardText";

const WORK_TEXT = "text-body";

export interface TranscriptTurnProps {
  turn: Turn;
  agent?: TranscriptAgent;
}

export const TranscriptTurn = memo(function TranscriptTurn({ turn, agent }: TranscriptTurnProps) {
  const items = readItems(turn);
  const { onError } = useTranscriptActions();
  return (
    <div data-testid="transcript-turn" data-turn-id={turn.id}>
      {turn.stampAt != null ? <DaySeparator at={turn.stampAt} /> : null}
      {items.map((item, index) => {
        const previous = items[index - 1];
        switch (item.type) {
          case "user":
            return <UserMessage key={item.id} item={item} mono={!!agent} />;
          case "header":
            return agent ? <MonoHeader key={item.id} item={item} agent={agent} /> : <FoldLine key={item.id} item={item} />;
          case "work":
            return (
              <div key={item.id} className="px-5">
                <WorkPhases phases={item.phases} live={item.live} />
              </div>
            );
          case "text":
            return (
              <div key={item.id} className={cn("px-5 pb-1", previous && previous.type !== "user" && previous.type !== "text" ? "pt-1" : "pt-3")}>
                <Reply text={item.text} streaming={!!item.streaming} />
              </div>
            );
          case "sessions":
            return <LaunchedSessions key={item.id} sessions={item.sessions} />;
          case "choices":
            return <MonoChoices key={item.id} options={item.options} chosen={item.chosen} />;
          case "document":
            return <DocumentCard key={item.id} document={item} />;
          case "notice":
            return item.error ? (
              <ErrorNotice key={item.id} item={item} onPress={() => onError?.(item)} />
            ) : (
              <p key={item.id} className="px-5 py-2 text-small text-[color:var(--content-a50)]">
                {item.text}
              </p>
            );
        }
      })}
      {turn.footer ? <TurnFooter turn={turn} /> : null}
    </div>
  );
});

function ErrorNotice({ item, onPress }: { item: NoticeItem; onPress: () => void }) {
  const { title, recovery } = sessionErrorContent(item.text);
  return (
    <button
      type="button"
      onClick={onPress}
      aria-label={`${title}. View details`}
      className="mx-5 my-2 flex min-h-16 w-[calc(100%-2.5rem)] items-center gap-3 rounded-card border border-stroke bg-content-a04 p-3.5 text-left hover:bg-[var(--content-a07)]"
    >
      <Icon name={recovery ? "lock" : "chat"} size={18} className="shrink-0 text-muted-text" />
      <span className="min-w-0 flex-1">
        <span className="block text-small font-medium text-muted-text">{title}</span>
        <span className="block text-caption text-muted-text">View details</span>
      </span>
      <Icon name="chevronRight" size={16} className="shrink-0 text-faint" />
    </button>
  );
}

/** What a finished turn leaves under the answer: copy the reply, its token metrics, and when it landed. */
function TurnFooter({ turn }: { turn: Turn }) {
  const [showMetrics, setShowMetrics] = useState(false);
  const metrics = readMetrics(turn);
  const shown = hasTurnMetrics(metrics) ? metrics : undefined;
  return (
    <div className="flex items-center gap-2.5 pt-1 pr-5 pb-3 pl-4">
      <div className="flex items-center gap-1">
        {turn.copyText ? <CopyButton text={turn.copyText} /> : null}
        {shown ? (
          <button
            type="button"
            aria-label="Turn metrics"
            onClick={() => setShowMetrics(true)}
            className={cn("inline-flex min-h-11 min-w-11 items-center justify-center rounded-sm text-[color:var(--content-a40)] hover:bg-[var(--content-a08)] pointer-fine:min-h-7 pointer-fine:min-w-7", showMetrics && "bg-[var(--content-a08)]")}
          >
            <Icon name="metrics" size={16} />
          </button>
        ) : null}
      </div>
      {turn.completedAt != null ? (
        <>
          {turn.copyText || shown ? <span aria-hidden="true" className="size-[3px] rounded-full bg-[var(--content-a25)]" /> : null}
          <span className="text-small text-[color:var(--content-a35)]">{formatClockTime(turn.completedAt)}</span>
        </>
      ) : null}
      {shown ? <TurnMetricsDialog metrics={shown} elapsedMs={turn.durationMs ?? null} completedAt={turn.completedAt} open={showMetrics} onOpenChange={setShowMetrics} /> : null}
    </div>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const [fallback, setFallback] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = () => {
    // Called inside the click handler: the clipboard needs the user gesture. A refusal opens the text to select.
    const write = globalThis.navigator?.clipboard?.writeText(safeClipboardText(text));
    if (!write) {
      setFallback(true);
      return;
    }
    write.then(
      () => {
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 2000);
      },
      () => setFallback(true),
    );
  };
  return (
    <>
      <button
        type="button"
        onClick={copy}
        aria-label={copied ? "Copied" : "Copy response"}
        className="inline-flex min-h-11 min-w-11 items-center justify-center rounded-sm text-[color:var(--content-a40)] hover:bg-[var(--content-a08)] pointer-fine:min-h-7 pointer-fine:min-w-7"
      >
        <Icon name={copied ? "check" : "copy"} size={16} />
      </button>
      {fallback ? <CopyFallbackDialog text={text} open onOpenChange={setFallback} /> : null}
    </>
  );
}

function Reply({ text, streaming }: { text: string; streaming: boolean }) {
  const opened = use(TranscriptOpened);
  const [animate] = useState(opened);
  const { onOpenFile, onNavigate } = useTranscriptActions();
  return (
    <div data-chat-message-role="assistant">
      <Markdown text={text} streaming={streaming} animate={animate} onOpenFile={onOpenFile} onNavigate={onNavigate} />
    </div>
  );
}

function DaySeparator({ at }: { at: number }) {
  const [day, time] = dayStamp(at);
  return (
    <div className="flex justify-center pt-6 pb-3" role="separator" aria-label={`${day} ${time}`}>
      <span className="rounded-pill bg-[var(--content-a07)] px-3 py-1 text-caption text-[color:var(--content-a45)]">
        <span className="font-semibold text-[color:var(--content-a75)]">{day}</span> {time}
      </span>
    </div>
  );
}

function useElementSize() {
  const ref = useRef<HTMLParagraphElement | null>(null);
  const [size, setSize] = useState({ lines: 0, clamped: false, everClamped: false });
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => {
      const line = parseFloat(getComputedStyle(element).lineHeight) || 23;
      const lines = Math.round(element.scrollHeight / line);
      const clamped = element.scrollHeight - element.clientHeight > 1;
      setSize((previous) => {
        const everClamped = previous.everClamped || clamped;
        return previous.lines === lines && previous.clamped === clamped && previous.everClamped === everClamped ? previous : { lines, clamped, everClamped };
      });
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, size] as const;
}

function UserMessage({ item, mono }: { item: UserItem; mono: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const [ref, size] = useElementSize();
  // The clamp is only measurable while it is on, so remember that it cut the message before it was opened.
  const overflows = size.clamped || (expanded && size.everClamped);
  const singleLine = size.lines === 1 && !item.attachments;

  // In a Mono's chat a few emoji stand alone, like a reaction: no bubble.
  if (mono && item.emojiOnly) {
    return (
      <div className={cn("flex flex-col items-end pr-5 pl-14", item.compact ? "pt-1" : "pt-1.5 pb-4")}>
        <div className="relative">
          <span className="text-[56px] leading-[64px]">{item.text.trim()}</span>
          {item.reaction ? <Reaction emoji={item.reaction} /> : null}
        </div>
      </div>
    );
  }

  return (
    <div className={cn("flex flex-col items-end pr-5 pl-14", item.compact ? "pt-1" : "pt-1.5 pb-4")} data-chat-message-role="user">
      <div className={cn("relative max-w-full bg-bubble px-3.5 py-[9px] text-content", singleLine ? "rounded-[21px]" : "rounded-[14px]")}>
        {item.attachments ? <p className="mb-1 text-meta text-muted-text">{item.attachments === 1 ? "1 attachment" : `${item.attachments} attachments`}</p> : null}
        {item.text ? (
          <p ref={ref} className={cn("text-[16px] leading-[23px] whitespace-pre-wrap select-text wrap-anywhere", !expanded && "line-clamp-4")}>
            {item.text}
          </p>
        ) : null}
        {overflows ? (
          <button type="button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)} className="-mb-2 mt-0 inline-flex min-h-11 items-center text-meta text-[color:var(--content-a60)] pointer-fine:mb-0 pointer-fine:mt-1 pointer-fine:min-h-6">
            {expanded ? "Show less" : "Show more"}
          </button>
        ) : null}
        {item.reaction ? <Reaction emoji={item.reaction} /> : null}
      </div>
      {!item.compact && item.at != null ? <span className="mt-1 px-3 text-caption text-[color:var(--content-a40)]">{formatClockTime(item.at)}</span> : null}
    </div>
  );
}

/** A Mono's emoji answer, on the corner of the message it answers. */
function Reaction({ emoji }: { emoji: string }) {
  return (
    <span role="img" aria-label={`Reacted ${emoji}`} className="absolute -top-2.5 -left-2.5 flex h-[22px] items-center rounded-[8px] border border-stroke bg-surface-raised px-[7px] text-micro">
      {emoji}
    </span>
  );
}

/** Milliseconds since `startedAt`, ticking each second; null without a start. */
function useElapsed(startedAt: number | undefined): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (startedAt == null) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [startedAt]);
  return startedAt == null ? null : Math.max(0, now - startedAt);
}

const RAIL: CSSProperties = { left: 28 };

/**
 * The line a turn's work folds behind: the provider's mark and the model's clock, ticking while it works and
 * how long it took once done. Opened, the work hangs off it on a rail: the agent's notes, muted, between the
 * phases of its work, as on desktop.
 */
function FoldLine({ item }: { item: HeaderItem }) {
  const [open, setOpen] = useState(false);
  const { onOpenFile, onNavigate } = useTranscriptActions();
  const elapsed = useElapsed(item.live ? item.startedAt : undefined);
  const modelName = useModelName(item.model);
  const label =
    item.label ?? (item.live ? (item.waiting ?? formatWorkingDuration(elapsed, modelName)) : formatWorkingDuration(item.durationMs ?? null, modelName, true));
  const expandable = item.entries.length > 0;
  const shown = open && expandable;
  const markClass = "size-4 shrink-0 text-[color:var(--content-a45)]";
  const mark = shown ? <Icon name="chevronDown" size={16} className={markClass} /> : item.harness ? <HarnessIcon harness={item.harness} className={markClass} /> : null;

  return (
    <div>
      <button
        type="button"
        disabled={!expandable}
        onClick={() => setOpen((value) => !value)}
        aria-expanded={expandable ? shown : undefined}
        aria-label={expandable ? (shown ? "Hide the work" : "Show the work") : label}
        className="relative flex w-full items-center gap-1.5 px-5 py-1 text-left disabled:cursor-default"
      >
        {mark}
        {item.live ? (
          <Shimmer duration={1} className={cn("min-w-0 truncate text-[color:var(--content-a50)]", WORK_TEXT)}>
            {label}
          </Shimmer>
        ) : (
          <span className={cn("min-w-0 truncate text-[color:var(--content-a50)]", WORK_TEXT)}>{label}</span>
        )}
        {shown ? <span aria-hidden="true" className="absolute bottom-0 w-px bg-rail" style={{ ...RAIL, top: 4 + (24 + 16) / 2 }} /> : null}
      </button>
      <Collapsible open={shown}>
        <div className="pb-1">
          {item.entries.map((entry, index) => (
            <div key={entry.id} className="relative pr-5 pb-1 pl-[42px]">
              <span aria-hidden="true" className={cn("absolute top-0 w-px bg-rail", index === item.entries.length - 1 ? "h-[9px]" : "bottom-0")} style={RAIL} />
              <span aria-hidden="true" className="absolute top-2 size-2 rounded-bl-[8px] border-b border-l border-rail" style={RAIL} />
              {entry.type === "prose" ? (
                <div className="py-1">
                  <Markdown text={entry.text} muted onOpenFile={onOpenFile} onNavigate={onNavigate} />
                </div>
              ) : (
                <Phase phase={entry} active={false} />
              )}
            </div>
          ))}
        </div>
      </Collapsible>
    </div>
  );
}

/**
 * The turn's work as phases, each the agent's own line or what its calls add up to, under the icon for what
 * it did most. Live, the phase in progress stays open on its latest steps; pressing any phase lists them.
 */
function WorkPhases({ phases, live }: { phases: WorkPhase[]; live: boolean }) {
  return (
    <div className="flex flex-col gap-0.5 py-0.5">
      {phases.map((phase, index) => (
        <Phase key={phase.id} phase={phase} active={live && index === phases.length - 1} />
      ))}
    </div>
  );
}

const KIND_ICONS: Partial<Record<WorkKind, IconName>> = {
  edit: "edit",
  research: "search",
  run: "terminal",
  agent: "robot",
  other: "wrench",
  note: "minus",
};

function Phase({ phase, active }: { phase: WorkPhase; active: boolean }) {
  const [collapsed] = useCollapsedToolCalls();
  const [override, setOverride] = useState<boolean | null>(null);
  const expandable = phase.steps.length > 0;
  const open = expandable && (override ?? (active && !collapsed));
  const icon = KIND_ICONS[phase.kind];
  const steps = active && override === null ? phase.steps.slice(-4) : phase.steps;
  const titleClass = cn("min-w-0 truncate text-[color:var(--content-a50)]", WORK_TEXT);
  return (
    <div data-testid="work-phase" data-kind={phase.kind}>
      <button
        type="button"
        disabled={!expandable}
        aria-expanded={expandable ? open : undefined}
        onClick={() => setOverride(!open)}
        className="flex w-full items-center gap-1.5 py-1 text-left disabled:cursor-default"
      >
        {icon ? <Icon name={icon} size={16} className="shrink-0 text-[color:var(--content-a45)]" /> : null}
        {active ? (
          <Shimmer className={titleClass}>{phase.title}</Shimmer>
        ) : (
          <span className={titleClass}>{phase.title}</span>
        )}
      </button>
      <Collapsible open={open}>
        <div className="mb-1">
          {steps.map((step, index) => (
            // Each step branches off the phase's rail, as on desktop.
            <div key={step.id} className="relative py-1 pl-[22px]">
              <span aria-hidden="true" className={cn("absolute top-0 left-[7px] w-px bg-rail", index === steps.length - 1 ? "h-[9px]" : "bottom-0")} />
              <span aria-hidden="true" className="absolute top-2 left-[7px] size-2 rounded-bl-[8px] border-b border-l border-rail" />
              <p className={cn("line-clamp-2", WORK_TEXT, step.failed ? "text-deletions" : "text-[color:var(--content-a45)]")}>{step.label}</p>
            </div>
          ))}
        </div>
      </Collapsible>
    </div>
  );
}

