import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import { buildContributorsModel } from "./pipeline";
import { sinceFor } from "./range";
import type { CustomRange, RangeValue } from "./range";
import {
  ANA,
  BOB,
  file,
  ident,
  makeCommit,
  makePr,
  makeRaw,
  oct,
  withConfig,
} from "./testFixtures";

const NOW = new Date(2026, 9, 20, 10);

function raw() {
  return makeRaw({
    commits: [
      makeCommit("a1", ANA, oct(5), { files: [file("src/a.ts", 10)] }),
      makeCommit("a2", ident("Ana B", "ana@laptop.local"), oct(6), {
        files: [file("src/b.ts", 5)],
      }),
      makeCommit("b1", BOB, oct(7), { files: [file("src/c.ts", 7)] }),
    ],
    prs: [makePr("1", ANA, oct(8))],
  });
}

describe("buildContributorsModel", () => {
  it("scores the current and the previous season and builds the board and stats", () => {
    const model = buildContributorsModel({
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
    });
    expect(model.season.id).toBe("2026-10");
    expect(model.previousSeason.id).toBe("2026-09");
    expect(model.current.season.id).toBe("2026-10");
    expect(model.previous.season.id).toBe("2026-09");
    expect(model.leaderboard.rows.length).toBeGreaterThan(0);
    expect(model.stats.kpis.commits.value).toBe(3);
    expect(model.hasActivity).toBe(true);
  });

  it("merges identities through config people only when merge is on", () => {
    const config = withConfig({
      people: [
        {
          id: "ana",
          name: "Ana",
          identities: ["ana@example.com", "ana@laptop.local"],
        },
      ],
    });
    const on = buildContributorsModel({
      raw: raw(),
      config,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
    });
    const off = buildContributorsModel({
      raw: raw(),
      config,
      mailmap: [],
      merge: false,
      range: "30d",
      now: NOW,
    });
    expect(on.stats.rows.map((r) => r.name).sort()).toEqual(["Ana", "Bob"]);
    expect(off.stats.rows).toHaveLength(3);
  });

  it("marks the person that owns the git user email as me", () => {
    const model = buildContributorsModel({
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
      user: { name: "Bob", email: "BOB@example.com" },
    });
    expect(model.meId).toBe("bob@example.com");
    expect(model.leaderboard.rows.find((r) => r.isMe)?.name).toBe("Bob");
  });

  it("has no me when the git user was never seen or has no email", () => {
    const base = {
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d" as const,
      now: NOW,
    };
    expect(
      buildContributorsModel({
        ...base,
        user: { name: "Zed", email: "z@x.io" },
      }).meId,
    ).toBeUndefined();
    expect(
      buildContributorsModel({ ...base, user: { name: "", email: "" } }).meId,
    ).toBeUndefined();
    expect(buildContributorsModel(base).meId).toBeUndefined();
  });

  it("does not add the git user to the resolver people when they never committed", () => {
    const model = buildContributorsModel({
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
      user: { name: "Zed", email: "z@x.io" },
    });
    expect(model.resolver.people.some((p) => p.id === "z@x.io")).toBe(false);
  });

  it("reports no activity when nothing falls inside the range", () => {
    const model = buildContributorsModel({
      raw: makeRaw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
    });
    expect(model.hasActivity).toBe(false);
  });

  it("counts only the identities that need attention with the shared selector", () => {
    const model = buildContributorsModel({
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
    });
    // ana@example.com and bob@example.com are ordinary authors; the .local
    // address is a machine.
    expect(model.unmapped.map((u) => u.key)).toEqual(["ana@laptop.local"]);
  });

  it("compares KPIs with the equal-length period before the range and the board with the previous season", () => {
    const data = makeRaw({
      since: sinceFor("7d", NOW),
      commits: [
        makeCommit("old", ANA, new Date(2026, 8, 25, 12).toISOString(), {
          files: [file("src/a.ts", 4)],
        }),
        makeCommit("new1", ANA, oct(14), { files: [file("src/a.ts", 4)] }),
        makeCommit("new2", ANA, oct(15), { files: [file("src/a.ts", 4)] }),
        makeCommit("new3", ANA, oct(16), { files: [file("src/a.ts", 4)] }),
      ],
    });
    const model = buildContributorsModel({
      raw: data,
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "7d",
      now: NOW,
    });
    // 7d = Oct 14-20; the previous 7 days are Oct 7-13 (empty), not Sep 25.
    expect(model.stats.kpis.commits.value).toBe(3);
    expect(model.stats.kpis.commits.delta).toBeNull();
    expect(model.stats.kpis.prsMerged.delta).toBe(0);
    expect(model.previous.season.id).toBe("2026-09");
    expect(model.previous.people.map((p) => p.mainCommits)).toEqual([1]);
  });

  it("shows deltas for the default range with the since the hook really collects", () => {
    const data = makeRaw({
      since: sinceFor("30d", NOW),
      commits: [
        makeCommit("old", ANA, new Date(2026, 8, 5, 9).toISOString(), {
          files: [file("src/a.ts", 4)],
        }),
        makeCommit("new", ANA, oct(14), { files: [file("src/a.ts", 4)] }),
      ],
    });
    const model = buildContributorsModel({
      raw: data,
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
    });
    expect(model.stats.kpis.commits.delta).not.toBeNull();
    expect(model.stats.kpis.prsMerged.delta).not.toBeNull();
  });

  it("merges identities through the mailmap layer", () => {
    const model = buildContributorsModel({
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [
        { name: "Ana", identities: ["ana@example.com", "ana@laptop.local"] },
      ],
      merge: true,
      range: "30d",
      now: NOW,
    });
    expect(model.stats.rows.map((r) => r.name).sort()).toEqual(["Ana", "Bob"]);
    expect(model.unmapped).toEqual([]);
  });

  describe("custom range", () => {
    const range: CustomRange = {
      key: "custom",
      start: "2026-10-06",
      end: "2026-10-15",
    };
    const data = () =>
      makeRaw({
        since: sinceFor(range, NOW),
        commits: [
          makeCommit("before", ANA, oct(3), { files: [file("src/a.ts", 4)] }),
          makeCommit("in1", ANA, oct(6), { files: [file("src/a.ts", 4)] }),
          makeCommit("in2", ANA, oct(15), { files: [file("src/a.ts", 4)] }),
          makeCommit("after", ANA, oct(18), { files: [file("src/a.ts", 4)] }),
        ],
      });
    const build = (chosen: RangeValue = range) =>
      buildContributorsModel({
        raw: data(),
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        range: chosen,
        now: NOW,
      });

    it("shows exactly the chosen days, both inclusive", () => {
      const model = build();
      expect(model.rangeStart).toEqual(new Date(2026, 9, 6));
      expect(model.rangeEnd).toEqual(new Date(2026, 9, 16));
      expect(model.rangeDays).toBe(10);
      expect(model.rangeLabel).toBe("2026-10-06 to 2026-10-15");
      expect(model.stats.kpis.commits.value).toBe(2);
    });

    it("compares with the equal-length period right before the range", () => {
      // Oct 6-15 is 10 days; the previous 10 days are Sep 26 - Oct 5.
      const model = build();
      expect(model.stats.kpis.commits.delta).toBe(1);
    });

    it("keeps the board on the ranked season, whatever the range", () => {
      const model = build();
      expect(model.season.id).toBe("2026-10");
      expect(model.leaderboard.rows[0].name).toBe("Ana");
    });

    it("has no activity for a range with no commits", () => {
      const model = build({
        key: "custom",
        start: "2026-10-09",
        end: "2026-10-12",
      });
      expect(model.hasActivity).toBe(false);
      expect(build().hasActivity).toBe(true);
    });

    it("enforces the 30 day cap on a stored value", () => {
      const model = build({
        key: "custom",
        start: "2026-01-01",
        end: "2026-10-15",
      });
      expect(model.rangeDays).toBe(30);
      expect(model.rangeEnd).toEqual(new Date(2026, 9, 16));
    });

    it("labels a preset by its key", () => {
      const model = build("30d");
      expect(model.rangeLabel).toBe("30d");
      expect(model.rangeDays).toBe(30);
    });
  });

  describe("selected season", () => {
    const sept = (day: number) => new Date(2026, 8, day, 12).toISOString();
    const seasonRaw = () =>
      makeRaw({
        since: new Date(2026, 6, 1).toISOString(),
        commits: [
          makeCommit("s1", BOB, sept(8), { files: [file("src/a.ts", 30)] }),
          makeCommit("s2", BOB, sept(9), { files: [file("src/a.ts", 30)] }),
          makeCommit("o1", ANA, oct(5), { files: [file("src/b.ts", 10)] }),
        ],
      });
    const build = (seasonId?: string) =>
      buildContributorsModel({
        raw: seasonRaw(),
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        range: "30d",
        now: NOW,
        seasonId,
      });

    it("scores the current month when none is selected", () => {
      const model = build();
      expect(model.season.id).toBe("2026-10");
      expect(model.leaderboard.rows.map((r) => r.name)).toEqual(["Ana"]);
    });

    it("re-scores an older season from the same raw data", () => {
      const model = build("2026-09");
      expect(model.season.id).toBe("2026-09");
      expect(model.previousSeason.id).toBe("2026-08");
      expect(model.current.season.id).toBe("2026-09");
      expect(model.leaderboard.rows.map((r) => r.name)).toEqual(["Bob"]);
    });

    it("compares with the season before the selected one", () => {
      const model = build("2026-10");
      expect(model.previousSeason.id).toBe("2026-09");
      expect(model.leaderboard.rows[0].movement).toBeNull();
    });

    it("falls back to the current month for an unknown id", () => {
      expect(build("nope").season.id).toBe("2026-10");
    });

    it("keeps the stats range tied to now, not to the season", () => {
      const a = build();
      const b = build("2026-09");
      expect(b.rangeStart).toEqual(a.rangeStart);
      expect(b.rangeEnd).toEqual(a.rangeEnd);
    });
  });
  describe("person colors", () => {
    const people = Array.from({ length: 8 }, (_, i) =>
      ident(`P${i}`, `person${i}@corp.com`),
    );
    const manyRaw = () =>
      makeRaw({
        since: new Date(2026, 7, 1).toISOString(),
        commits: people.flatMap((who, i) => [
          makeCommit(`o${i}`, who, oct(5 + (i % 5)), {
            files: [file("src/a.ts", 3)],
          }),
          makeCommit(
            `s${i}`,
            who,
            new Date(2026, 8, 3 + (i % 5), 12).toISOString(),
            { files: [file("src/a.ts", 3)] },
          ),
        ]),
      });
    const build = (
      extra: Partial<Parameters<typeof buildContributorsModel>[0]> = {},
    ) =>
      buildContributorsModel({
        raw: manyRaw(),
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        range: "30d",
        now: NOW,
        ...extra,
      });

    it("gives every person a color and no two of eight people share one", () => {
      const model = build();
      const ids = model.leaderboard.rows.map((r) => r.personId);
      expect(ids).toHaveLength(8);
      for (const id of ids) expect(model.colors[id]).toBeDefined();
      expect(new Set(ids.map((id) => model.colors[id])).size).toBe(8);
    });

    it("uses the same colors for the stats rows, the legend and the board", () => {
      const model = build();
      for (const row of model.stats.rows) {
        expect(row.color).toBe(model.colors[row.personId]);
      }
      for (const entry of model.stats.legend) {
        expect(entry.color).toBe(model.colors[entry.personId]);
      }
    });

    it("keeps a person's color across ranges, seasons and merge", () => {
      const base = build().colors;
      expect(build({ range: "7d" }).colors).toEqual(base);
      expect(build({ seasonId: "2026-09" }).colors).toEqual(base);
    });

    it("keeps colors of people outside the range distinct too", () => {
      const model = build({ range: "7d" });
      const ids = Object.keys(model.colors);
      expect(new Set(ids.map((id) => model.colors[id])).size).toBe(ids.length);
    });
  });

  it("knows the current season from now, whatever season is ranked", () => {
    const model = buildContributorsModel({
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      range: "30d",
      now: NOW,
      seasonId: "2026-09",
    });
    expect(model.season.id).toBe("2026-09");
    expect(model.currentSeasonId).toBe("2026-10");
  });
});
