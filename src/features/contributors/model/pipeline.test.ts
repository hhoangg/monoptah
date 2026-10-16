import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import { buildContributorsModel } from "./pipeline";
import { currentPeriod, syncWindow } from "./period";
import type { Period } from "./period";
import {
  ANA,
  BOB,
  file,
  ident,
  makeCommit,
  makePr,
  makeRaw,
  oct,
  priorOf,
  withConfig,
} from "./testFixtures";

const NOW = new Date(2026, 9, 20, 10);
const MONTH = currentPeriod("month", NOW);
/** The since the hook collects for a period. */
const sinceOf = (period: Period) => syncWindow(period, NOW).from.toISOString();

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
  it("scores the period and the one before it and builds the board and stats", () => {
    const model = buildContributorsModel({
      raw: raw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      period: MONTH,
      now: NOW,
    });
    expect(model.period.id).toBe("2026-10");
    expect(model.previousPeriod.id).toBe("2026-09");
    expect(model.current.period.id).toBe("2026-10");
    expect(model.previous.period.id).toBe("2026-09");
    expect(model.leaderboard.rows.length).toBeGreaterThan(0);
    expect(model.stats.kpis.commits.value).toBe(3);
    expect(model.hasActivity).toBe(true);
  });

  describe("All time", () => {
    const FIRST = new Date(2026, 7, 3);
    const all = () => currentPeriod("all", NOW, FIRST);
    const data = () =>
      makeRaw({
        since: syncWindow(all(), NOW).from.toISOString(),
        commits: [
          makeCommit("old", ANA, new Date(2026, 7, 3, 9).toISOString(), {
            files: [file("src/a.ts", 4)],
          }),
          makeCommit("new", BOB, oct(14), { files: [file("src/b.ts", 6)] }),
        ],
        prs: [makePr("1", ANA, oct(8))],
      });
    const build = () =>
      buildContributorsModel({
        raw: data(),
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        period: all(),
        now: NOW,
      });

    it("has no previous period or scores, and compares nothing", () => {
      const model = build();
      expect(model.period.id).toBe("all");
      expect(model.previousPeriod).toBeNull();
      expect(model.previous).toBeNull();
      const { kpis } = model.stats;
      expect(kpis.commits.value).toBe(2);
      expect(kpis.commits.delta).toBeNull();
      expect(kpis.lines.delta).toBeNull();
      expect(kpis.prsMerged.value).toBe(1);
      expect(kpis.prsMerged.delta).toBeNull();
      expect(kpis.contributors.newcomers).toBe(0);
    });

    it("ranks everyone without a rank movement or a newcomer badge", () => {
      const { rows } = build().leaderboard;
      expect(rows.length).toBe(2);
      for (const row of rows) {
        expect(row.movement).toBeNull();
        expect(row.badges.map((b) => b.id)).not.toContain("newcomer");
      }
    });

    it("buckets the chart by month and counts the whole history", () => {
      const model = build();
      expect(model.stats.granularity).toBe("month");
      expect(model.stats.buckets.map((b) => b.start)).toEqual([
        "2026-08-01",
        "2026-09-01",
        "2026-10-01",
      ]);
      expect(model.hasActivity).toBe(true);
    });

    it("still gives a period without activity an empty state", () => {
      const model = buildContributorsModel({
        raw: makeRaw({ since: syncWindow(all(), NOW).from.toISOString() }),
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        period: all(),
        now: NOW,
      });
      expect(model.hasActivity).toBe(false);
    });
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
      period: MONTH,
      now: NOW,
    });
    const off = buildContributorsModel({
      raw: raw(),
      config,
      mailmap: [],
      merge: false,
      period: MONTH,
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
      period: MONTH,
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
      period: MONTH,
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
      period: MONTH,
      now: NOW,
      user: { name: "Zed", email: "z@x.io" },
    });
    expect(model.resolver.people.some((p) => p.id === "z@x.io")).toBe(false);
  });

  it("reports no activity when nothing falls inside the period", () => {
    const model = buildContributorsModel({
      raw: makeRaw(),
      config: DEFAULT_CONFIG,
      mailmap: [],
      merge: true,
      period: MONTH,
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
      period: MONTH,
      now: NOW,
    });
    // ana@example.com and bob@example.com are ordinary authors; the .local
    // address is a machine.
    expect(model.unmapped.map((u) => u.key)).toEqual(["ana@laptop.local"]);
  });

  it("compares KPIs and the board with the previous period of the same kind", () => {
    const week = currentPeriod("7d", NOW);
    const data = makeRaw({
      since: sinceOf(week),
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
      period: week,
      now: NOW,
    });
    // 7d = Oct 14-20; the previous 7 days are Oct 7-13 (empty), not Sep 25.
    expect(model.stats.kpis.commits.value).toBe(3);
    expect(model.stats.kpis.commits.delta).toBeNull();
    expect(model.stats.kpis.prsMerged.delta).toBe(0);
    expect(model.period.id).toBe("7d");
    expect(model.previousPeriod.start).toEqual(new Date(2026, 9, 7));
    expect(model.previous.period.start).toEqual(new Date(2026, 9, 7));
    expect(model.previous.people).toEqual([]);
  });

  it("shows deltas for a month with the since the hook really collects", () => {
    const data = makeRaw({
      since: sinceOf(MONTH),
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
      period: MONTH,
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
      period: MONTH,
      now: NOW,
    });
    expect(model.stats.rows.map((r) => r.name).sort()).toEqual(["Ana", "Bob"]);
    expect(model.unmapped).toEqual([]);
  });

  describe("periods", () => {
    const sept = (day: number) => new Date(2026, 8, day, 12).toISOString();
    const periodRaw = () =>
      makeRaw({
        since: new Date(2025, 0, 1).toISOString(),
        commits: [
          makeCommit("y1", ANA, new Date(2025, 5, 3, 12).toISOString(), {
            files: [file("src/a.ts", 10)],
          }),
          makeCommit("q1", ANA, new Date(2026, 5, 3, 12).toISOString(), {
            files: [file("src/a.ts", 10)],
          }),
          makeCommit("s1", BOB, sept(8), { files: [file("src/a.ts", 30)] }),
          makeCommit("s2", BOB, sept(9), { files: [file("src/a.ts", 30)] }),
          makeCommit("o1", ANA, oct(5), { files: [file("src/b.ts", 10)] }),
          makeCommit("o2", ANA, oct(18), { files: [file("src/b.ts", 10)] }),
        ],
      });
    const build = (period: Period) =>
      buildContributorsModel({
        raw: periodRaw(),
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        period,
        now: NOW,
      });

    it("ranks and shows the stats of the same month", () => {
      const model = build(MONTH);
      expect(model.period.id).toBe("2026-10");
      expect(model.leaderboard.rows.map((r) => r.name)).toEqual(["Ana"]);
      expect(model.stats.kpis.commits.value).toBe(2);
      expect(model.stats.granularity).toBe("day");
    });

    it("re-scores an older month from the same raw data", () => {
      const model = build(priorOf(MONTH));
      expect(model.period.id).toBe("2026-09");
      expect(model.previousPeriod.id).toBe("2026-08");
      expect(model.current.period.id).toBe("2026-09");
      expect(model.leaderboard.rows.map((r) => r.name)).toEqual(["Bob"]);
      expect(model.stats.kpis.commits.value).toBe(2);
    });

    it("moves rank against the previous period of the same kind", () => {
      const model = build(MONTH);
      expect(model.previousPeriod.id).toBe("2026-09");
      expect(model.leaderboard.rows[0].movement).toBeNull();
    });

    it("covers a quarter with weekly buckets and a year with monthly ones", () => {
      const quarter = build(currentPeriod("quarter", NOW));
      expect(quarter.period.id).toBe("2026-Q4");
      expect(quarter.stats.granularity).toBe("week");
      expect(quarter.stats.kpis.commits.value).toBe(2);
      const year = build(currentPeriod("year", NOW));
      expect(year.period.id).toBe("2026");
      expect(year.stats.granularity).toBe("month");
      expect(year.stats.kpis.commits.value).toBe(5);
      expect(year.stats.buckets.map((b) => b.start)).toEqual([
        "2026-01-01",
        "2026-02-01",
        "2026-03-01",
        "2026-04-01",
        "2026-05-01",
        "2026-06-01",
        "2026-07-01",
        "2026-08-01",
        "2026-09-01",
        "2026-10-01",
      ]);
    });

    it("compares a partly elapsed month with the same days of the previous one", () => {
      const day3 = new Date(2026, 9, 3, 10);
      const at = (m: number, d: number) =>
        new Date(2026, m, d, 12).toISOString();
      const data = makeRaw({
        since: new Date(2026, 8, 1).toISOString(),
        commits: [
          makeCommit("s1", ANA, at(8, 1), { files: [file("src/a.ts", 4)] }),
          makeCommit("s2", ANA, at(8, 2), { files: [file("src/a.ts", 4)] }),
          makeCommit("s3", ANA, at(8, 3), { files: [file("src/a.ts", 4)] }),
          makeCommit("s20", ANA, at(8, 20), { files: [file("src/a.ts", 4)] }),
          makeCommit("s21", ANA, at(8, 21), { files: [file("src/a.ts", 4)] }),
          makeCommit("o1", ANA, at(9, 1), { files: [file("src/a.ts", 4)] }),
          makeCommit("o2", ANA, at(9, 2), { files: [file("src/a.ts", 4)] }),
          makeCommit("o3", ANA, at(9, 3), { files: [file("src/a.ts", 4)] }),
        ],
      });
      const model = buildContributorsModel({
        raw: data,
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        period: currentPeriod("month", day3),
        now: day3,
      });
      // 3 commits so far against 3 in Sep 1-3 (not 5 in all of September).
      expect(model.stats.kpis.commits.value).toBe(3);
      expect(model.stats.kpis.commits.delta).toBe(0);
      // Rank movement still scores the whole previous month.
      expect(model.previous.period.id).toBe("2026-09");
      expect(model.previous.people[0].activeDays).toBe(5);
    });

    it("compares a complete past quarter with the whole previous quarter", () => {
      const at = (m: number, d: number) =>
        new Date(2026, m, d, 12).toISOString();
      const data = makeRaw({
        since: new Date(2026, 0, 1).toISOString(),
        commits: [
          makeCommit("q1a", ANA, at(0, 5), { files: [file("src/a.ts", 4)] }),
          makeCommit("q1b", ANA, at(2, 28), { files: [file("src/a.ts", 4)] }),
          makeCommit("q2a", ANA, at(3, 2), { files: [file("src/a.ts", 4)] }),
          makeCommit("q2b", ANA, at(4, 2), { files: [file("src/a.ts", 4)] }),
          makeCommit("q2c", ANA, at(5, 28), { files: [file("src/a.ts", 4)] }),
          makeCommit("q2d", ANA, at(5, 29), { files: [file("src/a.ts", 4)] }),
        ],
      });
      const q2 = priorOf(priorOf(currentPeriod("quarter", NOW)));
      expect(q2.id).toBe("2026-Q2");
      const model = buildContributorsModel({
        raw: data,
        config: DEFAULT_CONFIG,
        mailmap: [],
        merge: true,
        period: q2,
        now: NOW,
      });
      expect(model.stats.kpis.commits.value).toBe(4);
      expect(model.stats.kpis.commits.delta).toBe(1); // 4 vs the 2 of Q1
    });

    it("compares the year with the year before", () => {
      const year = build(currentPeriod("year", NOW));
      expect(year.previousPeriod.id).toBe("2025");
      expect(year.stats.kpis.commits.delta).toBe(4);
    });

    it("stops the chart at today for the current period", () => {
      const week = build(currentPeriod("7d", NOW));
      expect(week.stats.buckets).toHaveLength(7);
      expect(week.stats.buckets[6].start).toBe("2026-10-20");
      const month = build(MONTH);
      expect(month.stats.buckets).toHaveLength(20);
    });

    it("has no activity for a period with no commits", () => {
      expect(build(currentPeriod("7d", NOW)).hasActivity).toBe(true);
      const empty = build(priorOf(priorOf(MONTH)));
      expect(empty.hasActivity).toBe(false);
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
        period: MONTH,
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

    it("keeps a person's color across periods", () => {
      const base = build().colors;
      expect(build({ period: currentPeriod("7d", NOW) }).colors).toEqual(base);
      expect(build({ period: priorOf(MONTH) }).colors).toEqual(base);
    });

    it("keeps colors of people outside the period distinct too", () => {
      const model = build({ period: currentPeriod("7d", NOW) });
      const ids = Object.keys(model.colors);
      expect(new Set(ids.map((id) => model.colors[id])).size).toBe(ids.length);
    });
  });
});
