import { describe, expect, it } from "vitest";
import {
  type Criteria,
  type Level,
  type TicketRecord,
  type WorkspaceScope,
  deliveredAt,
  effectiveLevel,
  includedAssignees,
  inScope,
  isActive,
  isDelivered,
  levelOf,
} from "./tickets";

const criteriaWithSum = (sum: number): Criteria => {
  const keys = [
    "ambiguity",
    "investigation",
    "scope",
    "risk",
    "coordination",
  ] as const;
  const out: Criteria = {
    ambiguity: 0,
    investigation: 0,
    scope: 0,
    risk: 0,
    coordination: 0,
  };
  let left = sum;
  for (const key of keys) {
    const take = Math.min(2, left);
    out[key] = take;
    left -= take;
  }
  return out;
};

const ticket = (over: Partial<TicketRecord> = {}): TicketRecord => ({
  source: "clickup",
  id: "t1",
  title: "A ticket",
  url: "https://app.clickup.com/t/t1",
  workspaceId: "9001",
  spaceId: "s1",
  status: "done",
  statusType: "done",
  updatedAt: "2026-10-01T00:00:00Z",
  doneAt: "2026-10-01T00:00:00Z",
  statusSince: null,
  parentId: null,
  hasSubtasks: false,
  assignees: [],
  tags: [],
  assessment: null,
  overrideLevel: null,
  overrideNote: null,
  ...over,
});

const scope = (over: Partial<WorkspaceScope> = {}): WorkspaceScope => ({
  workspaceId: "9001",
  name: "Acme",
  spaceIds: null,
  excludedStatuses: [],
  deliveredStatuses: [],
  excludedMembers: [],
  ...over,
});

describe("levelOf", () => {
  it("maps sums to levels at every boundary", () => {
    const expected: [number, Level][] = [
      [0, "XS"],
      [1, "XS"],
      [2, "S"],
      [3, "S"],
      [4, "M"],
      [5, "M"],
      [6, "L"],
      [7, "L"],
      [8, "XL"],
      [10, "XL"],
    ];
    for (const [sum, level] of expected) {
      expect(levelOf(criteriaWithSum(sum)), `sum ${sum}`).toBe(level);
    }
  });
});

describe("effectiveLevel", () => {
  it("prefers the override, then the assessment, else null", () => {
    const assessment = {
      criteria: criteriaWithSum(6),
      reasons: {
        ambiguity: "",
        investigation: "",
        scope: "",
        risk: "",
        coordination: "",
      },
      rubricVersion: 1,
      model: "m",
      runner: "api" as const,
      assessedAt: "2026-10-01T00:00:00Z",
    };
    expect(effectiveLevel(ticket())).toBeNull();
    expect(effectiveLevel(ticket({ assessment }))).toBe("L");
    expect(effectiveLevel(ticket({ assessment, overrideLevel: "XS" }))).toBe(
      "XS",
    );
    expect(effectiveLevel(ticket({ overrideLevel: "M" }))).toBe("M");
  });
});

describe("inScope", () => {
  it("rejects a ticket from another workspace", () => {
    expect(inScope(ticket({ workspaceId: "other" }), scope())).toBe(false);
    expect(inScope(ticket(), scope())).toBe(true);
  });

  it("honours spaceIds when set and ignores them when null", () => {
    expect(inScope(ticket({ spaceId: "s2" }), scope({ spaceIds: null }))).toBe(
      true,
    );
    expect(
      inScope(ticket({ spaceId: "s1" }), scope({ spaceIds: ["s1"] })),
    ).toBe(true);
    expect(
      inScope(ticket({ spaceId: "s2" }), scope({ spaceIds: ["s1"] })),
    ).toBe(false);
    expect(inScope(ticket({ spaceId: "s1" }), scope({ spaceIds: [] }))).toBe(
      false,
    );
  });

  it("rejects excluded statuses case-insensitively", () => {
    const s = scope({ excludedStatuses: ["cancelled"] });
    expect(inScope(ticket({ status: "Cancelled" }), s)).toBe(false);
    expect(inScope(ticket({ status: " CANCELLED " }), s)).toBe(false);
    expect(inScope(ticket({ status: "done" }), s)).toBe(true);
  });

  it("does not look at parents: subtasks are not this function's job", () => {
    expect(inScope(ticket({ id: "s", parentId: "p" }), scope())).toBe(true);
    expect(inScope(ticket({ hasSubtasks: true }), scope())).toBe(true);
  });
});

describe("deliveredAt", () => {
  it("prefers the done date of a done or closed ticket over later edits", () => {
    const t = ticket({
      doneAt: "2026-09-20T00:00:00Z",
      statusSince: "2026-09-25T00:00:00Z",
      updatedAt: "2026-10-30T00:00:00Z",
    });
    expect(deliveredAt(t)).toEqual({
      at: "2026-09-20T00:00:00Z",
      approximate: false,
    });
    expect(deliveredAt({ ...t, statusType: "closed" }).at).toBe(
      "2026-09-20T00:00:00Z",
    );
  });

  it("uses statusSince for a custom status, exact", () => {
    const t = ticket({
      status: "ready for testing",
      statusType: "custom",
      doneAt: null,
      statusSince: "2026-09-25T00:00:00Z",
      updatedAt: "2026-10-30T00:00:00Z",
    });
    expect(deliveredAt(t)).toEqual({
      at: "2026-09-25T00:00:00Z",
      approximate: false,
    });
  });

  it("falls back to updatedAt, flagged approximate", () => {
    const custom = ticket({
      statusType: "custom",
      doneAt: null,
      statusSince: null,
      updatedAt: "2026-10-30T00:00:00Z",
    });
    expect(deliveredAt(custom)).toEqual({
      at: "2026-10-30T00:00:00Z",
      approximate: true,
    });
    // A done ticket without a done date is no better.
    expect(deliveredAt({ ...custom, statusType: "done" }).approximate).toBe(
      true,
    );
  });

  it("ignores a stray done date on a custom status", () => {
    const t = ticket({
      statusType: "custom",
      doneAt: "2026-01-01T00:00:00Z",
      statusSince: null,
    });
    expect(deliveredAt(t).at).toBe(t.updatedAt);
  });
});

describe("isDelivered", () => {
  it("defaults to every done or closed status type", () => {
    expect(isDelivered(ticket({ statusType: "done" }), scope())).toBe(true);
    expect(isDelivered(ticket({ statusType: "closed" }), scope())).toBe(true);
    expect(isDelivered(ticket({ statusType: "custom" }), scope())).toBe(false);
    expect(isDelivered(ticket({ statusType: "open" }), scope())).toBe(false);
  });

  it("adds the chosen names to the done and closed types, case-insensitively", () => {
    const s = scope({ deliveredStatuses: [" Ready For Testing "] });
    const custom = ticket({
      status: "ready for testing",
      statusType: "custom",
    });
    expect(isDelivered(custom, s)).toBe(true);
    // The default still counts next to the chosen names.
    expect(isDelivered(ticket({ status: "done" }), s)).toBe(true);
    expect(isDelivered(custom, scope({ deliveredStatuses: [] }))).toBe(false);
  });

  it("counts a done status the checklist never listed", () => {
    // Edited while only "ready for testing" was known; "brand new" shows up later.
    const s = scope({ deliveredStatuses: ["ready for testing"] });
    expect(
      isDelivered(ticket({ status: "brand new", statusType: "closed" }), s),
    ).toBe(true);
  });

  it("lets an excluded status win over a delivered one", () => {
    const s = scope({
      deliveredStatuses: ["ready for testing"],
      excludedStatuses: ["READY FOR TESTING"],
    });
    expect(
      isDelivered(
        ticket({ status: "ready for testing", statusType: "custom" }),
        s,
      ),
    ).toBe(false);
    expect(isDelivered(ticket(), scope({ excludedStatuses: ["done"] }))).toBe(
      false,
    );
  });

  it("never counts a parent with subtasks, but counts the subtask", () => {
    expect(isDelivered(ticket({ hasSubtasks: true }), scope())).toBe(false);
    expect(isDelivered(ticket({ parentId: "p" }), scope())).toBe(true);
  });

  it("respects the workspace and spaces", () => {
    expect(isDelivered(ticket({ workspaceId: "other" }), scope())).toBe(false);
    expect(
      isDelivered(ticket({ spaceId: "s2" }), scope({ spaceIds: ["s1"] })),
    ).toBe(false);
  });
});

describe("isActive", () => {
  const working = (over: Partial<TicketRecord> = {}) =>
    ticket({ status: "in progress", statusType: "custom", ...over });

  it("is a custom status that is not delivered", () => {
    expect(isActive(working(), scope())).toBe(true);
    expect(isActive(ticket({ statusType: "open" }), scope())).toBe(false);
    expect(isActive(ticket({ statusType: "done" }), scope())).toBe(false);
  });

  it("is not active once the status is chosen as delivered", () => {
    const s = scope({ deliveredStatuses: ["in progress"] });
    expect(isActive(working(), s)).toBe(false);
    expect(isDelivered(working(), s)).toBe(true);
  });

  it("a done status stays delivered, so it is not active, next to chosen names", () => {
    const s = scope({ deliveredStatuses: ["ready for testing"] });
    expect(isDelivered(ticket(), s)).toBe(true);
    expect(isActive(ticket(), s)).toBe(false);
  });

  it("an excluded done status is neither delivered nor active", () => {
    const s = scope({ excludedStatuses: ["done"] });
    expect(isDelivered(ticket(), s)).toBe(false);
    expect(isActive(ticket(), s)).toBe(false);
  });

  it("is not active when excluded, out of scope, or a parent with subtasks", () => {
    expect(
      isActive(working(), scope({ excludedStatuses: ["In Progress"] })),
    ).toBe(false);
    expect(isActive(working({ workspaceId: "other" }), scope())).toBe(false);
    expect(isActive(working({ hasSubtasks: true }), scope())).toBe(false);
    expect(isActive(working({ parentId: "p" }), scope())).toBe(true);
  });
});

describe("excluded members", () => {
  const person = (id: string) => ({ id, name: `User ${id}`, email: "" });
  const s = scope({ excludedMembers: [{ id: "2", name: "Two" }] });
  const working = (over: Partial<TicketRecord> = {}) =>
    ticket({ status: "in progress", statusType: "custom", ...over });

  it("includedAssignees drops the excluded members", () => {
    const t = ticket({ assignees: [person("1"), person("2")] });
    expect(includedAssignees(t, s).map((a) => a.id)).toEqual(["1"]);
    expect(includedAssignees(t, scope())).toBe(t.assignees);
  });

  it("a ticket whose assignees are all excluded is out of scope", () => {
    expect(inScope(ticket({ assignees: [person("2")] }), s)).toBe(false);
    expect(isDelivered(ticket({ assignees: [person("2")] }), s)).toBe(false);
    expect(isActive(working({ assignees: [person("2")] }), s)).toBe(false);
  });

  it("a ticket shared with an included member, or with nobody, stays", () => {
    expect(
      isDelivered(ticket({ assignees: [person("1"), person("2")] }), s),
    ).toBe(true);
    expect(isDelivered(ticket({ assignees: [] }), s)).toBe(true);
    expect(isActive(working({ assignees: [] }), s)).toBe(true);
  });
});
