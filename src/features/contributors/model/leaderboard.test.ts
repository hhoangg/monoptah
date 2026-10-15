import { describe, expect, it } from "vitest";
import { buildLeaderboard, trend } from "./leaderboard";
import { currentPeriod } from "./period";
import type { PersonScore, PeriodScores } from "./scoring";

function person(id: string, extra: Partial<PersonScore> = {}): PersonScore {
  return {
    personId: id,
    name: id,
    score: 100,
    delivery: 50,
    deliveryFinal: 50,
    quality: 1,
    survival: null,
    care: 0,
    review: 0,
    consistency: 10,
    breadth: 0,
    activeDays: 2,
    streak: 2,
    mergedPrs: 1,
    branchCommits: 1,
    mainCommits: 1,
    aiAssistedPct: 0,
    fixPrs: 0,
    netNegativePrs: 0,
    reviewedPrs: 0,
    parts: { delivery: 50, care: 0, review: 0, consistency: 10, breadth: 0 },
    modules: [],
    prs: [],
    ...extra,
  };
}

function season(
  people: PersonScore[],
  month = 9,
  extra: Partial<PeriodScores> = {},
): PeriodScores {
  return {
    period: currentPeriod("month", new Date(2026, month, 10)),
    people,
    unattributed: { prs: 0, commits: 0 },
    ghAvailable: true,
    newcomers: [],
    ...extra,
  };
}

const ids = (rows: { personId: string }[]) => rows.map((r) => r.personId);

describe("buildLeaderboard: ranking", () => {
  it("ranks by score with 1-based ordinal ranks", () => {
    const { rows, podium } = buildLeaderboard(
      season([
        person("low", { score: 10 }),
        person("high", { score: 300 }),
        person("mid", { score: 200 }),
      ]),
      null,
      {},
    );
    expect(ids(rows)).toEqual(["high", "mid", "low"]);
    expect(rows.map((r) => r.rank)).toEqual([1, 2, 3]);
    expect(ids(podium)).toEqual(["high", "mid", "low"]);
  });

  it("breaks ties by merged PRs, then name", () => {
    const { rows } = buildLeaderboard(
      season([
        person("zed", { score: 100, mergedPrs: 2 }),
        person("bob", { score: 100, mergedPrs: 1 }),
        person("amy", { score: 100, mergedPrs: 1 }),
        person("top", { score: 100, mergedPrs: 5 }),
      ]),
      null,
      {},
    );
    expect(ids(rows)).toEqual(["top", "zed", "amy", "bob"]);
    expect(rows.map((r) => r.rank)).toEqual([1, 2, 3, 4]);
  });

  it("ranks by the rounded score the user sees, then merged PRs", () => {
    const { rows } = buildLeaderboard(
      season([
        person("few", { score: 612.4, mergedPrs: 1 }),
        person("many", { score: 611.6, mergedPrs: 4 }),
        person("ahead", { score: 613.2, mergedPrs: 0 }),
      ]),
      null,
      {},
    );
    expect(ids(rows)).toEqual(["ahead", "many", "few"]);
  });

  it("limits the podium to the top 3", () => {
    const { rows, podium } = buildLeaderboard(
      season(
        ["a", "b", "c", "d", "e"].map((id, i) => person(id, { score: 50 - i })),
      ),
      null,
      {},
    );
    expect(rows).toHaveLength(5);
    expect(ids(podium)).toEqual(["a", "b", "c"]);
  });

  it("omits people with no score and no activity", () => {
    const empty = person("ghost", {
      score: 0,
      activeDays: 0,
      mergedPrs: 0,
      mainCommits: 0,
      reviewedPrs: 0,
    });
    const { rows } = buildLeaderboard(
      season([person("real"), empty]),
      null,
      {},
    );
    expect(ids(rows)).toEqual(["real"]);
  });

  it("flags the current user", () => {
    const { rows } = buildLeaderboard(
      season([person("a", { score: 2 }), person("b", { score: 1 })]),
      null,
      { meId: "b" },
    );
    expect(rows.map((r) => r.isMe)).toEqual([false, true]);
  });

  it("handles an empty season", () => {
    expect(buildLeaderboard(season([]), null, {})).toEqual({
      rows: [],
      podium: [],
    });
  });
});

describe("buildLeaderboard: movement", () => {
  const previous = season(
    [
      person("a", { score: 300 }),
      person("b", { score: 200 }),
      person("c", { score: 100 }),
    ],
    8,
  );

  it("is previous rank minus current rank", () => {
    const { rows } = buildLeaderboard(
      season([
        person("c", { score: 900 }),
        person("a", { score: 800 }),
        person("b", { score: 700 }),
      ]),
      previous,
      {},
    );
    expect(rows.map((r) => [r.personId, r.movement])).toEqual([
      ["c", 2],
      ["a", -1],
      ["b", -1],
    ]);
  });

  it("is 0 when the rank did not change", () => {
    const { rows } = buildLeaderboard(
      season([person("a", { score: 9 })]),
      previous,
      {},
    );
    expect(rows[0].movement).toBe(0);
  });

  it("is null for someone absent from the previous season", () => {
    const { rows } = buildLeaderboard(
      season([person("new", { score: 999 }), person("a", { score: 5 })]),
      previous,
      {},
    );
    expect(rows.map((r) => [r.personId, r.movement])).toEqual([
      ["new", null],
      ["a", -1],
    ]);
  });

  it("is null for everyone without a previous season", () => {
    const { rows } = buildLeaderboard(season([person("a")]), null, {});
    expect(rows[0].movement).toBeNull();
  });
});

describe("buildLeaderboard: badges", () => {
  const badgeIds = (rows: { badges: { id: string }[] }[]) =>
    rows.map((r) => r.badges.map((b) => b.id));

  it("labels badges and gives each a glyph key", () => {
    const { rows } = buildLeaderboard(
      season(
        [
          person("a", {
            streak: 10,
            deliveryFinal: 10,
            fixPrs: 2,
            netNegativePrs: 2,
            review: 5,
          }),
        ],
        9,
        { newcomers: ["a"] },
      ),
      null,
      {},
    );
    const badges = rows[0].badges;
    expect(badges.map((b) => b.label)).toEqual([
      "Streak",
      "Builder",
      "Cleaner",
    ]);
    expect(badges.map((b) => b.icon)).toEqual(["fire", "hammer", "broom"]);
  });

  it("awards Streak from 10 days", () => {
    const { rows } = buildLeaderboard(
      season([
        person("a", { score: 3, streak: 10 }),
        person("b", { score: 2, streak: 9 }),
      ]),
      null,
      {},
    );
    expect(rows[0].badges.map((b) => b.id)).toContain("streak");
    expect(rows[1].badges.map((b) => b.id)).not.toContain("streak");
  });

  it("awards Builder to the highest delivery only", () => {
    const { rows } = buildLeaderboard(
      season([
        person("a", { score: 3, deliveryFinal: 40 }),
        person("b", { score: 2, deliveryFinal: 90 }),
      ]),
      null,
      {},
    );
    expect(badgeIds(rows)).toEqual([[], ["builder"]]);
  });

  it("gives a tied badge to the higher ranked person", () => {
    const { rows } = buildLeaderboard(
      season([
        person("a", { score: 3, deliveryFinal: 90 }),
        person("b", { score: 2, deliveryFinal: 90 }),
      ]),
      null,
      {},
    );
    expect(badgeIds(rows)).toEqual([["builder"], []]);
  });

  it("does not award Builder or Reviewer for zero points", () => {
    const { rows } = buildLeaderboard(
      season([person("a", { deliveryFinal: 0, review: 0 })]),
      null,
      {},
    );
    expect(rows[0].badges).toEqual([]);
  });

  it("needs 2 net-negative PRs for Cleaner and 2 fixes for Bug hunter", () => {
    const one = buildLeaderboard(
      season([person("a", { deliveryFinal: 0, netNegativePrs: 1, fixPrs: 1 })]),
      null,
      {},
    );
    expect(one.rows[0].badges).toEqual([]);
    const two = buildLeaderboard(
      season([
        person("a", { score: 3, deliveryFinal: 0, netNegativePrs: 3 }),
        person("b", { score: 2, deliveryFinal: 0, fixPrs: 2 }),
        person("c", { score: 1, deliveryFinal: 0, fixPrs: 4 }),
      ]),
      null,
      {},
    );
    expect(badgeIds(two.rows)).toEqual([["cleaner"], [], ["bug-hunter"]]);
    expect(two.rows[2].badges[0].label).toBe("Bug hunter");
  });

  it("awards Reviewer to the most review points", () => {
    const { rows } = buildLeaderboard(
      season([
        person("a", { score: 3, deliveryFinal: 0, review: 20 }),
        person("b", { score: 2, deliveryFinal: 0, review: 60 }),
      ]),
      null,
      {},
    );
    expect(badgeIds(rows)).toEqual([[], ["reviewer"]]);
  });

  it("awards Newcomer from the season data", () => {
    const { rows } = buildLeaderboard(
      season([person("a", { deliveryFinal: 0 })], 9, { newcomers: ["a"] }),
      null,
      {},
    );
    expect(rows[0].badges.map((b) => b.id)).toEqual(["newcomer"]);
  });

  it("gives at most 3 badges, in a fixed order", () => {
    const { rows } = buildLeaderboard(
      season(
        [
          person("a", {
            streak: 12,
            deliveryFinal: 99,
            netNegativePrs: 3,
            fixPrs: 3,
            review: 30,
          }),
        ],
        9,
        { newcomers: ["a"] },
      ),
      null,
      {},
    );
    expect(rows[0].badges.map((b) => b.id)).toEqual([
      "streak",
      "builder",
      "cleaner",
    ]);
  });
});

describe("trend", () => {
  it("returns oldest to newest scores for each person", () => {
    const seasons = [
      season([person("a", { score: 30 }), person("b", { score: 5 })], 9),
      season([person("a", { score: 10 })], 7),
      season([person("a", { score: 20 }), person("c", { score: 7 })], 8),
    ];
    const out = trend(seasons);
    expect(out).toEqual([
      { personId: "a", points: [10, 20, 30] },
      { personId: "b", points: [0, 0, 5] },
      { personId: "c", points: [0, 7, 0] },
    ]);
  });

  it("keeps only the last 6 seasons", () => {
    const seasons = Array.from({ length: 8 }, (_, i) =>
      season([person("a", { score: i + 1 })], i),
    );
    expect(trend(seasons)).toEqual([
      { personId: "a", points: [3, 4, 5, 6, 7, 8] },
    ]);
  });

  it("is empty without seasons", () => {
    expect(trend([])).toEqual([]);
  });
});
