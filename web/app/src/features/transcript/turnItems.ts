// Typed view of the turn items the desktop sends. The protocol keeps turn items open (`{ type, id? }`
// with extra fields), so the shapes of mobile `workspace/types.ts` are re-stated here and a turn is
// narrowed once, in `readItems`. An item of a type this client does not know (a newer desktop) is
// dropped, as mobile does (F-34).
import { MONO_EMOJI_MAX_PICKS, MonoEmojiPickSchema, type MonoEmojiPick, type Turn } from "@monoptah/protocol";

export type WorkKind = "research" | "edit" | "run" | "agent" | "other" | "think" | "note";
export interface WorkStep {
  id: string;
  label: string;
  failed?: boolean;
}
export interface WorkPhase {
  id: string;
  kind: WorkKind;
  title: string;
  steps: WorkStep[];
}
export type FoldEntry = { type: "prose"; id: string; text: string } | ({ type: "phase" } & WorkPhase);

export type LaunchedSessionState = "working" | "input" | "draft" | "failed" | "stopped" | "done";
export interface LaunchedSession {
  id: string;
  projectId: string;
  project: string;
  title: string;
  harness: string;
  state: LaunchedSessionState;
}

export type UserItem = {
  type: "user";
  id: string;
  text: string;
  at?: number;
  reaction?: string;
  emojiOnly?: boolean;
  attachments?: number;
  compact?: boolean;
};
/** `emoji`: the pictures of a Mono reply, each at a UTF-16 offset of `text` (a Mono transcript only; validated by `readEmoji`). */
export type TextItem = { type: "text"; id: string; text: string; streaming?: boolean; emoji?: MonoEmojiPick[] };
export type HeaderItem = {
  type: "header";
  id: string;
  live: boolean;
  label?: string;
  waiting?: string;
  startedAt?: number;
  durationMs?: number;
  model?: string;
  harness?: string;
  entries: FoldEntry[];
};
export type WorkItem = { type: "work"; id: string; live: boolean; phases: WorkPhase[] };
export type NoticeItem = { type: "notice"; id: string; text: string; error?: boolean };
export type SessionsItem = { type: "sessions"; id: string; sessions: LaunchedSession[] };
export type ChoicesItem = { type: "choices"; id: string; options: string[]; chosen?: string };
export type DocumentItem = { type: "document"; id: string; documentId: string; title: string; kind: string };

export type TurnItem = UserItem | TextItem | HeaderItem | WorkItem | NoticeItem | SessionsItem | ChoicesItem | DocumentItem;

export interface TurnMetrics {
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  cacheHitPercent?: number;
}

const KNOWN = new Set(["user", "text", "header", "work", "notice", "sessions", "choices", "document"]);
const KINDS = new Set<string>(["research", "edit", "run", "agent", "other", "think", "note"]);

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const str = (value: unknown, fallback = ""): string => (typeof value === "string" ? value : fallback);
const num = (value: unknown): number | undefined => (typeof value === "number" && Number.isFinite(value) ? value : undefined);

/**
 * The valid pictures of a text item: invalid entries dropped, at most `MONO_EMOJI_MAX_PICKS`, in the order of
 * their offsets (a stable sort, so picks at the same offset keep their order) and each offset clamped to the text.
 */
function readEmoji(value: unknown, textLength: number): MonoEmojiPick[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const picks: MonoEmojiPick[] = [];
  for (const raw of value) {
    const parsed = MonoEmojiPickSchema.safeParse(raw);
    if (parsed.success) picks.push({ ...parsed.data, at: Math.min(parsed.data.at, textLength) });
    if (picks.length >= MONO_EMOJI_MAX_PICKS) break;
  }
  if (picks.length === 0) return undefined;
  return picks.sort((a, b) => a.at - b.at);
}

function steps(value: unknown): WorkStep[] {
  if (!Array.isArray(value)) return [];
  const out: WorkStep[] = [];
  for (const raw of value) {
    if (!isRecord(raw) || typeof raw.id !== "string") continue;
    out.push({ id: raw.id, label: str(raw.label), ...(raw.failed === true ? { failed: true } : {}) });
  }
  return out;
}

function phase(raw: Record<string, unknown>): WorkPhase | null {
  if (typeof raw.id !== "string") return null;
  const kind = typeof raw.kind === "string" && KINDS.has(raw.kind) ? (raw.kind as WorkKind) : "other";
  return { id: raw.id, kind, title: str(raw.title), steps: steps(raw.steps) };
}

function phases(value: unknown): WorkPhase[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((raw) => (isRecord(raw) ? (phase(raw) ?? []) : []));
}

function entries(value: unknown): FoldEntry[] {
  if (!Array.isArray(value)) return [];
  const out: FoldEntry[] = [];
  for (const raw of value) {
    if (!isRecord(raw) || typeof raw.id !== "string") continue;
    if (raw.type === "prose") out.push({ type: "prose", id: raw.id, text: str(raw.text) });
    else if (raw.type === "phase") {
      const found = phase(raw);
      if (found) out.push({ type: "phase", ...found });
    }
  }
  return out;
}

const STATES = new Set<string>(["working", "input", "draft", "failed", "stopped", "done"]);

function launched(value: unknown): LaunchedSession[] {
  if (!Array.isArray(value)) return [];
  const out: LaunchedSession[] = [];
  for (const raw of value) {
    if (!isRecord(raw) || typeof raw.id !== "string" || typeof raw.projectId !== "string") continue;
    out.push({
      id: raw.id,
      projectId: raw.projectId,
      project: str(raw.project),
      title: str(raw.title),
      harness: str(raw.harness),
      state: typeof raw.state === "string" && STATES.has(raw.state) ? (raw.state as LaunchedSessionState) : "done",
    });
  }
  return out;
}

/** Narrows one wire item, or null for an unknown type or an item without an id. */
export function readItem(raw: unknown): TurnItem | null {
  if (!isRecord(raw) || typeof raw.type !== "string" || !KNOWN.has(raw.type) || typeof raw.id !== "string") return null;
  const id = raw.id;
  switch (raw.type) {
    case "user":
      return {
        type: "user",
        id,
        text: str(raw.text),
        ...(num(raw.at) !== undefined ? { at: num(raw.at) } : {}),
        ...(typeof raw.reaction === "string" ? { reaction: raw.reaction } : {}),
        ...(raw.emojiOnly === true ? { emojiOnly: true } : {}),
        ...(num(raw.attachments) ? { attachments: num(raw.attachments) } : {}),
        ...(raw.compact === true ? { compact: true } : {}),
      };
    case "text": {
      const text = str(raw.text);
      const emoji = readEmoji(raw.emoji, text.length);
      return { type: "text", id, text, ...(raw.streaming === true ? { streaming: true } : {}), ...(emoji ? { emoji } : {}) };
    }
    case "header":
      return {
        type: "header",
        id,
        live: raw.live === true,
        ...(typeof raw.label === "string" ? { label: raw.label } : {}),
        ...(typeof raw.waiting === "string" ? { waiting: raw.waiting } : {}),
        ...(num(raw.startedAt) !== undefined ? { startedAt: num(raw.startedAt) } : {}),
        ...(num(raw.durationMs) !== undefined ? { durationMs: num(raw.durationMs) } : {}),
        ...(typeof raw.model === "string" ? { model: raw.model } : {}),
        ...(typeof raw.harness === "string" ? { harness: raw.harness } : {}),
        entries: entries(raw.entries),
      };
    case "work":
      return { type: "work", id, live: raw.live === true, phases: phases(raw.phases) };
    case "notice":
      return { type: "notice", id, text: str(raw.text), ...(raw.error === true ? { error: true } : {}) };
    case "sessions":
      return { type: "sessions", id, sessions: launched(raw.sessions) };
    case "choices":
      return {
        type: "choices",
        id,
        options: Array.isArray(raw.options) ? raw.options.filter((option): option is string => typeof option === "string") : [],
        ...(typeof raw.chosen === "string" ? { chosen: raw.chosen } : {}),
      };
    case "document":
      return { type: "document", id, documentId: str(raw.documentId), title: str(raw.title), kind: str(raw.kind) };
    default:
      return null;
  }
}

const cache = new WeakMap<Turn, TurnItem[]>();

/** The known items of a turn. Cached per turn object, so an unchanged turn keeps its items' identity. */
export function readItems(turn: Turn): TurnItem[] {
  let items = cache.get(turn);
  if (!items) {
    items = turn.items.flatMap((raw) => readItem(raw) ?? []);
    cache.set(turn, items);
  }
  return items;
}

/** Token accounting of the footer's metrics button (the desktop leaves it out for Monos). */
export function readMetrics(turn: Turn): TurnMetrics | undefined {
  const raw = (turn as Record<string, unknown>).metrics;
  if (!isRecord(raw)) return undefined;
  const out: TurnMetrics = {};
  for (const key of ["inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens", "cacheHitPercent"] as const) {
    const value = num(raw[key]);
    if (value !== undefined) out[key] = value;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}
