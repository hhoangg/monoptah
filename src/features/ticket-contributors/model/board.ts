import { isInPeriod } from "../../contributors/model/period";
import type { Period } from "../../contributors/model/period";
import {
  LEVELS,
  deliveredAt,
  effectiveLevel,
  isActive,
  isDelivered,
  withIncludedAssignees,
} from "./tickets";
import type {
  ExcludedMember,
  Level,
  TicketRecord,
  WorkspaceScope,
} from "./tickets";

/** The start of the All time window: ClickUp tickets cannot be older than this. */
export const ALL_START = "2015-01-01T00:00:00Z";

export type MemberRow = {
  id: string;
  name: string;
  points: number;
  tickets: number;
  assessed: number;
  /** Tickets in a working status in the period. Not scored. */
  active: number;
  levels: Record<Level, number>;
  ticketList: {
    ticket: TicketRecord;
    level: Level | null;
    /** What the ticket is worth in total. */
    points: number;
    /** This member's part: `points` split equally between the assignees. */
    share: number;
  }[];
};

export type Board = {
  total: number;
  assessed: number;
  members: MemberRow[];
  unassigned: { tickets: number; assessed: number };
  /** Some counted ticket was dated by its last update, not by a delivery date. */
  datesApproximate: boolean;
};

/**
 * Tickets delivered in the period inside the workspace scope. Excluded members
 * are taken off the tickets, so everything built from these sees only the
 * included assignees; a ticket left with none drops out.
 */
export function periodTickets(
  tickets: TicketRecord[],
  scope: WorkspaceScope,
  period: Period,
): TicketRecord[] {
  return tickets
    .filter(
      (t) => isDelivered(t, scope) && isInPeriod(period, deliveredAt(t).at),
    )
    .map((t) => withIncludedAssignees(t, scope));
}

/** Tickets in a working status that were updated in the period, with included assignees only. */
export function activeTickets(
  tickets: TicketRecord[],
  scope: WorkspaceScope,
  period: Period,
): TicketRecord[] {
  return tickets
    .filter((t) => isActive(t, scope) && isInPeriod(period, t.updatedAt))
    .map((t) => withIncludedAssignees(t, scope));
}

/** Active tickets per member id; a ticket counts once for each distinct assignee. */
export function activeByMember(
  tickets: TicketRecord[],
  scope: WorkspaceScope,
  period: Period,
): Map<string, number> {
  const counts = new Map<string, number>();
  for (const t of activeTickets(tickets, scope, period)) {
    for (const m of distinctAssignees(t)) {
      counts.set(m.id, (counts.get(m.id) ?? 0) + 1);
    }
  }
  return counts;
}

/** Distinct statuses of the tickets (first spelling wins), sorted by type then name. */
export function statusesOf(
  tickets: TicketRecord[],
): { name: string; type: string }[] {
  const seen = new Map<string, { name: string; type: string }>();
  for (const t of tickets) {
    const name = t.status.trim();
    const key = name.toLowerCase();
    if (key !== "" && !seen.has(key)) {
      seen.set(key, { name, type: t.statusType });
    }
  }
  return [...seen.values()].sort(
    (a, b) => a.type.localeCompare(b.type) || a.name.localeCompare(b.name),
  );
}

/**
 * The excluded members with their current names: the name a loaded ticket
 * carries, else the one stored when they were excluded. Returns the scope's own
 * array when no name changed, so the caller can tell whether to save.
 */
export function excludedMembersOf(
  tickets: TicketRecord[],
  scope: WorkspaceScope,
): ExcludedMember[] {
  if (scope.excludedMembers.length === 0) return scope.excludedMembers;
  const names = new Map<string, string>();
  for (const t of tickets) {
    for (const a of t.assignees) {
      const name = a.name.trim();
      if (name !== "" && !names.has(a.id)) names.set(a.id, name);
    }
  }
  let changed = false;
  const next = scope.excludedMembers.map((m) => {
    const name = names.get(m.id);
    if (name === undefined || name === m.name) return m;
    changed = true;
    return { id: m.id, name };
  });
  return changed ? next : scope.excludedMembers;
}

const emptyLevels = (): Record<Level, number> =>
  Object.fromEntries(LEVELS.map((l) => [l, 0])) as Record<Level, number>;

/** Distinct assignees by id, first occurrence wins. */
function distinctAssignees(t: TicketRecord) {
  const seen = new Map<string, { id: string; name: string }>();
  for (const a of t.assignees) {
    if (!seen.has(a.id)) seen.set(a.id, { id: a.id, name: a.name });
  }
  return [...seen.values()];
}

/**
 * The board of already period-filtered tickets. A ticket's points are split
 * equally between its distinct assignees; an unassessed ticket is worth 0.
 * `active` (see `activeTickets`) only adds each member's Active count, and
 * lists members who have nothing else. Members are sorted by points, highest
 * first, then by name.
 */
export function buildBoard(
  tickets: TicketRecord[],
  points: Record<Level, number>,
  active: TicketRecord[] = [],
): Board {
  const rows = new Map<string, MemberRow>();
  const unassigned = { tickets: 0, assessed: 0 };
  let assessed = 0;
  let datesApproximate = false;

  const rowOf = (m: { id: string; name: string }): MemberRow => {
    let row = rows.get(m.id);
    if (!row) {
      row = {
        id: m.id,
        name: m.name,
        points: 0,
        tickets: 0,
        assessed: 0,
        active: 0,
        levels: emptyLevels(),
        ticketList: [],
      };
      rows.set(m.id, row);
    }
    return row;
  };

  for (const ticket of tickets) {
    if (deliveredAt(ticket).approximate) datesApproximate = true;
    const level = effectiveLevel(ticket);
    const worth = level ? points[level] : 0;
    if (level) assessed += 1;

    const members = distinctAssignees(ticket);
    if (members.length === 0) {
      unassigned.tickets += 1;
      if (level) unassigned.assessed += 1;
      continue;
    }
    const share = worth / members.length;
    for (const m of members) {
      const row = rowOf(m);
      row.points += share;
      row.tickets += 1;
      if (level) {
        row.assessed += 1;
        row.levels[level] += 1;
      }
      row.ticketList.push({ ticket, level, points: worth, share });
    }
  }

  for (const ticket of active) {
    for (const m of distinctAssignees(ticket)) rowOf(m).active += 1;
  }

  const members = [...rows.values()].sort(
    (a, b) =>
      b.points - a.points ||
      a.name.localeCompare(b.name) ||
      a.id.localeCompare(b.id),
  );
  return {
    total: tickets.length,
    assessed,
    members,
    unassigned,
    datesApproximate,
  };
}

/**
 * How many more tickets to assess to cover `percent` of the `total`:
 * `max(0, ceil(percent * total / 100) - assessed)`.
 */
export function toAssessCount(
  total: number,
  assessed: number,
  percent: number,
): number {
  const p = Number.isFinite(percent) ? Math.min(100, Math.max(0, percent)) : 0;
  const target = Math.ceil((p * total) / 100);
  return Math.max(0, target - assessed);
}

const deliveredTime = (t: TicketRecord) => {
  const time = Date.parse(deliveredAt(t).at);
  return Number.isNaN(time) ? Number.POSITIVE_INFINITY : time;
};

const byDeliveredThenId = (a: TicketRecord, b: TicketRecord) => {
  const ta = deliveredTime(a);
  const tb = deliveredTime(b);
  if (ta !== tb) return ta < tb ? -1 : 1;
  return a.id.localeCompare(b.id);
};

/**
 * The ids to assess in one run: `count` unassessed tickets, round-robin across
 * members (ordered by their oldest unassessed ticket, then id), each member's
 * oldest-delivered ticket first. A ticket with several assignees is taken once,
 * at its first member's turn. Unassigned tickets come last, oldest first.
 */
export function pickForRun(tickets: TicketRecord[], count: number): string[] {
  if (!(count > 0)) return [];
  const open = tickets
    .filter((t) => effectiveLevel(t) === null)
    .sort(byDeliveredThenId);

  const queues = new Map<string, TicketRecord[]>();
  const unassigned: TicketRecord[] = [];
  for (const ticket of open) {
    const members = distinctAssignees(ticket);
    if (members.length === 0) unassigned.push(ticket);
    for (const m of members) {
      const queue = queues.get(m.id);
      if (queue) queue.push(ticket);
      else queues.set(m.id, [ticket]);
    }
  }

  // Queues are already oldest first, so the head is the member's oldest ticket.
  const order = [...queues.entries()].sort(
    ([idA, a], [idB, b]) =>
      byDeliveredThenId(a[0], b[0]) || idA.localeCompare(idB),
  );

  const picked: string[] = [];
  const taken = new Set<string>();
  let progressed = true;
  while (picked.length < count && progressed) {
    progressed = false;
    for (const [, queue] of order) {
      if (picked.length >= count) break;
      while (queue.length > 0 && taken.has(queue[0].id)) queue.shift();
      const next = queue.shift();
      if (!next) continue;
      taken.add(next.id);
      picked.push(next.id);
      progressed = true;
    }
  }
  for (const ticket of unassigned) {
    if (picked.length >= count) break;
    picked.push(ticket.id);
  }
  return picked;
}
