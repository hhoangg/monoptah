// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import {
  RANGE_KEY,
  SETTINGS_KEY,
  readRange,
  readSettings,
  writeRange,
  writeSettings,
} from "./prefs";
import { DEFAULT_TICKET_POINTS } from "./tickets";

const NOW = new Date(2026, 9, 15, 12);

beforeEach(() => {
  localStorage.clear();
});

describe("ticket contributors settings", () => {
  it("uses the monocode prefix keys", () => {
    expect(SETTINGS_KEY).toBe("monocode.ticketContributors.settings");
    expect(RANGE_KEY).toBe("monocode.ticketContributors.range");
  });

  it("defaults when nothing is saved", () => {
    expect(readSettings()).toEqual({
      scope: null,
      points: DEFAULT_TICKET_POINTS,
      percent: 100,
    });
  });

  it("round trips", () => {
    const saved = {
      scope: {
        workspaceId: "w1",
        name: "Workspace",
        spaceIds: ["s1", "s2"],
        excludedStatuses: ["cancelled"],
        deliveredStatuses: ["ready for testing"],
        excludedMembers: [
          { id: "u1", name: "Una" },
          { id: "u2", name: "Uri" },
        ],
      },
      points: { XS: 1, S: 2, M: 4, L: 8, XL: 16 },
      percent: 40,
    };
    writeSettings(saved);
    expect(readSettings()).toEqual(saved);
  });

  it("falls back to defaults on malformed JSON", () => {
    for (const bad of ["{not json", "null", "[]", '"text"', "42"]) {
      localStorage.setItem(SETTINGS_KEY, bad);
      expect(readSettings()).toEqual({
        scope: null,
        points: DEFAULT_TICKET_POINTS,
        percent: 100,
      });
    }
  });

  it("falls back per field when a field is invalid", () => {
    localStorage.setItem(
      SETTINGS_KEY,
      JSON.stringify({
        scope: { workspaceId: 7 },
        points: { XS: "x", S: -1, M: 20 },
        percent: "lots",
      }),
    );
    expect(readSettings()).toEqual({
      scope: null,
      points: { ...DEFAULT_TICKET_POINTS, M: 20 },
      percent: 100,
    });
  });

  it("clamps the saved percent into 0..100", () => {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify({ percent: 250 }));
    expect(readSettings().percent).toBe(100);
    localStorage.setItem(SETTINGS_KEY, JSON.stringify({ percent: -5 }));
    expect(readSettings().percent).toBe(0);
  });

  it("reads empty or all blank spaceIds as null", () => {
    for (const spaceIds of [[], ["", "  "]]) {
      localStorage.setItem(
        SETTINGS_KEY,
        JSON.stringify({
          scope: {
            workspaceId: "w1",
            name: "Workspace",
            spaceIds,
            excludedStatuses: [],
          },
        }),
      );
      expect(readSettings().scope).toEqual({
        workspaceId: "w1",
        name: "Workspace",
        spaceIds: null,
        excludedStatuses: [],
        deliveredStatuses: [],
        excludedMembers: [],
      });
    }
  });

  it("keeps the non-blank space ids", () => {
    localStorage.setItem(
      SETTINGS_KEY,
      JSON.stringify({
        scope: {
          workspaceId: "w1",
          name: "Workspace",
          spaceIds: [" s1 ", "", "s2"],
          excludedStatuses: ["x"],
        },
      }),
    );
    expect(readSettings().scope?.spaceIds).toEqual(["s1", "s2"]);
  });
});

describe("delivered statuses", () => {
  const save = (extra: Record<string, unknown>) =>
    localStorage.setItem(
      SETTINGS_KEY,
      JSON.stringify({
        scope: {
          workspaceId: "w1",
          name: "Workspace",
          spaceIds: null,
          excludedStatuses: [],
          ...extra,
        },
      }),
    );

  it("reads an old scope without deliveredStatuses as an empty list", () => {
    save({});
    expect(readSettings().scope?.deliveredStatuses).toEqual([]);
  });

  it("migrates a stored null to an empty list", () => {
    save({ deliveredStatuses: null });
    expect(readSettings().scope?.deliveredStatuses).toEqual([]);
  });

  it("reads a non-array value as an empty list", () => {
    for (const bad of ["done", 7, {}, true]) {
      save({ deliveredStatuses: bad });
      expect(readSettings().scope?.deliveredStatuses).toEqual([]);
    }
  });

  it("keeps a saved list, dropping non-strings, and an empty list stays empty", () => {
    save({ deliveredStatuses: ["a", 3, "b"] });
    expect(readSettings().scope?.deliveredStatuses).toEqual(["a", "b"]);
    save({ deliveredStatuses: [] });
    expect(readSettings().scope?.deliveredStatuses).toEqual([]);
  });
});

describe("excluded members", () => {
  const save = (extra: Record<string, unknown>) =>
    localStorage.setItem(
      SETTINGS_KEY,
      JSON.stringify({
        scope: {
          workspaceId: "w1",
          name: "Workspace",
          spaceIds: null,
          excludedStatuses: [],
          deliveredStatuses: [],
          ...extra,
        },
      }),
    );

  it("reads an old scope without excludedMembers as an empty list", () => {
    save({});
    expect(readSettings().scope?.excludedMembers).toEqual([]);
  });

  it("migrates a stored null and non-array values to an empty list", () => {
    for (const bad of [null, "u1", 7, {}, true]) {
      save({ excludedMembers: bad });
      expect(readSettings().scope?.excludedMembers).toEqual([]);
    }
  });

  it("reads old bare ids as members named by their id, dropping junk", () => {
    save({ excludedMembers: ["u1", 3, "u2", "u1", "", null] });
    expect(readSettings().scope?.excludedMembers).toEqual([
      { id: "u1", name: "u1" },
      { id: "u2", name: "u2" },
    ]);
  });

  it("keeps stored names and falls back to the id for a blank or missing one", () => {
    save({
      excludedMembers: [
        { id: "u1", name: " Una " },
        { id: "u2", name: "" },
        { id: "u3" },
        { name: "No id" },
      ],
    });
    expect(readSettings().scope?.excludedMembers).toEqual([
      { id: "u1", name: "Una" },
      { id: "u2", name: "u2" },
      { id: "u3", name: "u3" },
    ]);
  });
});

describe("ticket contributors range", () => {
  it("defaults to the current month", () => {
    expect(readRange(NOW)).toEqual({ kind: "month", id: null });
  });

  it("round trips a kind and a pinned period", () => {
    for (const pick of [
      { kind: "month", id: null },
      { kind: "month", id: "2026-08" },
      { kind: "quarter", id: "2026-Q2" },
      { kind: "year", id: "2025" },
      { kind: "all", id: null },
    ] as const) {
      writeRange(pick);
      expect(readRange(NOW)).toEqual(pick);
    }
  });

  it("reads a saved 7d as month", () => {
    localStorage.setItem(RANGE_KEY, "7d");
    expect(readRange(NOW)).toEqual({ kind: "month", id: null });
  });

  it("falls back to month on garbage", () => {
    localStorage.setItem(RANGE_KEY, "fortnight");
    expect(readRange(NOW)).toEqual({ kind: "month", id: null });
  });
});
