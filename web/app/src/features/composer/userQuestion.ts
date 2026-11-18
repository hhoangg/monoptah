// Copied from src/features/sessions/model/userQuestion.ts at ca73ac2b
// Helpers over the protocol question types. `questionsFromUnknown` (harness input parsing) is not
// carried over: the desktop already sends normalised questions. The display helpers at the end
// come from mobile/src/features/sessions/QuestionCard.tsx so both answer alike.
import type { QuestionPrompt, QuestionReply } from "@monoptah/protocol";

export type Question = QuestionPrompt["questions"][number];
export type QuestionOption = Question["options"][number];
export type AnsweredQuestionReply = Extract<QuestionReply, { kind: "answered" }>;
export type QuestionAnswers = Record<string, string[]>;
export type QuestionCustom = Record<string, string>;

/** Id of the synthetic "Other" option (desktop `CUSTOM_OPTION_ID`). */
export const CUSTOM_OPTION_ID = "__custom__";

/** The countdown shows only for the last minute before the agent carries on (F-58). */
export const QUESTION_COUNTDOWN_MS = 60_000;

export function questionPromptTitle(questions: Question[]): string {
  const first = questions[0];
  return first?.header || first?.prompt || "Question";
}

export function questionIsComplete(
  question: Question,
  answers: QuestionAnswers,
  custom: QuestionCustom = {},
): boolean {
  const selected = answers[question.id] ?? [];
  if (selected.length === 0) {
    return question.allowCustom && !!custom[question.id]?.trim();
  }
  if (!question.multiSelect && selected.length !== 1) return false;
  return selected.every((id) => {
    if (!isCustomSelection(question, id)) return true;
    return !!custom[question.id]?.trim();
  });
}

export function questionAnswersComplete(
  questions: Question[],
  answers: QuestionAnswers,
  custom: QuestionCustom = {},
): boolean {
  if (questions.length === 0) return false;
  return questions.every((question) => questionIsComplete(question, answers, custom));
}

/** Skip every question gives `skipped`. Any answered question gives `answered` with those only. */
export function buildQuestionReply(
  questions: Question[],
  answers: QuestionAnswers,
  custom: QuestionCustom = {},
): QuestionReply {
  const answered = questions.filter((question) => questionIsComplete(question, answers, custom));
  if (answered.length === 0) return { kind: "skipped" };
  const nextAnswers: QuestionAnswers = {};
  const nextCustom: QuestionCustom = {};
  for (const question of answered) {
    const selected = answers[question.id];
    if (selected?.length) nextAnswers[question.id] = selected;
    const text = custom[question.id]?.trim();
    if (text) nextCustom[question.id] = text;
  }
  return {
    kind: "answered",
    answers: nextAnswers,
    ...(Object.keys(nextCustom).length > 0 ? { custom: nextCustom } : {}),
  };
}

export function selectedAnswerLabels(question: Question, reply: AnsweredQuestionReply): string[] {
  const custom = reply.custom?.[question.id]?.trim();
  const selected = reply.answers[question.id] ?? [];
  if (selected.length === 0 && custom && question.allowCustom) return [custom];
  return selected.flatMap((id) => {
    if (isCustomSelection(question, id)) return custom ? [custom] : [];
    const option = question.options.find((item) => item.id === id);
    const label = option?.label.trim() || id.trim();
    return label ? [label] : [];
  });
}

export function isCustomSelection(question: Question, optionId: string): boolean {
  if (optionId === CUSTOM_OPTION_ID) return true;
  const option = question.options.find((item) => item.id === optionId);
  return option ? isOtherOption(option) : false;
}

export function isOtherOption(option: Pick<QuestionOption, "id" | "label">): boolean {
  return option.id === CUSTOM_OPTION_ID || /^other$/i.test(option.label.trim());
}

/** The options to show: an "Other" row is appended when custom text is allowed and none exists. */
export function displayOptions(question: Question): QuestionOption[] {
  if (question.options.length === 0) return question.options;
  if (question.options.some(isOtherOption) || !question.allowCustom) return question.options;
  return [...question.options, { id: CUSTOM_OPTION_ID, label: "Other" }];
}

export function customOptionId(question: Question): string {
  return question.options.find(isOtherOption)?.id ?? CUSTOM_OPTION_ID;
}

export function isCustomId(question: Question, optionId: string): boolean {
  return optionId === CUSTOM_OPTION_ID || optionId === customOptionId(question);
}

/** The selection after toggling `optionId`: single-select replaces, multi-select toggles. */
export function nextSelection(question: Question, current: string[], optionId: string): string[] {
  if (!question.multiSelect) return [optionId];
  if (current.includes(optionId)) return current.filter((id) => id !== optionId);
  if (isCustomId(question, optionId)) {
    return [...current.filter((id) => !isCustomId(question, id)), optionId];
  }
  return [...current, optionId];
}

/** The selection after the "Other" text changes: typing selects "Other" (and drops a prior pick when single). */
export function selectionAfterCustomText(question: Question, current: string[]): string[] {
  if (!question.multiSelect) return [customOptionId(question)];
  return [...current.filter((id) => !isCustomId(question, id)), customOptionId(question)];
}

export type QuestionCountdown =
  | { kind: "none" }
  | { kind: "optional" }
  | { kind: "counting"; seconds: number };

/**
 * What the footer says about the agent's own deadline. More than a minute out it is just an
 * optional question; inside the last minute it counts down. `autoResolveAt` is a ms epoch.
 */
export function questionCountdown(autoResolveAt: number | undefined, now: number): QuestionCountdown {
  if (autoResolveAt == null) return { kind: "none" };
  const left = autoResolveAt - now;
  if (left > QUESTION_COUNTDOWN_MS) return { kind: "optional" };
  return { kind: "counting", seconds: Math.max(0, Math.ceil(left / 1000)) };
}
