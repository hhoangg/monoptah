// Copied from mobile/src/features/workspace/watchUpdates.ts at 5e188b47, adapted to the protocol
// types of @monoptah/protocol and extended with the `cloud` and `approvals` fields of frames
// (architecture.md 6.6: a frame carries a field only on change, absent means unchanged).
import type { Approval, CloudField, QuestionPrompt, Turn, UsageLimit, WatchFrame as ProtocolWatchFrame } from "@monoptah/protocol";

/**
 * A frame of a watched conversation, from the desktop's watch update. Every field is optional here:
 * an error frame carries only `error`, and the protocol schema is looser than what the desktop sends.
 */
export type WatchFrameInput = Partial<ProtocolWatchFrame> & { error?: string };

type WatchEdit = {
  path: (string | number)[];
  value?: unknown;
  append?: string;
  from?: number;
  remove?: true;
};

/** Copy only the edited branch, preserving untouched item identities. */
function editValue(value: unknown, edit: WatchEdit, depth = 0): unknown {
  if (depth === edit.path.length) {
    if (edit.append !== undefined) {
      if (typeof value !== "string" || value.length !== edit.from) throw new Error("Transcript append needs a fresh snapshot");
      return value + edit.append;
    }
    return edit.value;
  }
  const key = edit.path[depth];
  const last = depth === edit.path.length - 1;
  if (
    Array.isArray(value) &&
    typeof key === "number" &&
    Number.isInteger(key) &&
    key >= 0 &&
    (key < value.length || (key === value.length && last && "value" in edit))
  ) {
    const next = value.slice();
    if (last && edit.remove) next.splice(key, 1);
    else next[key] = editValue(value[key], edit, depth + 1);
    return next;
  }
  if (value && typeof value === "object" && !Array.isArray(value) && typeof key === "string") {
    const record = value as Record<string, unknown>;
    if (!Object.prototype.hasOwnProperty.call(record, key) && !(last && "value" in edit)) {
      throw new Error("Transcript path needs a fresh snapshot");
    }
    if (last && edit.remove) {
      const next = { ...record };
      delete next[key];
      return next;
    }
    return { ...record, [key]: editValue(record[key], edit, depth + 1) };
  }
  throw new Error("Transcript path needs a fresh snapshot");
}

export interface AppliedFrames {
  turns: Turn[];
  usageLimit?: UsageLimit | null;
  busy?: boolean;
  older?: boolean;
  /** Ordered transitions: a clear allows a later question to reuse its ID. */
  questions: (QuestionPrompt | null)[];
  error?: string;
  /** The last `cloud` a frame carried; undefined when no frame carried one (unchanged). */
  cloud?: CloudField | null;
  /** The last `approvals` a frame carried (an empty array clears); undefined when unchanged. */
  approvals?: Approval[];
}

/**
 * Applies frames to the turns the client has: the order says which turns there
 * are, and only the ones that changed come with it. Unchanged turns keep their
 * identity, so their rows skip rendering. Null when a frame names a turn the
 * client never got, which needs a fresh watch.
 */
export function applyFrames(previous: Turn[], frames: WatchFrameInput[]): AppliedFrames | null {
  let turns = previous;
  let busy: boolean | undefined;
  let usageLimit: UsageLimit | null | undefined;
  const questions: (QuestionPrompt | null)[] = [];
  let older: boolean | undefined;
  let error: string | undefined;
  let cloud: CloudField | null | undefined;
  let approvals: Approval[] | undefined;
  for (const frame of frames) {
    if (frame.error) {
      error = frame.error;
      continue;
    }
    error = undefined;
    if (frame.usageLimit !== undefined) usageLimit = frame.usageLimit;
    if (frame.busy !== undefined) busy = frame.busy;
    if (frame.older !== undefined) older = frame.older;
    if (frame.question !== undefined) questions.push(frame.question);
    if (frame.cloud !== undefined) cloud = frame.cloud;
    if (frame.approvals !== undefined) approvals = frame.approvals;
    if (!frame.order) continue;
    const known = new Map(turns.map((turn) => [turn.id, turn]));
    for (const turn of frame.turns ?? []) known.set(turn.id, turn);
    try {
      for (const patch of frame.patches ?? []) {
        let turn = known.get(patch.id);
        if (!turn) return null;
        for (const edit of patch.edits) turn = editValue(turn, edit) as Turn;
        if (turn.id !== patch.id) return null;
        known.set(patch.id, turn);
      }
    } catch {
      return null;
    }
    const next: Turn[] = [];
    for (const id of frame.order) {
      const turn = known.get(id);
      if (!turn) return null;
      next.push(turn);
    }
    turns = next.length === turns.length && next.every((turn, index) => turn === turns[index]) ? turns : next;
  }
  return {
    turns,
    busy,
    older,
    questions,
    error,
    ...(usageLimit !== undefined ? { usageLimit } : {}),
    ...(cloud !== undefined ? { cloud } : {}),
    ...(approvals !== undefined ? { approvals } : {}),
  };
}
