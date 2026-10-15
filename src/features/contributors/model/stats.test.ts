import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import { colorForPerson } from "./palette";
import { bucketGranularity, buildStats } from "./stats";
import { currentPeriod } from "./period";
import {
  ANA,
  BOB,
  CAT,
  CLAUDE,
  DEPENDABOT,
  GITHUB,
  file,
  ident,
  makeCommit,
  makePr,
  makeRaw,
  makeResolver,
  oct,
  withConfig,
} from "./testFixtures";
import type { ContributorsConfig, RawData } from "./types";

const sep = (day: number) => new Date(2026, 8, day, 12).toISOString();
const RANGE = { start: new Date(2026, 9, 5), end: new Date(2026, 9, 19) };
const ANA_LAPTOP = ident("Ana", "ana@laptop.local");

const config: ContributorsConfig = withConfig({
  people: [
    {
      id: "p-ana",
      name: "Ana",
      identities: ["ana@example.com", "ana@laptop.local"],
    },
  ],
});

function stats(
  raw: RawData,
  extra: { previous?: { start: Date; end: Date } } = {},
  cfg: ContributorsConfig = config,
) {
  return buildStats({
    raw,
    config: cfg,
    resolver: makeResolver(raw, cfg),
    rangeStart: RANGE.start,
    rangeEnd: RANGE.end,
    ...extra,
  });
}

const sample = () =>
  makeRaw({
    commits: [
      makeCommit("c1", ANA, oct(6), {
        files: [file("src/a.ts", 10, 2), file("Cargo.lock", 500)],
      }),
      makeCommit("c2", ANA_LAPTOP, oct(13), {
        coAuthors: [CLAUDE],
        files: [file("src/b.ts", 5, 1)],
      }),
      makeCommit("c3", BOB, oct(7), { files: [file("src/c.ts", 20)] }),
      makeCommit("c4", DEPENDABOT, oct(8), { files: [file("src/d.ts", 99)] }),
      makeCommit("c5", CLAUDE, oct(9), { committer: GITHUB }),
      makeCommit("c6", CAT, oct(10), { files: [file("src/e.ts", 7)] }),
      makeCommit("p0", ANA, sep(2)),
      makeCommit("p1", ANA, sep(25), { files: [file("src/a.ts", 10)] }),
      makeCommit("p2", BOB, sep(28), { files: [file("src/a.ts", 10, 10)] }),
      makeCommit("p3", BOB, sep(29)),
    ],
    prs: [
      makePr("1", ANA, oct(8)),
      makePr("2", BOB, oct(12)),
      makePr("3", DEPENDABOT, oct(9), { committer: GITHUB }),
      makePr("4", CLAUDE, oct(10), { committer: GITHUB }),
      makePr("5", BOB, oct(11), {
        state: "closed",
        mergedAt: undefined,
        closedAt: oct(11),
      }),
      makePr("old", ANA, sep(30)),
    ],
  });

describe("buildStats: kpis", () => {
  it("counts commits with a delta against the previous period", () => {
    const { kpis } = stats(sample());
    expect(kpis.commits.value).toBe(4);
    expect(kpis.commits.delta).toBeCloseTo((4 - 3) / 3, 10);
  });

  it("counts the same commits as the contributors table", () => {
    const { kpis, rows } = stats(sample());
    expect(kpis.commits.value).toBe(rows.reduce((n, r) => n + r.commits, 0));
  });

  it("counts contributors, identities and newcomers", () => {
    const { kpis } = stats(sample());
    expect(kpis.contributors).toEqual({
      value: 3,
      identities: 4,
      newcomers: 1,
    });
  });

  it("sums ignore-filtered lines with a delta on lines changed", () => {
    const { kpis } = stats(sample());
    expect(kpis.lines.added).toBe(42);
    expect(kpis.lines.deleted).toBe(3);
    expect(kpis.lines.delta).toBeCloseTo((45 - 30) / 30, 10);
  });

  it("counts merged PRs with an absolute delta", () => {
    const { kpis } = stats(sample());
    expect(kpis.prsMerged.value).toBe(3);
    expect(kpis.prsMerged.delta).toBe(3 - 1);
  });

  it("excludes the range end", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("in", ANA, new Date(2026, 9, 5).toISOString()),
        makeCommit("out", ANA, new Date(2026, 9, 19).toISOString()),
      ],
    });
    expect(stats(raw).kpis.commits.value).toBe(1);
  });

  it("uses null deltas when the data does not reach the previous period", () => {
    const raw = { ...sample(), since: new Date(2026, 9, 1).toISOString() };
    const { kpis } = stats(raw);
    expect(kpis.commits.delta).toBeNull();
    expect(kpis.lines.delta).toBeNull();
    expect(kpis.prsMerged.delta).toBeNull();
    expect(kpis.commits.value).toBe(4);
  });

  it("uses a null fraction when the previous period was empty", () => {
    const raw = makeRaw({ commits: [makeCommit("c", ANA, oct(6))] });
    const { kpis } = stats(raw);
    expect(kpis.commits.delta).toBeNull();
    expect(kpis.lines.delta).toBeNull();
    expect(kpis.prsMerged.delta).toBe(0);
  });

  it("accepts an explicit previous period", () => {
    const { kpis } = stats(sample(), {
      previous: { start: new Date(2026, 8, 28), end: new Date(2026, 9, 5) },
    });
    expect(kpis.commits.delta).toBeCloseTo((4 - 2) / 2, 10);
  });

  it("does not flag newcomers when the data starts at the range", () => {
    const raw = makeRaw({
      since: RANGE.start.toISOString(),
      commits: [makeCommit("c", ANA, oct(6))],
    });
    expect(stats(raw).kpis.contributors.newcomers).toBe(0);
  });

  it("needs data reaching the previous period to call someone new", () => {
    const raw = makeRaw({
      since: new Date(2026, 9, 4).toISOString(),
      commits: [makeCommit("c", ANA, oct(6))],
    });
    expect(stats(raw).kpis.contributors.newcomers).toBe(0);
  });

  it("is all zeros for no commits", () => {
    const { kpis, rows, buckets, legend } = stats(makeRaw());
    expect(kpis.commits).toEqual({ value: 0, delta: null });
    expect(kpis.contributors).toEqual({
      value: 0,
      identities: 0,
      newcomers: 0,
    });
    expect(kpis.lines).toEqual({ added: 0, deleted: 0, delta: null });
    expect(rows).toEqual([]);
    expect(legend).toEqual([]);
    expect(buckets.map((w) => w.byPerson)).toEqual([{}, {}]);
  });
});

describe("buildStats: week buckets", () => {
  it("buckets commits per person into Monday-start weeks", () => {
    const { buckets } = stats(sample());
    expect(
      buckets.map((b) => ({ start: b.start, byPerson: b.byPerson })),
    ).toEqual([
      {
        start: "2026-10-05",
        byPerson: {
          "p-ana": 1,
          "bob@example.com": 1,
          "cat@example.com": 1,
        },
      },
      { start: "2026-10-12", byPerson: { "p-ana": 1 } },
    ]);
    expect(buckets[0].label).toBe("Week of Oct 5, 2026");
    expect(buckets[0].short).toBe("Oct 5");
  });

  it("starts at the Monday before a mid-week range start", () => {
    const raw = makeRaw({ commits: [makeCommit("c", ANA, oct(8))] });
    const { buckets: weekly } = buildStats({
      raw,
      config,
      resolver: makeResolver(raw, config),
      rangeStart: new Date(2026, 9, 8),
      rangeEnd: new Date(2026, 9, 9),
    });
    expect(weekly.map((w) => w.start)).toEqual(["2026-10-05"]);
  });

  it("keeps empty weeks", () => {
    const raw = makeRaw({ commits: [makeCommit("c", ANA, oct(6))] });
    const { buckets: weekly } = buildStats({
      raw,
      config,
      resolver: makeResolver(raw, config),
      rangeStart: new Date(2026, 9, 5),
      rangeEnd: new Date(2026, 9, 26),
    });
    expect(weekly.map((w) => w.start)).toEqual([
      "2026-10-05",
      "2026-10-12",
      "2026-10-19",
    ]);
    expect(weekly[1].byPerson).toEqual({});
  });

  it("puts a Sunday commit in the week that began the Monday before", () => {
    const raw = makeRaw({ commits: [makeCommit("c", ANA, oct(11))] });
    expect(stats(raw).buckets[0].byPerson).toEqual({ "p-ana": 1 });
  });
});

describe("buildStats: rows and legend", () => {
  it("builds one row per person, busiest first", () => {
    const { rows } = stats(sample());
    expect(rows.map((r) => r.personId)).toEqual([
      "p-ana",
      "bob@example.com",
      "cat@example.com",
    ]);
    const ana = rows[0];
    expect(ana.name).toBe("Ana");
    expect(ana.commits).toBe(2);
    expect(ana.added).toBe(15);
    expect(ana.deleted).toBe(3);
    expect(ana.files).toBe(2);
    expect(ana.aiAssistedPct).toBe(50);
    expect(ana.sharePct).toBe(50);
    expect(ana.lastActive).toBe(oct(13));
    expect(ana.identities).toEqual(["ana@example.com", "ana@laptop.local"]);
    expect(rows[1].sharePct).toBe(25);
  });

  it("colors each person by a stable hash of the id and mirrors it in the legend", () => {
    const { rows, legend } = stats(sample());
    for (const row of rows)
      expect(row.color).toBe(colorForPerson(row.personId));
    expect(legend).toEqual(
      rows.map((r) => ({
        personId: r.personId,
        name: r.name,
        color: colorForPerson(r.personId),
      })),
    );
  });

  it("gives a person the same color whatever range or rank they have", () => {
    const wide = buildStats({
      raw: sample(),
      config,
      resolver: makeResolver(sample(), config),
      rangeStart: new Date(2026, 8, 1),
      rangeEnd: new Date(2026, 10, 1),
    });
    const narrow = stats(sample());
    for (const row of narrow.rows) {
      expect(wide.rows.find((r) => r.personId === row.personId)?.color).toBe(
        row.color,
      );
    }
  });

  it("credits an agent-authored commit to its PR owner", () => {
    const raw = makeRaw({
      prs: [makePr("9", BOB, oct(8))],
      commits: [
        makeCommit("c", CLAUDE, oct(8), {
          prId: "9",
          committer: GITHUB,
          files: [file("src/a.ts", 4)],
        }),
      ],
    });
    const { rows } = stats(raw);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      personId: "bob@example.com",
      commits: 1,
      added: 4,
      aiAssistedPct: 100,
      identities: [],
    });
  });

  it("breaks commit ties by name", () => {
    const raw = makeRaw({
      commits: [makeCommit("c1", CAT, oct(6)), makeCommit("c2", BOB, oct(6))],
    });
    expect(stats(raw, {}, DEFAULT_CONFIG).rows.map((r) => r.name)).toEqual([
      "Bob",
      "Cat",
    ]);
  });
});

describe("bucketGranularity", () => {
  it("is day for 7d and month, week for a quarter, month for a year", () => {
    const now = new Date(2026, 9, 20);
    expect(bucketGranularity(currentPeriod("7d", now))).toBe("day");
    expect(bucketGranularity(currentPeriod("month", now))).toBe("day");
    expect(bucketGranularity(currentPeriod("quarter", now))).toBe("week");
    expect(bucketGranularity(currentPeriod("year", now))).toBe("month");
  });
});

describe("buildStats: day and month buckets", () => {
  const bucketed = (
    raw: RawData,
    start: Date,
    end: Date,
    granularity: "day" | "week" | "month",
  ) =>
    buildStats({
      raw,
      config,
      resolver: makeResolver(raw, config),
      rangeStart: start,
      rangeEnd: end,
      granularity,
    });

  it("defaults to weeks", () => {
    expect(stats(makeRaw()).granularity).toBe("week");
  });

  it("makes one bucket per local day, empty days included", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a", ANA, oct(5)),
        makeCommit("b", BOB, oct(5)),
        makeCommit("c", ANA, oct(7)),
      ],
    });
    const { buckets, granularity } = bucketed(
      raw,
      new Date(2026, 9, 5),
      new Date(2026, 9, 8),
      "day",
    );
    expect(granularity).toBe("day");
    expect(buckets.map((b) => b.start)).toEqual([
      "2026-10-05",
      "2026-10-06",
      "2026-10-07",
    ]);
    expect(buckets[0].label).toBe("Oct 5, 2026");
    expect(buckets[0].short).toBe("Oct 5");
    expect(buckets[0].byPerson).toEqual({ "bob@example.com": 1, "p-ana": 1 });
    expect(buckets[1].byPerson).toEqual({});
    expect(buckets[2].byPerson).toEqual({ "p-ana": 1 });
  });

  it("buckets by local calendar month and labels them", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a", ANA, sep(20)),
        makeCommit("b", BOB, oct(3)),
        makeCommit("c", ANA, oct(30)),
      ],
    });
    const { buckets, granularity } = bucketed(
      raw,
      new Date(2026, 8, 15),
      new Date(2026, 9, 31),
      "month",
    );
    expect(granularity).toBe("month");
    expect(buckets.map((b) => b.start)).toEqual(["2026-09-01", "2026-10-01"]);
    expect(buckets.map((b) => b.label)).toEqual(["Sep 2026", "Oct 2026"]);
    expect(buckets.map((b) => b.short)).toEqual(["Sep 2026", "Oct 2026"]);
    expect(buckets[0].byPerson).toEqual({ "p-ana": 1 });
    expect(buckets[1].byPerson).toEqual({ "bob@example.com": 1, "p-ana": 1 });
  });

  it("keeps empty months and crosses the year boundary", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a", ANA, new Date(2025, 10, 10, 12).toISOString()),
        makeCommit("b", ANA, new Date(2026, 1, 10, 12).toISOString()),
      ],
    });
    const { buckets } = bucketed(
      raw,
      new Date(2025, 10, 1),
      new Date(2026, 2, 1),
      "month",
    );
    expect(buckets.map((b) => b.start)).toEqual([
      "2025-11-01",
      "2025-12-01",
      "2026-01-01",
      "2026-02-01",
    ]);
    expect(buckets[1].byPerson).toEqual({});
  });

  it("only counts commits inside the range", () => {
    const raw = makeRaw({
      commits: [makeCommit("a", ANA, sep(2)), makeCommit("b", ANA, oct(3))],
    });
    const { buckets } = bucketed(
      raw,
      new Date(2026, 8, 15),
      new Date(2026, 9, 31),
      "month",
    );
    expect(buckets[0].byPerson).toEqual({});
    expect(buckets[1].byPerson).toEqual({ "p-ana": 1 });
  });
});
