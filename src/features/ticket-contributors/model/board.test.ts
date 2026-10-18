import { describe, expect, it } from "vitest";
import {
  ALL_START,
  activeByMember,
  activeTickets,
  buildBoard,
  excludedMembersOf,
  periodTickets,
  pickForRun,
  statusesOf,
  toAssessCount,
} from "./board";
import { DEFAULT_TICKET_POINTS } from "./tickets";
import type {
  TicketAssessment,
  TicketAssignee,
  TicketRecord,
  WorkspaceScope,
} from "./tickets";
import type { Period } from "../../contributors/model/period";

const user = (id: string, name = `user-${id}`): TicketAssignee => ({
  id,
  name,
  email: `${id}@example.com`,
});

const assessment = (score: number): TicketAssessment => ({
  criteria: {
    ambiguity: score,
    investigation: score,
    scope: score,
    risk: score,
    coordination: score,
  },
  reasons: {
    ambiguity: "",
    investigation: "",
    scope: "",
    risk: "",
    coordination: "",
  },
  rubricVersion: 1,
  model: "test",
  runner: "api",
  assessedAt: "2026-10-01T00:00:00Z",
});

function ticket(id: string, patch: Partial<TicketRecord> = {}): TicketRecord {
  return {
    source: "clickup",
    id,
    title: `Ticket ${id}`,
    url: `https://app.clickup.com/t/${id}`,
    workspaceId: "w1",
    spaceId: "s1",
    status: "closed",
    statusType: "closed",
    updatedAt: "2026-10-05T10:00:00Z",
    doneAt: "2026-10-05T10:00:00Z",
    statusSince: null,
    parentId: null,
    hasSubtasks: false,
    assignees: [],
    tags: [],
    assessment: null,
    overrideLevel: null,
    overrideNote: null,
    ...patch,
  };
}

describe("buildBoard", () => {
  it("splits points equally between distinct assignees", () => {
    const board = buildBoard(
      [
        // Score 1 per criterion: sum 5, level index 2 = M = 16 points.
        ticket("a", {
          assignees: [user("1"), user("2"), user("1")],
          assessment: assessment(1),
        }),
        // Override XS = 3 points, one assignee.
        ticket("b", { assignees: [user("1")], overrideLevel: "XS" }),
      ],
      DEFAULT_TICKET_POINTS,
    );
    expect(board.members.map((m) => [m.id, m.points])).toEqual([
      ["1", 11],
      ["2", 8],
    ]);
    const first = board.members[0];
    expect(
      first.ticketList.map((t) => [t.ticket.id, t.level, t.points, t.share]),
    ).toEqual([
      ["a", "M", 16, 8],
      ["b", "XS", 3, 3],
    ]);
    expect(first.levels).toEqual({ XS: 1, S: 0, M: 1, L: 0, XL: 0 });
    expect(board.total).toBe(2);
    expect(board.assessed).toBe(2);
  });

  it("a member's assessed count includes overrides", () => {
    const board = buildBoard(
      [
        ticket("a", { assignees: [user("1")], assessment: assessment(0) }),
        ticket("b", { assignees: [user("1")], overrideLevel: "L" }),
        ticket("c", { assignees: [user("1")] }),
      ],
      DEFAULT_TICKET_POINTS,
    );
    const m = board.members[0];
    expect(m.tickets).toBe(3);
    expect(m.assessed).toBe(2);
    expect(m.points).toBe(3 + 32);
    const open = m.ticketList.find((t) => t.ticket.id === "c");
    expect(open).toMatchObject({ level: null, points: 0, share: 0 });
    expect(board.assessed).toBe(2);
  });

  it("counts unassigned tickets apart", () => {
    const board = buildBoard(
      [
        ticket("a", { overrideLevel: "S" }),
        ticket("b"),
        ticket("c", { assignees: [user("1")], overrideLevel: "S" }),
      ],
      DEFAULT_TICKET_POINTS,
    );
    expect(board.unassigned).toEqual({ tickets: 2, assessed: 1 });
    expect(board.members).toHaveLength(1);
    expect(board.total).toBe(3);
    expect(board.assessed).toBe(2);
  });

  it("sorts members by points desc, then name", () => {
    const board = buildBoard(
      [
        ticket("a", { assignees: [user("1", "Zed")], overrideLevel: "S" }),
        ticket("b", { assignees: [user("2", "Amy")], overrideLevel: "S" }),
        ticket("c", { assignees: [user("3", "Bob")], overrideLevel: "XL" }),
      ],
      DEFAULT_TICKET_POINTS,
    );
    expect(board.members.map((m) => m.name)).toEqual(["Bob", "Amy", "Zed"]);
  });

  it("uses the given points table", () => {
    const board = buildBoard(
      [ticket("a", { assignees: [user("1")], overrideLevel: "XS" })],
      { ...DEFAULT_TICKET_POINTS, XS: 5 },
    );
    expect(board.members[0].points).toBe(5);
  });
});

describe("toAssessCount", () => {
  it("covers p=0, p=100, A above target, N=0", () => {
    expect(toAssessCount(10, 0, 0)).toBe(0);
    expect(toAssessCount(10, 0, 100)).toBe(10);
    expect(toAssessCount(10, 4, 100)).toBe(6);
    expect(toAssessCount(10, 4, 50)).toBe(1);
    expect(toAssessCount(10, 8, 50)).toBe(0);
    expect(toAssessCount(0, 0, 100)).toBe(0);
    expect(toAssessCount(0, 0, 0)).toBe(0);
  });

  it("rounds the target up and clamps the percent", () => {
    expect(toAssessCount(7, 0, 50)).toBe(4);
    expect(toAssessCount(10, 0, 150)).toBe(10);
    expect(toAssessCount(10, 0, -20)).toBe(0);
    expect(toAssessCount(10, 0, Number.NaN)).toBe(0);
  });
});

describe("pickForRun", () => {
  it("interleaves members oldest first", () => {
    const tickets = [
      ticket("a1", {
        assignees: [user("1")],
        doneAt: "2026-10-01T00:00:00Z",
      }),
      ticket("a2", {
        assignees: [user("1")],
        doneAt: "2026-10-03T00:00:00Z",
      }),
      ticket("a3", {
        assignees: [user("1")],
        doneAt: "2026-10-05T00:00:00Z",
      }),
      ticket("b1", {
        assignees: [user("2")],
        doneAt: "2026-10-02T00:00:00Z",
      }),
      ticket("b2", {
        assignees: [user("2")],
        doneAt: "2026-10-04T00:00:00Z",
      }),
    ];
    expect(pickForRun(tickets, 5)).toEqual(["a1", "b1", "a2", "b2", "a3"]);
    expect(pickForRun(tickets, 3)).toEqual(["a1", "b1", "a2"]);
  });

  it("skips tickets that already have an assessment or an override", () => {
    const tickets = [
      ticket("a1", {
        assignees: [user("1")],
        doneAt: "2026-10-01T00:00:00Z",
        assessment: assessment(1),
      }),
      ticket("a2", {
        assignees: [user("1")],
        doneAt: "2026-10-02T00:00:00Z",
        overrideLevel: "S",
      }),
      ticket("a3", {
        assignees: [user("1")],
        doneAt: "2026-10-03T00:00:00Z",
      }),
    ];
    expect(pickForRun(tickets, 10)).toEqual(["a3"]);
  });

  it("takes a multi-assignee ticket once, at its first member's turn", () => {
    const tickets = [
      ticket("shared", {
        assignees: [user("1"), user("2")],
        doneAt: "2026-10-01T00:00:00Z",
      }),
      ticket("a2", {
        assignees: [user("1")],
        doneAt: "2026-10-03T00:00:00Z",
      }),
      ticket("b2", {
        assignees: [user("2")],
        doneAt: "2026-10-04T00:00:00Z",
      }),
    ];
    const picked = pickForRun(tickets, 10);
    expect(picked).toEqual(["shared", "b2", "a2"]);
    expect(new Set(picked).size).toBe(picked.length);
  });

  it("puts unassigned tickets last, oldest first", () => {
    const tickets = [
      ticket("u2", { doneAt: "2026-10-02T00:00:00Z" }),
      ticket("u1", { doneAt: "2026-09-01T00:00:00Z" }),
      ticket("a1", {
        assignees: [user("1")],
        doneAt: "2026-10-05T00:00:00Z",
      }),
    ];
    expect(pickForRun(tickets, 10)).toEqual(["a1", "u1", "u2"]);
    expect(pickForRun(tickets, 2)).toEqual(["a1", "u1"]);
  });

  it("returns nothing for a count of zero", () => {
    expect(pickForRun([ticket("a")], 0)).toEqual([]);
  });
});

describe("periodTickets", () => {
  const scope: WorkspaceScope = {
    workspaceId: "w1",
    name: "Workspace",
    spaceIds: ["s1"],
    excludedStatuses: ["Cancelled"],
    deliveredStatuses: [],
    excludedMembers: [],
  };
  const october: Period = {
    kind: "month",
    id: "2026-10",
    label: "Oct 2026",
    start: new Date(2026, 9, 1),
    end: new Date(2026, 10, 1),
  };
  const inOct = "2026-10-10T12:00:00";

  it("drops other workspaces, other spaces, excluded statuses, other periods", () => {
    const kept = ticket("kept", { doneAt: inOct });
    const result = periodTickets(
      [
        kept,
        ticket("other-ws", { workspaceId: "w2", doneAt: inOct }),
        ticket("other-space", { spaceId: "s2", doneAt: inOct }),
        ticket("cancelled", { status: "cancelled", doneAt: inOct }),
        ticket("september", { doneAt: "2026-09-30T12:00:00" }),
        ticket("november", { doneAt: "2026-11-01T12:00:00" }),
      ],
      scope,
      october,
    );
    expect(result).toEqual([kept]);
  });

  it("keeps a ticket done in the period even if edited after it", () => {
    const t = ticket("late-edit", {
      doneAt: inOct,
      updatedAt: "2026-12-24T09:00:00",
    });
    expect(periodTickets([t], scope, october)).toEqual([t]);
  });

  it("drops a ticket edited in the period but done before it", () => {
    const t = ticket("early", {
      doneAt: "2026-08-01T00:00:00",
      updatedAt: inOct,
    });
    expect(periodTickets([t], scope, october)).toEqual([]);
  });

  it("counts a custom delivered status by statusSince, else updatedAt", () => {
    const chosen = { ...scope, deliveredStatuses: ["ready for testing"] };
    const base = {
      status: "Ready For Testing",
      statusType: "custom",
      doneAt: null,
    };
    const exact = ticket("exact", {
      ...base,
      statusSince: inOct,
      updatedAt: "2026-12-01T00:00:00",
    });
    const approx = ticket("approx", { ...base, updatedAt: inOct });
    const notChosen = ticket("working", {
      ...base,
      status: "in progress",
      updatedAt: inOct,
    });
    expect(
      periodTickets([exact, approx, notChosen], chosen, october).map(
        (t) => t.id,
      ),
    ).toEqual(["exact", "approx"]);
  });

  it("uses done and closed types by default and ignores custom ones", () => {
    const custom = ticket("c", { statusType: "custom", doneAt: null });
    const done = ticket("d", { statusType: "done", doneAt: inOct });
    expect(periodTickets([custom, done], scope, october)).toEqual([done]);
  });

  it("excluded wins over a delivered status", () => {
    const s = {
      ...scope,
      deliveredStatuses: ["cancelled"],
      excludedStatuses: ["Cancelled"],
    };
    expect(
      periodTickets(
        [ticket("x", { status: "cancelled", doneAt: inOct })],
        s,
        october,
      ),
    ).toEqual([]);
  });

  it("drops a parent with subtasks and keeps its subtask", () => {
    const parent = ticket("parent", { doneAt: inOct, hasSubtasks: true });
    const sub = ticket("sub", { doneAt: inOct, parentId: "parent" });
    expect(periodTickets([parent, sub], scope, october)).toEqual([sub]);
  });

  it("never lists active work", () => {
    const active = ticket("a", {
      status: "in progress",
      statusType: "custom",
      doneAt: null,
      updatedAt: inOct,
    });
    expect(periodTickets([active], scope, october)).toEqual([]);
  });
});

describe("active work", () => {
  const scope: WorkspaceScope = {
    workspaceId: "w1",
    name: "Workspace",
    spaceIds: null,
    excludedStatuses: [],
    deliveredStatuses: [],
    excludedMembers: [],
  };
  const october: Period = {
    kind: "month",
    id: "2026-10",
    label: "Oct 2026",
    start: new Date(2026, 9, 1),
    end: new Date(2026, 10, 1),
  };
  const working = (id: string, patch: Partial<TicketRecord> = {}) =>
    ticket(id, {
      status: "in progress",
      statusType: "custom",
      doneAt: null,
      updatedAt: "2026-10-10T12:00:00",
      assignees: [user("1")],
      ...patch,
    });

  it("counts active tickets per assignee by updatedAt in the period", () => {
    const counts = activeByMember(
      [
        working("a"),
        working("b", { assignees: [user("1"), user("2")] }),
        working("old", { updatedAt: "2026-09-10T12:00:00" }),
        working("done", { statusType: "done", doneAt: "2026-10-10T12:00:00" }),
        working("open", { statusType: "open" }),
      ],
      scope,
      october,
    );
    expect([...counts]).toEqual([
      ["1", 2],
      ["2", 1],
    ]);
  });

  it("an active ticket is in activeTickets and not in periodTickets", () => {
    const t = working("a");
    expect(activeTickets([t], scope, october)).toEqual([t]);
    expect(periodTickets([t], scope, october)).toEqual([]);
  });

  it("buildBoard adds the active count and lists active-only members", () => {
    const board = buildBoard(
      [ticket("d", { assignees: [user("1", "Ann")], overrideLevel: "XS" })],
      DEFAULT_TICKET_POINTS,
      [
        working("a", { assignees: [user("1", "Ann")] }),
        working("b", { assignees: [user("2", "Bob")] }),
      ],
    );
    expect(
      board.members.map((m) => [m.id, m.points, m.tickets, m.active]),
    ).toEqual([
      ["1", 3, 1, 1],
      ["2", 0, 0, 1],
    ]);
    expect(board.members[1].name).toBe("Bob");
    expect(board.members[1].ticketList).toEqual([]);
    expect(board.total).toBe(1);
  });

  it("active is 0 when there is no active work", () => {
    const board = buildBoard(
      [ticket("d", { assignees: [user("1")] })],
      DEFAULT_TICKET_POINTS,
    );
    expect(board.members[0].active).toBe(0);
  });
});

describe("datesApproximate", () => {
  it("is set when a counted ticket fell back to updatedAt", () => {
    const exact = ticket("e", { assignees: [user("1")] });
    const approx = ticket("a", {
      assignees: [user("1")],
      statusType: "custom",
      doneAt: null,
    });
    expect(buildBoard([exact], DEFAULT_TICKET_POINTS).datesApproximate).toBe(
      false,
    );
    expect(
      buildBoard([exact, approx], DEFAULT_TICKET_POINTS).datesApproximate,
    ).toBe(true);
    expect(buildBoard([], DEFAULT_TICKET_POINTS).datesApproximate).toBe(false);
  });

  it("is not set by statusSince", () => {
    const t = ticket("s", {
      statusType: "custom",
      doneAt: null,
      statusSince: "2026-10-05T00:00:00Z",
    });
    expect(buildBoard([t], DEFAULT_TICKET_POINTS).datesApproximate).toBe(false);
  });
});

describe("statusesOf", () => {
  it("lists distinct statuses sorted by type, then name", () => {
    const tickets = [
      ticket("1", { status: "Done", statusType: "done" }),
      ticket("2", { status: "done ", statusType: "done" }),
      ticket("3", { status: "ready for testing", statusType: "custom" }),
      ticket("4", { status: "Closed", statusType: "closed" }),
      ticket("5", { status: "backlog", statusType: "open" }),
      ticket("6", { status: "in progress", statusType: "custom" }),
    ];
    expect(statusesOf(tickets)).toEqual([
      { name: "Closed", type: "closed" },
      { name: "in progress", type: "custom" },
      { name: "ready for testing", type: "custom" },
      { name: "Done", type: "done" },
      { name: "backlog", type: "open" },
    ]);
  });

  it("skips blank statuses", () => {
    expect(statusesOf([ticket("1", { status: "  " })])).toEqual([]);
  });
});

describe("ALL_START", () => {
  it("is the fixed start of the all-time window", () => {
    expect(ALL_START).toBe("2015-01-01T00:00:00Z");
  });
});

describe("excluded members", () => {
  const scope: WorkspaceScope = {
    workspaceId: "w1",
    name: "Workspace",
    spaceIds: null,
    excludedStatuses: [],
    deliveredStatuses: [],
    excludedMembers: [{ id: "2", name: "Two" }],
  };
  const october: Period = {
    kind: "month",
    id: "2026-10",
    label: "Oct 2026",
    start: new Date(2026, 9, 1),
    end: new Date(2026, 10, 1),
  };
  const inOct = "2026-10-10T12:00:00";
  const delivered = (id: string, patch: Partial<TicketRecord> = {}) =>
    ticket(id, { doneAt: inOct, ...patch });

  it("splits a shared ticket between the included assignees only", () => {
    const shared = delivered("shared", {
      assignees: [user("1"), user("2")],
      overrideLevel: "S",
    });
    const board = buildBoard(
      periodTickets([shared], scope, october),
      DEFAULT_TICKET_POINTS,
    );
    expect(board.members.map((m) => [m.id, m.points])).toEqual([["1", 8]]);
    expect(board.members[0].ticketList[0]).toMatchObject({
      points: 8,
      share: 8,
    });
    // Without the exclusion the same ticket is split in two.
    const plain = buildBoard(
      periodTickets([shared], { ...scope, excludedMembers: [] }, october),
      DEFAULT_TICKET_POINTS,
    );
    expect(plain.members.map((m) => [m.id, m.points])).toEqual([
      ["1", 4],
      ["2", 4],
    ]);
  });

  it("drops a ticket whose assignees are all excluded from N, coverage and the run", () => {
    const mine = delivered("mine", {
      assignees: [user("1")],
      overrideLevel: "XS",
    });
    const theirs = delivered("theirs", { assignees: [user("2")] });
    const alsoTheirs = delivered("also", {
      assignees: [user("2"), user("2", "Two")],
    });
    const open = delivered("open", { assignees: [user("1")] });
    const shown = periodTickets(
      [mine, theirs, alsoTheirs, open],
      scope,
      october,
    );
    expect(shown.map((t) => t.id)).toEqual(["mine", "open"]);

    const board = buildBoard(shown, DEFAULT_TICKET_POINTS);
    expect(board.total).toBe(2);
    expect(board.assessed).toBe(1);
    expect(board.members.map((m) => m.id)).toEqual(["1"]);
    expect(toAssessCount(board.total, board.assessed, 100)).toBe(1);
    expect(pickForRun(shown, 5)).toEqual(["open"]);
  });

  it("keeps a ticket without assignees as unassigned", () => {
    const shown = periodTickets(
      [delivered("nobody"), delivered("theirs", { assignees: [user("2")] })],
      scope,
      october,
    );
    const board = buildBoard(shown, DEFAULT_TICKET_POINTS);
    expect(shown.map((t) => t.id)).toEqual(["nobody"]);
    expect(board.total).toBe(1);
    expect(board.unassigned).toEqual({ tickets: 1, assessed: 0 });
    expect(pickForRun(shown, 1)).toEqual(["nobody"]);
  });

  it("leaves tickets alone when nobody is excluded from them", () => {
    const t = delivered("a", { assignees: [user("1")] });
    expect(periodTickets([t], scope, october)[0]).toBe(t);
  });

  it("does not count excluded members or their tickets as active", () => {
    const working = (id: string, assignees: TicketAssignee[]) =>
      ticket(id, {
        status: "in progress",
        statusType: "custom",
        doneAt: null,
        updatedAt: inOct,
        assignees,
      });
    const tickets = [
      working("a", [user("1")]),
      working("b", [user("1"), user("2")]),
      working("c", [user("2")]),
    ];
    expect(activeTickets(tickets, scope, october).map((t) => t.id)).toEqual([
      "a",
      "b",
    ]);
    expect([...activeByMember(tickets, scope, october)]).toEqual([["1", 2]]);
    const board = buildBoard(
      [],
      DEFAULT_TICKET_POINTS,
      activeTickets(tickets, scope, october),
    );
    expect(board.members.map((m) => [m.id, m.active])).toEqual([["1", 2]]);
  });

  it("keeps the stored name unless a loaded ticket carries a newer one", () => {
    const tickets = [delivered("a", { assignees: [user("2", "Bea")] })];
    const stored = {
      ...scope,
      excludedMembers: [
        { id: "2", name: "Old Bea" },
        { id: "9", name: "Left Lee" },
      ],
    };
    expect(excludedMembersOf(tickets, stored)).toEqual([
      { id: "2", name: "Bea" },
      { id: "9", name: "Left Lee" },
    ]);
    // Nothing newer: the scope's own array comes back, so there is nothing to save.
    expect(excludedMembersOf([], stored)).toBe(stored.excludedMembers);
  });
});
