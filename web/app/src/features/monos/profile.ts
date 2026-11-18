// A Mono's profile as the desktop's Overview tab has it (`mono.profile`, F-76, F-77) and a document (`mono.document`),
// read defensively: the wire result is an open object, so every field is checked and a missing one reads as empty.
// Types follow mobile/src/features/workspace/monoProfile.ts.
import { useLinkQuery } from "@/data";

export interface MonoRun {
  at: number;
  durationMs: number;
  outcome: string;
  report: string | null;
  error: string | null;
}

export interface MonoHabit {
  id: string;
  name: string;
  instructions: string;
  schedule: string;
  enabled: boolean;
  when: string;
  running: boolean;
  lastRunAt: number | null;
  lastOutcome: string | null;
  lastError: string | null;
  runs: MonoRun[];
}

export interface MonoDocumentRow {
  id: string;
  title: string;
  kind: string;
  summary: string;
  updatedAt: number;
}

export interface MonoProfile {
  status: string;
  state: "idle" | "working" | "needs-you";
  contributions: { total: number; streak: number; levels: number[] };
  today: { merged: number; additions: number; deletions: number; sessions: number; habitsRan: number; habitsTotal: number };
  upNext: { id: string; name: string; schedule: string; when: string; running: boolean }[];
  habits: MonoHabit[];
  documents: MonoDocumentRow[];
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const str = (value: unknown, fallback = ""): string => (typeof value === "string" ? value : fallback);
const num = (value: unknown, fallback = 0): number => (typeof value === "number" && Number.isFinite(value) ? value : fallback);
const bool = (value: unknown): boolean => value === true;
const list = (value: unknown): Record<string, unknown>[] => (Array.isArray(value) ? value.filter(isRecord) : []);

function readRun(value: Record<string, unknown>): MonoRun {
  return { at: num(value.at), durationMs: num(value.durationMs), outcome: str(value.outcome), report: typeof value.report === "string" ? value.report : null, error: typeof value.error === "string" ? value.error : null };
}

function readHabit(value: Record<string, unknown>): MonoHabit {
  return {
    id: str(value.id),
    name: str(value.name),
    instructions: str(value.instructions),
    schedule: str(value.schedule),
    enabled: bool(value.enabled),
    when: str(value.when),
    running: bool(value.running),
    lastRunAt: typeof value.lastRunAt === "number" ? value.lastRunAt : null,
    lastOutcome: typeof value.lastOutcome === "string" ? value.lastOutcome : null,
    lastError: typeof value.lastError === "string" ? value.lastError : null,
    runs: list(value.runs).map(readRun),
  };
}

/** The profile in a `mono.profile` result; null when the result is not an object. */
export function readProfile(value: unknown): MonoProfile | null {
  if (!isRecord(value)) return null;
  const contributions = isRecord(value.contributions) ? value.contributions : {};
  const today = isRecord(value.today) ? value.today : {};
  const state = value.state === "working" || value.state === "needs-you" ? value.state : "idle";
  return {
    status: str(value.status),
    state,
    contributions: {
      total: num(contributions.total),
      streak: num(contributions.streak),
      levels: Array.isArray(contributions.levels) ? contributions.levels.map((level) => Math.min(Math.max(Math.round(num(level)), 0), 4)) : [],
    },
    today: {
      merged: num(today.merged),
      additions: num(today.additions),
      deletions: num(today.deletions),
      sessions: num(today.sessions),
      habitsRan: num(today.habitsRan),
      habitsTotal: num(today.habitsTotal),
    },
    upNext: list(value.upNext).map((habit) => ({ id: str(habit.id), name: str(habit.name), schedule: str(habit.schedule), when: str(habit.when), running: bool(habit.running) })),
    habits: list(value.habits).map(readHabit),
    documents: list(value.documents).map((document) => ({
      id: str(document.id),
      title: str(document.title, "Untitled"),
      kind: str(document.kind),
      summary: str(document.summary),
      updatedAt: num(document.updatedAt),
    })),
  };
}

/** The profile of a Mono (read-only; allowed for read-only devices), with a refresh that replaces pull-to-refresh. */
export function useMonoProfile(monoId: string) {
  const query = useLinkQuery("mono.profile", { monoId });
  const profile = query.data === undefined ? null : readProfile(query.data);
  return {
    profile,
    loading: query.isPending,
    error: query.isError ? (query.error.message || "Could not load the profile.") : null,
    refreshing: query.isFetching && !query.isPending,
    refresh: () => void query.refetch(),
  };
}

export interface MonoDocumentBody {
  title: string;
  kind: string;
  body: string;
  /** A visual's component tree, as the desktop rendered it. */
  tree: unknown;
  treeError: string | null;
}

export function readDocument(value: unknown): MonoDocumentBody | null {
  if (!isRecord(value)) return null;
  return {
    title: str(value.title, "Document"),
    kind: str(value.kind),
    // The desktop sends `body`; `text` and `content` are what the WS-T document dialog looked for.
    body: str(value.body, str(value.text, str(value.content))),
    tree: value.tree ?? null,
    treeError: typeof value.treeError === "string" ? value.treeError : null,
  };
}

/** How long ago, in the desktop's short form: now, 5m, 3h, 2d. */
export function ago(ms: number, now = Date.now()): string {
  const seconds = Math.max(0, (now - ms) / 1000);
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86400)}d`;
}
