/**
 * How a question ended, keyed by session and request id. The session state
 * only drops `pendingQuestion`, so the harness decision is noted here for the
 * Telegram message edit ("Answered on desktop" vs "Skipped" vs "Cancelled").
 */
export type QuestionDecision = "answered" | "skipped" | "cancelled";

const MAX_OUTCOMES = 200;
const outcomes = new Map<string, QuestionDecision>();

function outcomeKey(sessionId: string, requestId: number): string {
  return `${sessionId}#${requestId}`;
}

export function recordQuestionOutcome(
  sessionId: string,
  requestId: number,
  decision: QuestionDecision,
): void {
  const key = outcomeKey(sessionId, requestId);
  outcomes.delete(key);
  outcomes.set(key, decision);
  if (outcomes.size > MAX_OUTCOMES) {
    const oldest = outcomes.keys().next().value;
    if (oldest !== undefined) outcomes.delete(oldest);
  }
}

/** Reads an outcome without forgetting it. */
export function peekQuestionOutcome(
  sessionId: string,
  requestId: number,
): QuestionDecision | undefined {
  return outcomes.get(outcomeKey(sessionId, requestId));
}

/** Reads and forgets the outcome of a question. */
export function takeQuestionOutcome(
  sessionId: string,
  requestId: number,
): QuestionDecision | undefined {
  const key = outcomeKey(sessionId, requestId);
  const decision = outcomes.get(key);
  outcomes.delete(key);
  return decision;
}

/** Forgets an outcome without reading it, e.g. one left by an earlier question with the same key. */
export function discardQuestionOutcome(
  sessionId: string,
  requestId: number,
): void {
  outcomes.delete(outcomeKey(sessionId, requestId));
}
