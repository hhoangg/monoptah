// Copied from src/features/sessions/ui/QuestionForm.tsx at ca73ac2b
// Restyled to the design tokens (Glass card, rounded-row options). The 60 s countdown and the
// keep-open-on-first-touch behaviour follow mobile/src/features/sessions/QuestionCard.tsx.
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import type { QuestionPrompt, QuestionReply } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { Glass } from "@/ui/Glass";
import { Check, MessageSquare } from "@/ui/icons";
import {
  buildQuestionReply,
  customOptionId,
  displayOptions,
  isCustomId,
  isOtherOption,
  nextSelection,
  questionCountdown,
  questionIsComplete,
  selectionAfterCustomText,
  CUSTOM_OPTION_ID,
  type Question,
  type QuestionAnswers,
  type QuestionCustom,
} from "./userQuestion";

export interface QuestionFormProps {
  prompt: QuestionPrompt;
  /** Sends the reply (`session.answerQuestion`). A rejection keeps the form open and shows an error. */
  onReply: (requestId: number, reply: QuestionReply) => void | Promise<void>;
  /** Called once per request on the first touch of a timed question (`session.keepQuestionOpen`). */
  onKeepOpen?: (requestId: number) => void | Promise<void>;
  /** When set the form is read only and this text explains why (a read-only device). */
  disabledReason?: string;
  className?: string;
}

/** Re-mounts per request so a replacement prompt never inherits the previous answers. */
export function QuestionForm(props: QuestionFormProps) {
  return <QuestionFormBody key={props.prompt.requestId} {...props} />;
}

/** The clock, each second while a deadline counts down. */
function useNow(ticking: boolean): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  return now;
}

function QuestionFormBody({ prompt, onReply, onKeepOpen, disabledReason, className }: QuestionFormProps) {
  const [step, setStep] = useState(0);
  const [answers, setAnswers] = useState<QuestionAnswers>({});
  const [custom, setCustom] = useState<QuestionCustom>({});
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const keptOpen = useRef(false);
  const now = useNow(prompt.autoResolveAt != null);
  const reasonId = useId();
  const readOnly = disabledReason != null;

  const questions = prompt.questions;
  const total = questions.length;
  const index = Math.min(step, Math.max(total - 1, 0));
  const question: Question | undefined = questions[index];
  const last = index >= total - 1;
  const ready = useMemo(
    () => (question ? questionIsComplete(question, answers, custom) : false),
    [answers, custom, question],
  );

  const interact = () => {
    if (prompt.autoResolveAt == null || !onKeepOpen || keptOpen.current) return;
    keptOpen.current = true;
    // A later touch can retry after a failure; never duplicate a request still in flight.
    const retry = () => {
      keptOpen.current = false;
    };
    try {
      Promise.resolve(onKeepOpen(prompt.requestId)).catch(retry);
    } catch {
      retry();
    }
  };

  const finish = async (nextAnswers = answers, nextCustom = custom) => {
    setSending(true);
    setError(null);
    try {
      await onReply(prompt.requestId, buildQuestionReply(questions, nextAnswers, nextCustom));
    } catch (reason) {
      setError(reason instanceof Error && reason.message ? reason.message : "Answer not sent");
    } finally {
      setSending(false);
    }
  };

  if (!question) return null;

  const skipCurrent = () => {
    const nextAnswers = { ...answers };
    const nextCustom = { ...custom };
    delete nextAnswers[question.id];
    delete nextCustom[question.id];
    setAnswers(nextAnswers);
    setCustom(nextCustom);
    if (last) void finish(nextAnswers, nextCustom);
    else setStep(index + 1);
  };

  const continueCurrent = () => {
    if (!ready || readOnly || sending) return;
    if (last) void finish();
    else setStep(index + 1);
  };

  const title = question.header?.trim() || prompt.title?.trim() || "Question";
  const countdown = questionCountdown(prompt.autoResolveAt, now);
  const busy = readOnly || sending;

  return (
    <div
      className={cn("px-3 pb-2", className)}
      data-question-form
      onPointerDownCapture={interact}
      onKeyDownCapture={interact}
      onPasteCapture={interact}
    >
      <Glass className="rounded-card">
        <form
          className="flex flex-col gap-2 px-4 py-3"
          aria-describedby={readOnly ? reasonId : undefined}
          onSubmit={(event) => {
            event.preventDefault();
            continueCurrent();
          }}
        >
          <div className="flex items-center gap-1.5">
            <MessageSquare size={14} className="shrink-0 text-muted-text" aria-hidden="true" />
            <span className="min-w-0 flex-1 truncate text-small text-muted-text">{title}</span>
            {total > 1 ? (
              <span className="shrink-0 text-caption text-muted-text">
                {index + 1} of {total}
              </span>
            ) : null}
            <button
              type="button"
              disabled={busy}
              onClick={skipCurrent}
              className="min-h-9 shrink-0 rounded-row px-2 text-small text-muted-text hover:text-content disabled:opacity-40"
            >
              Skip
            </button>
          </div>

          <QuestionFields
            key={question.id}
            question={question}
            selected={answers[question.id] ?? []}
            custom={custom[question.id] ?? ""}
            disabled={busy}
            onSelect={(optionId) =>
              setAnswers((current) => ({
                ...current,
                [question.id]: nextSelection(question, current[question.id] ?? [], optionId),
              }))
            }
            onCustom={(value) => {
              setCustom((current) => ({ ...current, [question.id]: value }));
              setAnswers((current) => ({
                ...current,
                [question.id]: selectionAfterCustomText(question, current[question.id] ?? []),
              }));
            }}
          />

          {readOnly ? (
            <p id={reasonId} className="text-caption text-muted-text">
              {disabledReason}
            </p>
          ) : null}
          {error ? (
            <p role="alert" className="text-caption text-destructive">
              {error}
            </p>
          ) : null}

          <div className="mt-1 flex items-center justify-end gap-3">
            {countdown.kind !== "none" ? (
              <span
                className="mr-auto min-w-0 truncate text-caption text-muted-text"
                title="Touch this question to keep it open."
                data-question-countdown={countdown.kind}
              >
                {countdown.kind === "optional"
                  ? "Optional question"
                  : `Continues without an answer in ${countdown.seconds}s`}
              </span>
            ) : null}
            {index > 0 ? (
              <button
                type="button"
                disabled={busy}
                onClick={() => setStep(index - 1)}
                className="min-h-9 shrink-0 rounded-row px-2 text-small text-muted-text hover:text-content disabled:opacity-40"
              >
                Back
              </button>
            ) : null}
            <Button type="submit" size="pill" disabled={!ready || busy} className="min-h-9">
              {last ? "Send" : "Continue"}
            </Button>
          </div>
        </form>
      </Glass>
    </div>
  );
}

function QuestionFields({
  question,
  selected,
  custom,
  disabled,
  onSelect,
  onCustom,
}: {
  question: Question;
  selected: string[];
  custom: string;
  disabled: boolean;
  onSelect: (optionId: string) => void;
  onCustom: (value: string) => void;
}) {
  const options = displayOptions(question);
  const customSelected = selected.some((id) => isCustomId(question, id));
  const customId = customOptionId(question);
  const [highlighted, setHighlighted] = useState(() => {
    const selectedIndex = options.findIndex((option) => selected.includes(option.id));
    return selectedIndex >= 0 ? selectedIndex : 0;
  });
  const optionButtons = useRef<Array<HTMLButtonElement | null>>([]);
  const labelId = useId();

  const highlight = (index: number) => {
    setHighlighted(index);
    optionButtons.current[index]?.focus();
  };

  const onOptionKeyDown = (event: ReactKeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const offset = event.key === "ArrowDown" ? 1 : -1;
      highlight((index + offset + options.length) % options.length);
      return;
    }
    if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      highlight(event.key === "Home" ? 0 : options.length - 1);
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onSelect(options[index]!.id);
      return;
    }
    const shortcut = Number(event.key);
    if (Number.isInteger(shortcut) && shortcut >= 1 && shortcut <= Math.min(options.length, 9)) {
      event.preventDefault();
      highlight(shortcut - 1);
      onSelect(options[shortcut - 1]!.id);
    }
  };

  const customInput = (
    <input
      value={custom}
      disabled={disabled}
      aria-label="Your answer"
      onChange={(event) => onCustom(event.target.value)}
      placeholder="Type your answer"
      className="mt-1.5 min-h-10 w-full rounded-row border border-stroke bg-transparent px-3 text-body text-content outline-none placeholder:text-faint focus:border-muted-text"
    />
  );

  return (
    <div className="min-w-0">
      <p id={labelId} className="text-body font-medium text-content">
        {question.prompt}
      </p>
      {question.multiSelect ? <p className="text-caption text-muted-text">Select all that apply</p> : null}
      {options.length === 0 && question.allowCustom ? (
        customInput
      ) : (
        <div
          className="mt-1.5 flex max-h-72 flex-col gap-1.5 overflow-y-auto"
          role={question.multiSelect ? "group" : "radiogroup"}
          aria-labelledby={labelId}
        >
          {options.map((option, optionIndex) => {
            const isCustom = isOtherOption(option) || option.id === CUSTOM_OPTION_ID;
            const active = selected.includes(option.id);
            return (
              <div key={option.id}>
                <button
                  ref={(button) => {
                    optionButtons.current[optionIndex] = button;
                  }}
                  type="button"
                  role={question.multiSelect ? "checkbox" : "radio"}
                  aria-checked={active}
                  aria-keyshortcuts={optionIndex < 9 ? String(optionIndex + 1) : undefined}
                  disabled={disabled}
                  tabIndex={highlighted === optionIndex ? 0 : -1}
                  onFocus={() => setHighlighted(optionIndex)}
                  onKeyDown={(event) => onOptionKeyDown(event, optionIndex)}
                  onClick={() => {
                    setHighlighted(optionIndex);
                    onSelect(option.id);
                  }}
                  className={cn(
                    "flex min-h-11 w-full items-start gap-2.5 rounded-row border px-3 py-2.5 text-left disabled:opacity-40",
                    active ? "border-muted-text bg-selection" : "border-stroke hover:bg-content-a04",
                  )}
                >
                  <span
                    aria-hidden="true"
                    className={cn(
                      "mt-0.5 grid size-[18px] shrink-0 place-items-center border-[1.5px]",
                      question.multiSelect ? "rounded-[5px]" : "rounded-full",
                      active ? "border-content bg-content text-inverse" : "border-faint",
                    )}
                  >
                    {active ? <Check size={11} strokeWidth={2.5} /> : null}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-body text-content">{option.label}</span>
                    {option.description ? (
                      <span className="mt-0.5 block text-small text-muted-text">{option.description}</span>
                    ) : null}
                  </span>
                </button>
                {isCustom && (active || customSelected) ? (
                  <input
                    value={custom}
                    disabled={disabled}
                    aria-label="Your answer"
                    onChange={(event) => onCustom(event.target.value)}
                    placeholder="Type your answer"
                    className="mt-1.5 min-h-10 w-full rounded-row border border-stroke bg-transparent px-3 text-body text-content outline-none placeholder:text-faint focus:border-muted-text"
                    onFocus={() => {
                      if (!customSelected) onSelect(customId);
                    }}
                  />
                ) : null}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
