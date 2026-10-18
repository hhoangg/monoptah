export const LEVELS = ["XS", "S", "M", "L", "XL"] as const;
export type Level = (typeof LEVELS)[number];

export const CRITERIA = [
  "ambiguity",
  "investigation",
  "scope",
  "risk",
  "coordination",
] as const;
export type Criterion = (typeof CRITERIA)[number];
export type Criteria = Record<Criterion, number>;

export type TicketAssessment = {
  criteria: Criteria;
  reasons: Record<Criterion, string>;
  rubricVersion: number;
  model: string;
  runner: "claude-code" | "api";
  assessedAt: string;
};

export type TicketAssignee = { id: string; name: string; email: string };

export type TicketRecord = {
  source: "clickup";
  id: string;
  customId?: string;
  title: string;
  url: string;
  workspaceId: string;
  spaceId: string;
  status: string;
  /** ClickUp status type: `open`, `custom`, `done` or `closed`. */
  statusType: string;
  updatedAt: string;
  /** When ClickUp marked the ticket done; only set for done and closed statuses. */
  doneAt: string | null;
  /** When the ticket entered its current status; `null` without status history. */
  statusSince: string | null;
  parentId: string | null;
  /** Some stored ticket has this one as its parent. */
  hasSubtasks: boolean;
  assignees: TicketAssignee[];
  tags: string[];
  assessment: TicketAssessment | null;
  overrideLevel: Level | null;
  overrideNote: string | null;
};

export type WorkspaceScope = {
  workspaceId: string;
  name: string;
  /** `null` means every space of the workspace. */
  spaceIds: string[] | null;
  excludedStatuses: string[];
  /**
   * Status names that count as delivered work on top of the default (every
   * status of type done or closed). A done or closed status is taken out with
   * `excludedStatuses`, not from here.
   */
  deliveredStatuses: string[];
  /**
   * ClickUp user ids left off the board. An excluded member counts as if they
   * were not an assignee (see `includedAssignees`).
   */
  excludedMembers: ExcludedMember[];
};

/** A member left off the board; the name is kept so the list reads without the member's tickets. */
export type ExcludedMember = { id: string; name: string };

export const DEFAULT_TICKET_POINTS: Record<Level, number> = {
  XS: 3,
  S: 8,
  M: 16,
  L: 32,
  XL: 48,
};

/** Sum of the criteria scores picks the level: two points per step, capped at XL. */
export function levelOf(criteria: Criteria): Level {
  const sum = CRITERIA.reduce((acc, key) => acc + criteria[key], 0);
  const index = Math.min(LEVELS.length - 1, Math.max(0, Math.floor(sum / 2)));
  return LEVELS[index];
}

/** The manual override wins, then the assessment, else the ticket is unassessed. */
export function effectiveLevel(t: TicketRecord): Level | null {
  if (t.overrideLevel) return t.overrideLevel;
  if (t.assessment) return levelOf(t.assessment.criteria);
  return null;
}

/** Status names compare case-insensitively and trimmed. */
const statusKey = (name: string) => name.trim().toLowerCase();

const isFinishedType = (statusType: string) =>
  statusType === "done" || statusType === "closed";

/**
 * The assignees that count on the board: the ticket's own minus the excluded
 * members. The one place that rule lives.
 */
export function includedAssignees(
  t: TicketRecord,
  scope: WorkspaceScope,
): TicketAssignee[] {
  if (scope.excludedMembers.length === 0) return t.assignees;
  const excluded = new Set(scope.excludedMembers.map((m) => m.id));
  return t.assignees.filter((a) => !excluded.has(a.id));
}

/**
 * The ticket as the board sees it: only its included assignees. The same
 * object when nobody is excluded from it, so identity is kept for most tickets.
 */
export function withIncludedAssignees(
  t: TicketRecord,
  scope: WorkspaceScope,
): TicketRecord {
  const included = includedAssignees(t, scope);
  return included.length === t.assignees.length
    ? t
    : { ...t, assignees: included };
}

/**
 * True when the ticket had assignees and every one is excluded, so it drops
 * out of the board. A ticket without any assignee is never dropped.
 */
function droppedByExclusion(t: TicketRecord, scope: WorkspaceScope): boolean {
  return t.assignees.length > 0 && includedAssignees(t, scope).length === 0;
}

/**
 * True when the ticket belongs to the scope's workspace/space, is not in an
 * excluded status and is not left with only excluded assignees.
 */
export function inScope(t: TicketRecord, scope: WorkspaceScope): boolean {
  if (scope.workspaceId !== t.workspaceId) return false;
  if (scope.spaceIds !== null && !scope.spaceIds.includes(t.spaceId)) {
    return false;
  }
  if (droppedByExclusion(t, scope)) return false;
  const status = statusKey(t.status);
  return !scope.excludedStatuses.some((s) => statusKey(s) === status);
}

/** The status rule alone: a done or closed type, or a name chosen as delivered. */
function hasDeliveredStatus(t: TicketRecord, scope: WorkspaceScope): boolean {
  if (isFinishedType(t.statusType)) return true;
  const status = statusKey(t.status);
  return scope.deliveredStatuses.some((s) => statusKey(s) === status);
}

/**
 * Delivered work: in scope (so not excluded, which wins over delivered), in a
 * delivered status, and not a parent whose work lives in its subtasks.
 */
export function isDelivered(t: TicketRecord, scope: WorkspaceScope): boolean {
  return inScope(t, scope) && !t.hasSubtasks && hasDeliveredStatus(t, scope);
}

/**
 * When the ticket was delivered, by preference: its done date (done and closed
 * types), the date it entered its status, its last update. Only the last one
 * is approximate.
 */
export function deliveredAt(t: TicketRecord): {
  at: string;
  approximate: boolean;
} {
  if (isFinishedType(t.statusType) && t.doneAt) {
    return { at: t.doneAt, approximate: false };
  }
  if (t.statusSince) return { at: t.statusSince, approximate: false };
  return { at: t.updatedAt, approximate: true };
}

/**
 * Work in progress: a custom status that is neither delivered nor excluded.
 * A parent with subtasks is left out like it is for delivered work, so the
 * same work is not counted twice. Anchored by `updatedAt`, never scored.
 */
export function isActive(t: TicketRecord, scope: WorkspaceScope): boolean {
  return (
    inScope(t, scope) &&
    !t.hasSubtasks &&
    t.statusType === "custom" &&
    !hasDeliveredStatus(t, scope)
  );
}
