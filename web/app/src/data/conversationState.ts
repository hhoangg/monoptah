// The value stored at `['link', desktopId, 'conversation', conversationId]` and the pure functions that
// change it. Decision (architecture.md 3.2 and 6.6): the `cloud` and `approvals` fields live in this
// TanStack Query entry, next to the turns, not in a second store, so a read, a watch frame and the
// persisted cache all write one place and `useCloud` / `useApprovals` are selectors over it.
import type { Approval, Attachment, CloudField, QuestionPrompt, Turn, UsageLimit } from "@monoptah/protocol";
import type { AppliedFrames } from "./watchUpdates.ts";

/** A message the user sent that the desktop transcript does not hold yet (F-09, F-06). */
export interface OptimisticTurn {
  /** `pending:<n>`: local only, never used as a watch cursor. */
  id: string;
  text: string;
  at: number;
  attachments: number;
  /** `sending`: in flight. `sent`: the desktop accepted it, its turn is not in the transcript yet. `not_sent`: shows "Not sent, retry". */
  status: "sending" | "sent" | "not_sent";
  /** Count of user items in the desktop transcript when it was sent; the turn is confirmed once the count is higher. */
  baseline: number;
  /** Why it was not sent: a link or desktop error code, or the message. */
  error?: string;
}

export interface ConversationState {
  turns: Turn[];
  busy: boolean;
  older: boolean;
  question: QuestionPrompt | null;
  usageLimit: UsageLimit | null;
  /** The last watch error, shown until the next good frame. */
  error: string | null;
  cloud: CloudField | undefined;
  approvals: Approval[];
  /**
   * Counts the times the desktop itself sent the `approvals` field (a frame that carried it, or a read). A card
   * armed before a reconnect compares it with the value it saw then, so a list that survived the reconnect
   * unconfirmed never decides anything (U-13).
   */
  approvalsSeq: number;
  optimistic: OptimisticTurn[];
  /** The turns come from the persisted cache only; the desktop has not confirmed them yet. */
  cachedOnly: boolean;
}

export const NO_APPROVALS: Approval[] = [];

export function emptyConversation(): ConversationState {
  return {
    turns: [],
    busy: false,
    older: false,
    question: null,
    usageLimit: null,
    error: null,
    cloud: undefined,
    approvals: NO_APPROVALS,
    approvalsSeq: 0,
    optimistic: [],
    cachedOnly: false,
  };
}

export function countUserItems(turns: Turn[]): number {
  let count = 0;
  for (const turn of turns) for (const item of turn.items) if (item.type === "user") count += 1;
  return count;
}

/** Error codes that leave it unknown whether the desktop got the message (the reply may have been lost). */
const AMBIGUOUS_ERRORS = new Set(["timeout", "session_ended"]);

function userTexts(turns: Turn[]): string[] {
  const texts: string[] = [];
  for (const turn of turns) for (const item of turn.items) if (item.type === "user") texts.push(typeof item.text === "string" ? item.text.trim() : "");
  return texts;
}

/**
 * Drops optimistic turns whose message the transcript now holds.
 * - `sending` and `sent`: confirmed once the transcript holds more user items than their baseline.
 * - `not_sent`: never dropped by a later message. Only when the failure was ambiguous (`timeout`,
 *   `session_ended`) and a user item after its baseline carries the same text (each transcript item
 *   confirms one turn). A `not_sent` turn with an error that proves non-delivery (`not_ready`, `busy`, a
 *   desktop error code) stays until the user retries it (and it succeeds) or dismisses it.
 */
export function reconcile(state: ConversationState): ConversationState {
  if (state.optimistic.length === 0) return state;
  const count = countUserItems(state.turns);
  const texts = userTexts(state.turns);
  const used = new Set<number>();
  const kept = state.optimistic.filter((turn) => {
    if (turn.status !== "not_sent") return count <= turn.baseline;
    if (!AMBIGUOUS_ERRORS.has(turn.error ?? "")) return true;
    const wanted = turn.text.trim();
    for (let index = Math.max(0, turn.baseline); index < texts.length; index += 1) {
      if (!used.has(index) && texts[index] === wanted) {
        used.add(index);
        return false;
      }
    }
    return true;
  });
  return kept.length === state.optimistic.length ? state : { ...state, optimistic: kept };
}

/** Folds the frames the watch delivered into the state. */
export function withFrames(state: ConversationState, applied: AppliedFrames): ConversationState {
  const lastQuestion = applied.questions.length > 0 ? applied.questions[applied.questions.length - 1] : undefined;
  return reconcile({
    ...state,
    turns: applied.turns,
    busy: applied.busy ?? state.busy,
    older: applied.older ?? state.older,
    question: lastQuestion !== undefined ? lastQuestion : state.question,
    usageLimit: applied.usageLimit !== undefined ? applied.usageLimit : state.usageLimit,
    error: applied.error ?? null,
    // A field a frame does not carry is unchanged; `approvals: []` and `cloud: null` clear.
    cloud: applied.cloud !== undefined ? (applied.cloud ?? undefined) : state.cloud,
    approvals: applied.approvals !== undefined ? applied.approvals : state.approvals,
    approvalsSeq: applied.approvals !== undefined ? state.approvalsSeq + 1 : state.approvalsSeq,
    cachedOnly: false,
  });
}

/** A `conversation.read` result is the whole conversation: it replaces the fields, even to clear them (`cloud` only by an explicit null). */
export function withRead(
  state: ConversationState,
  result: {
    turns: Turn[];
    busy?: boolean | undefined;
    older: boolean;
    usageLimit?: UsageLimit | null | undefined;
    question?: QuestionPrompt | null | undefined;
    cloud?: CloudField | null | undefined;
    approvals?: Approval[] | undefined;
  },
): ConversationState {
  return reconcile({
    ...state,
    turns: keepUnchanged(state.turns, result.turns),
    busy: result.busy ?? false,
    older: result.older,
    question: result.question ?? null,
    usageLimit: result.usageLimit ?? null,
    error: null,
    // Only an explicit `cloud: null` clears it; a read that omits the field (marks not loaded yet) leaves it.
    cloud: result.cloud === undefined ? state.cloud : (result.cloud ?? undefined),
    approvals: result.approvals ?? NO_APPROVALS,
    approvalsSeq: state.approvalsSeq + 1,
    cachedOnly: false,
  });
}

/** The received turns, reusing the previous object of a turn that did not change. */
function keepUnchanged(previous: Turn[], received: Turn[]): Turn[] {
  const known = new Map(previous.map((turn) => [turn.id, turn]));
  const next = received.map((turn) => {
    const old = known.get(turn.id);
    return old && JSON.stringify(old) === JSON.stringify(turn) ? old : turn;
  });
  return next.length === previous.length && next.every((turn, index) => turn === previous[index]) ? previous : next;
}

export function withOptimistic(state: ConversationState, turn: OptimisticTurn): ConversationState {
  return { ...state, optimistic: [...state.optimistic.filter((other) => other.id !== turn.id), turn] };
}

export function withOptimisticStatus(
  state: ConversationState,
  id: string,
  patch: Pick<OptimisticTurn, "status"> & { error?: string | undefined },
): ConversationState {
  return {
    ...state,
    optimistic: state.optimistic.map((turn) => {
      if (turn.id !== id) return turn;
      const { error, ...rest } = turn;
      void error;
      return { ...rest, status: patch.status, ...(patch.error !== undefined ? { error: patch.error } : {}) };
    }),
  };
}

/** Wire attachments count for the optimistic turn. */
export function attachmentCount(attachments: Attachment[] | undefined): number {
  return attachments?.length ?? 0;
}
