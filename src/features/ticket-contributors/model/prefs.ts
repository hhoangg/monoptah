import { parsePick, serializePick } from "../../contributors/model/prefs";
import type { PeriodPick } from "../../contributors/model/period";
import { DEFAULT_TICKET_POINTS, LEVELS } from "./tickets";
import type { ExcludedMember, Level, WorkspaceScope } from "./tickets";

// The `monocode.` prefix is deliberately kept, like every other preference key.
export const SETTINGS_KEY = "monocode.ticketContributors.settings";
export const RANGE_KEY = "monocode.ticketContributors.range";

export type TicketBoardSettings = {
  scope: WorkspaceScope | null;
  points: Record<Level, number>;
  percent: number;
};

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // private mode / quota: the control still works for this window
  }
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const strings = (value: unknown): string[] =>
  Array.isArray(value)
    ? value.filter((v): v is string => typeof v === "string")
    : [];

/** Old scopes stored bare ids; those read as a member named by its id. */
function parseExcludedMembers(value: unknown): ExcludedMember[] {
  if (!Array.isArray(value)) return [];
  const seen = new Map<string, ExcludedMember>();
  for (const entry of value) {
    let member: ExcludedMember | null = null;
    if (typeof entry === "string") {
      member = { id: entry, name: entry };
    } else if (isRecord(entry) && typeof entry.id === "string") {
      const name = typeof entry.name === "string" ? entry.name.trim() : "";
      member = { id: entry.id, name: name === "" ? entry.id : name };
    }
    if (member && member.id !== "" && !seen.has(member.id)) {
      seen.set(member.id, member);
    }
  }
  return [...seen.values()];
}

function parseScope(value: unknown): WorkspaceScope | null {
  if (!isRecord(value)) return null;
  if (typeof value.workspaceId !== "string" || value.workspaceId === "") {
    return null;
  }
  const spaceIds = strings(value.spaceIds)
    .map((s) => s.trim())
    .filter((s) => s !== "");
  return {
    workspaceId: value.workspaceId,
    name: typeof value.name === "string" ? value.name : "",
    // Every space of the workspace: nothing (or only blanks) was picked.
    spaceIds: spaceIds.length > 0 ? spaceIds : null,
    excludedStatuses: strings(value.excludedStatuses),
    // Scopes saved before delivered statuses existed (or a stored `null`) add none.
    deliveredStatuses: strings(value.deliveredStatuses),
    // Scopes saved before excluded members existed (or a stored `null`) exclude nobody.
    excludedMembers: parseExcludedMembers(value.excludedMembers),
  };
}

function parsePoints(value: unknown): Record<Level, number> {
  const points = { ...DEFAULT_TICKET_POINTS };
  if (!isRecord(value)) return points;
  for (const level of LEVELS) {
    const n = value[level];
    if (typeof n === "number" && Number.isFinite(n) && n >= 0) {
      points[level] = n;
    }
  }
  return points;
}

function parsePercent(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return 100;
  return Math.min(100, Math.max(0, value));
}

/** The saved settings; a missing or malformed value reads as the defaults, field by field. */
export function readSettings(): TicketBoardSettings {
  let saved: unknown = null;
  try {
    const text = read(SETTINGS_KEY);
    saved = text === null ? null : JSON.parse(text);
  } catch {
    saved = null;
  }
  const obj = isRecord(saved) ? saved : {};
  return {
    scope: parseScope(obj.scope),
    points: parsePoints(obj.points),
    percent: parsePercent(obj.percent),
  };
}

export function writeSettings(s: TicketBoardSettings): void {
  write(SETTINGS_KEY, JSON.stringify(s));
}

/** The saved range. There is no 7d here (tickets close too rarely), so it reads as the month. */
export function readRange(now: Date = new Date()): PeriodPick {
  const pick = parsePick(read(RANGE_KEY), now);
  if (!pick || pick.kind === "7d") return { kind: "month", id: null };
  return pick;
}

export const writeRange = (pick: PeriodPick) =>
  write(RANGE_KEY, serializePick(pick));
