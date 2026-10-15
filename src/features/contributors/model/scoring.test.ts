import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import { buildResolver } from "./identity";
import {
  attributeCommits,
  attributePrs,
  careRate,
  iterationPoints,
  isDocsPath,
  isTestPath,
  moduleOf,
  observedIdentities,
  prPoints,
  prSize,
  qualityFactor,
  scorePeriod,
  streakOf,
} from "./scoring";
import { currentPeriod } from "./period";
import type { Period } from "./period";
import type { PersonScore, PeriodScores } from "./scoring";
import {
  ANA,
  BOB,
  CAT,
  CLAUDE,
  DEPENDABOT,
  GITHUB,
  PERIOD,
  PERIOD_NOW,
  file,
  ident,
  makeCommit,
  makePr,
  makeRaw,
  makeResolver,
  makeReview,
  oct,
  withConfig,
} from "./testFixtures";
import type { ContributorsConfig, RawCommit, RawData, RawPr } from "./types";

function score(
  raw: RawData,
  config: ContributorsConfig = DEFAULT_CONFIG,
): PeriodScores {
  return scorePeriod({
    raw,
    config,
    resolver: makeResolver(raw, config),
    period: PERIOD,
    now: PERIOD_NOW,
  });
}

function of(scores: PeriodScores, name: string): PersonScore {
  const found = scores.people.find((p) => p.name === name);
  if (!found) throw new Error(`no person named ${name}`);
  return found;
}

describe("prSize", () => {
  it("is log2(1 + added + 0.5 * deleted)", () => {
    expect(prSize([file("a.ts", 7)])).toBeCloseTo(3, 10);
    expect(prSize([file("a.ts", 3), file("b.ts", 2, 8)])).toBeCloseTo(
      Math.log2(1 + 3 + 2 + 4),
      10,
    );
  });

  it("is 0 for no files", () => {
    expect(prSize([])).toBe(0);
  });
});

describe("iterationPoints", () => {
  it("follows 3 * log2(1 + n) and caps at 12", () => {
    expect(iterationPoints(1)).toBeCloseTo(3, 10);
    expect(iterationPoints(3)).toBeCloseTo(6, 10);
    expect(iterationPoints(7)).toBeCloseTo(9, 10);
    expect(iterationPoints(15)).toBeCloseTo(12, 10);
    expect(iterationPoints(500)).toBe(12);
  });

  it("treats a commit count below 1 as 1", () => {
    expect(iterationPoints(0)).toBeCloseTo(3, 10);
  });
});

describe("prPoints", () => {
  it("is 10 + 4 * size + iteration", () => {
    const files = [file("a.ts", 7)];
    expect(prPoints(files, 7)).toBeCloseTo(10 + 4 * 3 + 9, 10);
  });

  it("caps at 72", () => {
    expect(prPoints([file("a.ts", 10_000_000)], 50)).toBe(72);
  });
});

describe("qualityFactor", () => {
  it("is 1 without a survival sample", () => {
    expect(qualityFactor(null)).toBe(1);
  });

  it("is 0.8 + 0.4 * survival", () => {
    expect(qualityFactor(0)).toBeCloseTo(0.8, 10);
    expect(qualityFactor(0.5)).toBeCloseTo(1, 10);
    expect(qualityFactor(0.85)).toBeCloseTo(1.14, 10);
    expect(qualityFactor(1)).toBeCloseTo(1.2, 10);
  });

  it("clamps to [0.8, 1.2]", () => {
    expect(qualityFactor(3)).toBe(1.2);
    expect(qualityFactor(-2)).toBe(0.8);
  });
});

describe("careRate", () => {
  const none = { test: false, docs: false, netNegative: false, fix: false };

  it("adds the four bonuses", () => {
    expect(careRate(none)).toBe(0);
    expect(careRate({ ...none, test: true })).toBeCloseTo(0.2, 10);
    expect(careRate({ ...none, docs: true })).toBeCloseTo(0.1, 10);
    expect(careRate({ ...none, netNegative: true })).toBeCloseTo(0.15, 10);
    expect(careRate({ ...none, fix: true })).toBeCloseTo(0.15, 10);
    expect(
      careRate({ test: true, docs: true, netNegative: true, fix: true }),
    ).toBeCloseTo(0.6, 10);
  });
});

describe("moduleOf", () => {
  it("takes the first two segments of the directory part", () => {
    expect(moduleOf("src/features/x/a.ts")).toBe("src/features");
    expect(moduleOf("src-tauri/src/lib.rs")).toBe("src-tauri/src");
    expect(moduleOf("src/a.ts")).toBe("src");
    expect(moduleOf("docs/a.md")).toBe("docs");
  });

  it("uses (root) for a root file", () => {
    expect(moduleOf("README.md")).toBe("(root)");
  });

  it("normalizes backslashes and a leading ./", () => {
    expect(moduleOf("./src\\features\\a.ts")).toBe("src/features");
  });
});

describe("isTestPath and isDocsPath", () => {
  it("recognizes test files", () => {
    for (const p of [
      "src/a.test.ts",
      "src/a.spec.tsx",
      "tests/a.ts",
      "src/test/a.ts",
      "src/__tests__/a.ts",
      "spec/a.rb",
      "pkg/a_test.go",
      "src/a_test.rs",
      "py/a_test.py",
      "SRC/A.TEST.TS",
    ]) {
      expect(isTestPath(p), p).toBe(true);
    }
    for (const p of ["src/latest.ts", "src/contest/a.ts", "src/a.ts"]) {
      expect(isTestPath(p), p).toBe(false);
    }
  });

  it("recognizes docs files", () => {
    for (const p of [
      "README.md",
      "src/a/guide.mdx",
      "docs/a.png",
      "docs/deep/a.txt",
      "notes.rst",
    ]) {
      expect(isDocsPath(p), p).toBe(true);
    }
    for (const p of ["src/a.ts", "src/docs/a.ts", "Makefile"]) {
      expect(isDocsPath(p), p).toBe(false);
    }
  });
});

describe("streakOf", () => {
  const day = (d: number) => `2026-10-${String(d).padStart(2, "0")}`;
  const streak = (days: number[]) =>
    streakOf(days.map(day), PERIOD, PERIOD_NOW);

  it("is 0 without activity", () => {
    expect(streak([])).toBe(0);
  });

  it("counts consecutive working days", () => {
    expect(streak([5, 6, 7, 8, 9])).toBe(5);
    expect(streak([5, 6, 8, 9])).toBe(2);
  });

  it("does not break across an idle weekend", () => {
    expect(streak([8, 9, 12, 13])).toBe(4);
  });

  it("skips weekend days entirely, active or not", () => {
    expect(streak([9, 10, 12])).toBe(2);
    expect(streak([9, 11, 12])).toBe(2);
    expect(streak([9, 10, 11, 12])).toBe(2);
  });

  it("gives a weekend-only run no streak", () => {
    expect(streak([3, 4])).toBe(0);
  });

  it("breaks on a missed working day", () => {
    expect(streak([9, 13, 14])).toBe(2);
  });

  it("ignores days outside the period", () => {
    expect(streakOf(["2026-09-30", "2026-10-01"], PERIOD, PERIOD_NOW)).toBe(1);
    expect(streakOf(["2026-11-02"], PERIOD, PERIOD_NOW)).toBe(0);
  });

  it("takes the longest run", () => {
    expect(streak([1, 2, 6, 7, 8, 9])).toBe(4);
  });

  it("does not look past today in the current period", () => {
    const now = new Date(2026, 9, 9, 10);
    expect(streakOf([5, 6, 7, 8, 9, 12, 13].map(day), PERIOD, now)).toBe(5);
  });

  it("runs over the days of a 7d period, even across a month change", () => {
    const now = new Date(2026, 10, 3, 10); // Tue 2026-11-03
    const week = currentPeriod("7d", now);
    expect(
      streakOf(
        ["2026-10-28", "2026-10-29", "2026-10-30", "2026-11-02"],
        week,
        now,
      ),
    ).toBe(4);
    expect(streakOf(["2026-10-27"], week, now)).toBe(0);
  });

  it("runs across the whole of a quarter", () => {
    const now = new Date(2026, 11, 31, 10);
    const quarter = currentPeriod("quarter", now);
    expect(
      streakOf(["2026-10-30", "2026-11-02", "2026-12-31"], quarter, now),
    ).toBe(2);
  });
});

describe("observedIdentities", () => {
  it("collects every identity in the raw data once", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", ANA, oct(5), { coAuthors: [BOB], committer: GITHUB }),
        makeCommit("c2", ANA, oct(6)),
      ],
      prs: [
        makePr("1", CAT, oct(6), {
          committer: BOB,
          landingCoAuthors: [CLAUDE],
          branchCommits: [
            { authoredAt: oct(5), committedAt: oct(5), authors: [CAT, ANA] },
          ],
        }),
      ],
      reviews: [makeReview("1", ident("dev", "", "dev-login"), oct(7))],
    });
    const keys = observedIdentities(raw).map((i) => i.email || i.login);
    expect(keys.sort()).toEqual(
      [
        "ana@example.com",
        "bob@example.com",
        "cat@example.com",
        "noreply@anthropic.com",
        "noreply@github.com",
        "dev-login",
      ].sort(),
    );
  });
});

describe("scorePeriod: delivery", () => {
  it("scores a merged PR by its final diff and commit count", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          commitCount: 3,
          files: [file("src/features/a.ts", 100)],
        }),
      ],
    });
    const ana = of(score(raw), "Ana");
    const expected = prPoints([file("src/features/a.ts", 100)], 3);
    expect(ana.delivery).toBeCloseTo(expected, 8);
    expect(ana.deliveryFinal).toBeCloseTo(expected, 8);
    expect(ana.quality).toBe(1);
    expect(ana.survival).toBeNull();
    expect(ana.mergedPrs).toBe(1);
    expect(ana.branchCommits).toBe(3);
    const pr = ana.prs[0];
    expect(pr.id).toBe("1");
    expect(pr.points).toBeCloseTo(expected, 8);
    expect(pr.deliveryShare).toBe(1);
    expect(pr.size).toBeCloseTo(prSize([file("a.ts", 100)]), 8);
    expect(pr.iteration).toBeCloseTo(6, 8);
    expect(pr.added).toBe(100);
    expect(pr.commitCount).toBe(3);
    expect(pr.mergedAt).toBe(oct(8));
  });

  it("caps a PR at 72 points", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          commitCount: 40,
          files: [file("src/a.ts", 5_000_000)],
        }),
      ],
    });
    expect(of(score(raw), "Ana").prs[0].points).toBe(72);
  });

  it("applies ignore globs before sizing", () => {
    const withLock = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          files: [file("package-lock.json", 5000), file("src/a.ts", 100)],
        }),
      ],
    });
    const without = makeRaw({
      prs: [makePr("1", ANA, oct(8), { files: [file("src/a.ts", 100)] })],
    });
    const a = of(score(withLock), "Ana");
    expect(a.delivery).toBeCloseTo(of(score(without), "Ana").delivery, 8);
    expect(a.prs[0].added).toBe(100);
  });

  it("applies ignore globs before test and docs detection", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          files: [file("src/a.ts", 100), file("src/gen/a.test.ts", 50)],
        }),
      ],
    });
    const ignored = withConfig({ ignore: ["src/gen/**"] });
    expect(of(score(raw), "Ana").prs[0].testFiles).toBe(true);
    expect(of(score(raw, ignored), "Ana").prs[0].testFiles).toBe(false);
  });

  it("gives closed PRs no delivery", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          state: "closed",
          mergedAt: undefined,
          closedAt: oct(8),
          branchCommits: [
            { authoredAt: oct(6), committedAt: oct(6), authors: [ANA] },
            { authoredAt: oct(7), committedAt: oct(7), authors: [ANA] },
          ],
        }),
      ],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.delivery).toBe(0);
    expect(ana.mergedPrs).toBe(0);
    expect(ana.prs).toEqual([]);
    expect(ana.activeDays).toBe(2);
    expect(ana.consistency).toBe(5 * 2 + 2 * 2);
  });

  it("falls back to closedAt for a closed PR without branch commits", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          state: "closed",
          mergedAt: undefined,
          closedAt: oct(8),
        }),
      ],
    });
    expect(of(score(raw), "Ana").activeDays).toBe(1);
  });

  it("only counts PRs merged inside the period", () => {
    const raw = makeRaw({
      prs: [
        makePr("before", ANA, new Date(2026, 8, 30, 23).toISOString()),
        makePr("first", ANA, new Date(2026, 9, 1, 0, 0, 1).toISOString()),
        makePr("last", ANA, new Date(2026, 9, 31, 23).toISOString()),
        makePr("after", ANA, new Date(2026, 10, 1, 0, 0, 0).toISOString()),
      ],
    });
    expect(of(score(raw), "Ana").prs.map((p) => p.id)).toEqual(
      expect.arrayContaining(["first", "last"]),
    );
    expect(of(score(raw), "Ana").mergedPrs).toBe(2);
  });

  it("zeroes delivery and care of a reverted PR and nothing else", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          title: "fix: a thing",
          files: [file("src/a.ts", 100), file("src/a.test.ts", 20)],
        }),
        makePr("2", ANA, oct(9)),
      ],
      revertedPrIds: ["1"],
    });
    const ana = of(score(raw), "Ana");
    const reverted = ana.prs.find((p) => p.id === "1");
    expect(reverted?.reverted).toBe(true);
    expect(reverted?.points).toBe(0);
    expect(reverted?.care).toBe(0);
    expect(ana.delivery).toBeCloseTo(prPoints([file("a", 100)], 1), 8);
    expect(ana.care).toBe(0);
    expect(ana.mergedPrs).toBe(2);
    expect(ana.fixPrs).toBe(0);
  });
});

describe("scorePeriod: quality", () => {
  it("multiplies delivery by the survival factor", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(8)), makePr("2", ANA, oct(9))],
      survival: [
        { prId: "1", written: 100, alive: 50 },
        { prId: "2", written: 300, alive: 300 },
      ],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.survival).toBeCloseTo(350 / 400, 10);
    expect(ana.quality).toBeCloseTo(0.8 + 0.4 * 0.875, 10);
    expect(ana.deliveryFinal).toBeCloseTo(ana.delivery * ana.quality, 8);
  });

  it("ignores samples with nothing written", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(8))],
      survival: [{ prId: "1", written: 0, alive: 0 }],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.survival).toBeNull();
    expect(ana.quality).toBe(1);
  });

  it("ignores the survival sample of a reverted PR", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(6)),
        makePr("2", ANA, oct(7)),
        makePr("3", ANA, oct(8)),
        makePr("4", ANA, oct(9)),
      ],
      revertedPrIds: ["4"],
      survival: [
        { prId: "1", written: 1000, alive: 900 },
        { prId: "2", written: 1000, alive: 900 },
        { prId: "3", written: 1000, alive: 900 },
        { prId: "4", written: 1000, alive: 0 },
      ],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.survival).toBeCloseTo(0.9, 10);
    expect(ana.quality).toBeCloseTo(1.16, 10);
  });

  it("does not apply quality to care", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          files: [file("src/a.ts", 100), file("src/a.test.ts", 20)],
        }),
      ],
      survival: [{ prId: "1", written: 100, alive: 0 }],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.quality).toBeCloseTo(0.8, 10);
    expect(ana.care).toBeCloseTo(ana.prs[0].points * 0.2, 8);
  });
});

describe("scorePeriod: care", () => {
  const care = (pr: Partial<RawPr>) =>
    of(score(makeRaw({ prs: [makePr("1", ANA, oct(8), pr)] })), "Ana");

  it("adds 20% for test files", () => {
    const a = care({
      files: [file("src/a.ts", 100), file("src/a.test.ts", 10)],
    });
    expect(a.prs[0].testFiles).toBe(true);
    expect(a.care).toBeCloseTo(a.prs[0].points * 0.2, 8);
  });

  it("adds 10% for docs", () => {
    const a = care({ files: [file("src/a.ts", 100), file("docs/a.md", 10)] });
    expect(a.prs[0].docs).toBe(true);
    expect(a.care).toBeCloseTo(a.prs[0].points * 0.1, 8);
  });

  it("adds 15% when more lines are deleted than added", () => {
    const a = care({ files: [file("src/a.ts", 100, 400)] });
    expect(a.prs[0].netNegative).toBe(true);
    expect(a.netNegativePrs).toBe(1);
    expect(a.care).toBeCloseTo(a.prs[0].points * 0.15, 8);
  });

  it("is not net negative when deleted equals added", () => {
    expect(
      care({ files: [file("src/a.ts", 100, 100)] }).prs[0].netNegative,
    ).toBe(false);
  });

  it("adds 15% for a fix title or a closed bug issue", () => {
    for (const title of ["fix: a", "Fix(core): a", "fix!: a", "fix a bug"]) {
      const a = care({ title });
      expect(a.prs[0].fix, title).toBe(true);
      expect(a.care).toBeCloseTo(a.prs[0].points * 0.15, 8);
    }
    expect(care({ title: "prefix: a" }).prs[0].fix).toBe(false);
    expect(care({ title: "fixture update" }).prs[0].fix).toBe(false);
    const bug = care({ closesBug: true });
    expect(bug.prs[0].fix).toBe(true);
    expect(bug.fixPrs).toBe(1);
  });

  it("stacks the bonuses", () => {
    const a = care({
      title: "fix: all",
      files: [
        file("src/a.ts", 10, 400),
        file("src/a.test.ts", 5),
        file("README.md", 1),
      ],
    });
    expect(a.care).toBeCloseTo(a.prs[0].points * 0.6, 8);
  });
});

describe("scorePeriod: credit", () => {
  it("splits 60/40 with human co-authors", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          landingCoAuthors: [BOB, CAT],
          files: [file("src/a.ts", 100), file("src/a.test.ts", 10)],
        }),
      ],
    });
    const scores = score(raw);
    const points = of(scores, "Ana").prs[0].points;
    expect(of(scores, "Ana").prs[0].deliveryShare).toBeCloseTo(0.6, 10);
    expect(of(scores, "Bob").prs[0].deliveryShare).toBeCloseTo(0.2, 10);
    expect(of(scores, "Cat").prs[0].deliveryShare).toBeCloseTo(0.2, 10);
    expect(of(scores, "Ana").delivery).toBeCloseTo(points * 0.6, 8);
    expect(of(scores, "Bob").delivery).toBeCloseTo(points * 0.2, 8);
    expect(of(scores, "Bob").care).toBeCloseTo(points * 0.2 * 0.2, 8);
    expect(of(scores, "Bob").mergedPrs).toBe(1);
    expect(of(scores, "Bob").branchCommits).toBe(0);
    expect(of(scores, "Ana").branchCommits).toBe(1);
    const total = scores.people.reduce((sum, p) => sum + p.delivery, 0);
    expect(total).toBeCloseTo(points, 8);
  });

  it("gives a single human co-author 40%", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(8), { landingCoAuthors: [BOB] })],
    });
    const scores = score(raw);
    expect(of(scores, "Ana").prs[0].deliveryShare).toBeCloseTo(0.6, 10);
    expect(of(scores, "Bob").prs[0].deliveryShare).toBeCloseTo(0.4, 10);
  });

  it("keeps 100% when the only co-author is the owner or an agent", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), { landingCoAuthors: [ANA, CLAUDE] }),
        makePr("2", ANA, oct(9), { landingCoAuthors: [CLAUDE, DEPENDABOT] }),
      ],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.prs.map((p) => p.deliveryShare)).toEqual([1, 1]);
    expect(score(raw).people).toHaveLength(1);
  });

  it("falls back to the committer when the author is an agent", () => {
    const raw = makeRaw({
      prs: [makePr("1", CLAUDE, oct(8), { committer: BOB })],
    });
    const scores = score(raw);
    expect(scores.people.map((p) => p.name)).toEqual(["Bob"]);
    expect(scores.unattributed.prs).toBe(0);
  });

  it("falls back to an identity mapping when the committer is not human", () => {
    const config = withConfig({
      people: [
        {
          id: "p-dev",
          name: "Dev",
          identities: ["noreply@anthropic.com", "dev@example.com"],
        },
      ],
    });
    const raw = makeRaw({
      prs: [makePr("1", CLAUDE, oct(8), { committer: GITHUB })],
    });
    const scores = score(raw, config);
    expect(scores.people.map((p) => p.personId)).toEqual(["p-dev"]);
    expect(scores.people[0].name).toBe("Dev");
  });

  it("never credits an agent mapped to a person with no human identity", () => {
    const config = withConfig({
      people: [
        { id: "p-bot", name: "Bot", identities: ["noreply@anthropic.com"] },
      ],
    });
    const raw = makeRaw({
      prs: [makePr("1", CLAUDE, oct(8), { committer: GITHUB })],
    });
    const scores = score(raw, config);
    expect(scores.people).toEqual([]);
    expect(scores.unattributed.prs).toBe(1);
  });

  it("never credits an agent linked to itself through its gh login", () => {
    const copilot = ident("Copilot", "", "Copilot");
    const copilotMail = ident(
      "Copilot",
      "198982749+Copilot@users.noreply.github.com",
    );
    const raw = makeRaw({
      prs: [makePr("1", copilot, oct(8), { committer: GITHUB })],
      commits: [makeCommit("c1", copilotMail, oct(8), { committer: GITHUB })],
    });
    const scores = score(raw);
    expect(scores.people).toEqual([]);
    expect(scores.unattributed).toEqual({ prs: 1, commits: 1 });
  });

  it("never credits an agent grouped by a mailmap entry", () => {
    const alias = ident("Claude", "claude-alt@anthropic.com");
    const raw = makeRaw({
      prs: [makePr("1", alias, oct(8), { committer: GITHUB })],
    });
    const resolver = buildResolver(DEFAULT_CONFIG, {
      merge: true,
      mailmap: [
        {
          name: "Claude",
          identities: ["claude@anthropic.com", "claude-alt@anthropic.com"],
        },
      ],
      observedIdentities: observedIdentities(raw),
    });
    const scores = scorePeriod({
      raw,
      config: DEFAULT_CONFIG,
      resolver,
      period: PERIOD,
      now: PERIOD_NOW,
    });
    expect(scores.people).toEqual([]);
    expect(scores.unattributed.prs).toBe(1);
  });

  it("credits a human whose git name is an agent word once a config person lists their email", () => {
    const devin = ident("Devin", "devin@company.com");
    const config = withConfig({
      people: [
        { id: "p-devin", name: "Devin Lee", identities: ["devin@company.com"] },
      ],
    });
    const raw = makeRaw({
      prs: [makePr("1", devin, oct(8), { committer: GITHUB })],
      commits: [makeCommit("c1", devin, oct(8))],
    });
    const scores = score(raw, config);
    expect(scores.people.map((p) => p.personId)).toEqual(["p-devin"]);
    expect(scores.people[0].mainCommits).toBe(1);
    expect(scores.unattributed).toEqual({ prs: 0, commits: 0 });
    expect(score(raw).people).toEqual([]);
  });

  it("trusts a config person built around an agent noreply email", () => {
    const config = withConfig({
      people: [
        {
          id: "p-copilot",
          name: "Copilot",
          identities: ["198982749+copilot@users.noreply.github.com"],
        },
      ],
    });
    const raw = makeRaw({
      prs: [
        makePr("1", ident("Copilot", "", "Copilot"), oct(8), {
          committer: GITHUB,
        }),
      ],
    });
    expect(score(raw, config).people.map((p) => p.personId)).toEqual([
      "p-copilot",
    ]);
  });

  describe("a mailmap person judged by what was observed", () => {
    const agentNamed = ident("Cursor Agent", "ana@example.com");
    const human = ident("Ana", "ana@example.com");
    const claude = ident("Claude", "claude@anthropic.com");
    const run = (commits: RawCommit[]) => {
      const raw = makeRaw({
        prs: [makePr("1", claude, oct(8), { committer: GITHUB })],
        commits,
      });
      const resolver = buildResolver(DEFAULT_CONFIG, {
        merge: true,
        mailmap: [
          {
            name: "Ana",
            identities: ["ana@example.com", "claude@anthropic.com"],
          },
        ],
        observedIdentities: observedIdentities(raw),
      });
      return scorePeriod({
        raw,
        config: DEFAULT_CONFIG,
        resolver,
        period: PERIOD,
        now: PERIOD_NOW,
      });
    };

    it("is credited when any observed name of its email is human, in any order", () => {
      for (const order of [
        [agentNamed, human],
        [human, agentNamed],
      ]) {
        const scores = run(
          order.map((id, i) => makeCommit(`c${i}`, id, oct(6 + i))),
        );
        const ana = scores.people.find((p) => p.personId === "ana@example.com");
        expect(ana?.mergedPrs).toBe(1);
        expect(scores.unattributed.prs).toBe(0);
      }
    });

    it("is not credited when every observed name is an agent", () => {
      const scores = run([makeCommit("c0", agentNamed, oct(6))]);
      expect(scores.unattributed.prs).toBe(1);
      expect(scores.people.every((p) => p.mergedPrs === 0)).toBe(true);
    });
  });

  it("reports an agent PR nobody can own as unattributed", () => {
    const raw = makeRaw({
      prs: [makePr("1", CLAUDE, oct(8), { committer: GITHUB })],
    });
    const scores = score(raw);
    expect(scores.people).toEqual([]);
    expect(scores.unattributed.prs).toBe(1);
  });

  it("drops bot PRs nobody can own without reporting them", () => {
    const raw = makeRaw({
      prs: [makePr("1", DEPENDABOT, oct(8), { committer: GITHUB })],
    });
    const scores = score(raw);
    expect(scores.people).toEqual([]);
    expect(scores.unattributed.prs).toBe(0);
  });

  it("resolves aliases of one person to the same score", () => {
    const config = withConfig({
      people: [
        {
          id: "p-ana",
          name: "Ana Full",
          identities: ["ana@example.com", "ana@laptop.local"],
        },
      ],
    });
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8)),
        makePr("2", ident("Ana", "ana@laptop.local"), oct(9)),
      ],
    });
    const scores = score(raw, config);
    expect(scores.people).toHaveLength(1);
    expect(scores.people[0].name).toBe("Ana Full");
    expect(scores.people[0].mergedPrs).toBe(2);
  });
});

describe("scorePeriod: commits and consistency", () => {
  it("counts owned mainline commits and their days", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", ANA, oct(5, 9)),
        makeCommit("c2", ANA, oct(5, 15)),
        makeCommit("c3", ANA, oct(6)),
        makeCommit("old", ANA, new Date(2026, 8, 20).toISOString()),
        makeCommit("bot", DEPENDABOT, oct(7)),
      ],
    });
    const scores = score(raw);
    const ana = of(scores, "Ana");
    expect(ana.mainCommits).toBe(3);
    expect(ana.activeDays).toBe(2);
    expect(ana.streak).toBe(2);
    expect(ana.consistency).toBe(5 * 2 + 2 * 2);
    expect(scores.people).toHaveLength(1);
  });

  it("buckets days by committer date, not author date", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", ANA, oct(6), { authoredAt: oct(1) }),
        makeCommit("c2", ANA, oct(6, 20), { authoredAt: oct(2) }),
      ],
    });
    expect(of(score(raw), "Ana").activeDays).toBe(1);
  });

  it("counts days of branch commits of owned PRs", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(9), {
          commitCount: 3,
          branchCommits: [
            { authoredAt: oct(6), committedAt: oct(6), authors: [ANA] },
            { authoredAt: oct(7), committedAt: oct(7), authors: [ANA] },
            { authoredAt: oct(9), committedAt: oct(9), authors: [ANA] },
          ],
        }),
      ],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.activeDays).toBe(3);
    expect(ana.streak).toBe(2);
  });

  it("ignores branch commit days outside the period", () => {
    const sep = new Date(2026, 8, 29).toISOString();
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(2), {
          branchCommits: [
            { authoredAt: sep, committedAt: sep, authors: [ANA] },
          ],
        }),
      ],
    });
    expect(of(score(raw), "Ana").activeDays).toBe(0);
  });

  it("credits a branch commit day to its human author on someone else's PR", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(9), {
          branchCommits: [
            { authoredAt: oct(7), committedAt: oct(7), authors: [ANA, BOB] },
            {
              authoredAt: oct(8),
              committedAt: oct(8),
              authors: [CLAUDE, DEPENDABOT],
            },
          ],
        }),
      ],
    });
    const scores = score(raw);
    expect(of(scores, "Bob").activeDays).toBe(1);
    expect(of(scores, "Bob").mergedPrs).toBe(0);
    expect(of(scores, "Ana").activeDays).toBe(2);
  });

  it("caps the streak bonus at 14 days", () => {
    const days = [
      1, 2, 5, 6, 7, 8, 9, 12, 13, 14, 15, 16, 19, 20, 21, 22, 23, 26, 27, 28,
      29, 30,
    ];
    const raw = makeRaw({
      commits: days.map((d) => makeCommit(`c${d}`, ANA, oct(d))),
    });
    const ana = of(score(raw), "Ana");
    expect(ana.activeDays).toBe(22);
    expect(ana.streak).toBe(22);
    expect(ana.consistency).toBe(5 * 22 + 2 * 14);
  });

  it("does not let an idle weekend break the streak", () => {
    const raw = makeRaw({
      commits: [8, 9, 12, 13].map((d) => makeCommit(`c${d}`, ANA, oct(d))),
    });
    expect(of(score(raw), "Ana").streak).toBe(4);
  });

  it("counts a weekend with activity as an active day", () => {
    const raw = makeRaw({
      commits: [9, 10, 12].map((d) => makeCommit(`c${d}`, ANA, oct(d))),
    });
    const ana = of(score(raw), "Ana");
    expect(ana.activeDays).toBe(3);
    expect(ana.streak).toBe(2);
    expect(ana.consistency).toBe(5 * 3 + 2 * 2);
  });

  it("counts branch commit days of a PR merged in the next period", () => {
    const at = (d: number) => oct(d, 10);
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, new Date(2026, 10, 3, 12).toISOString(), {
          commitCount: 3,
          branchCommits: [27, 28, 29].map((d) => ({
            authoredAt: at(d),
            committedAt: at(d),
            authors: [ANA],
          })),
        }),
      ],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.activeDays).toBe(3);
    expect(ana.mergedPrs).toBe(0);
    expect(ana.delivery).toBe(0);
  });
});

describe("scorePeriod: breadth", () => {
  const moduleFiles = (n: number) =>
    Array.from({ length: n }, (_, i) => file(`mod${i}/sub/a.ts`, 40));

  it("awards 15 points per module with at least 30 effective lines", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          files: [
            file("src/features/a.ts", 30),
            file("src-tauri/src/a.rs", 29),
            file("docs/a.md", 20, 20),
            file("README.md", 40),
          ],
        }),
      ],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.modules.sort()).toEqual(["(root)", "docs", "src/features"]);
    expect(ana.breadth).toBe(45);
  });

  it("sums the lines of a module across PRs", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), { files: [file("src/features/a.ts", 20)] }),
        makePr("2", ANA, oct(9), { files: [file("src/features/b.ts", 10)] }),
      ],
    });
    expect(of(score(raw), "Ana").breadth).toBe(15);
  });

  it("caps at 6 modules", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(8), { files: moduleFiles(9) })],
    });
    const ana = of(score(raw), "Ana");
    expect(ana.modules).toHaveLength(6);
    expect(ana.breadth).toBe(90);
  });

  it("ignores ignored files and closed PRs", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), { files: [file("vendor/x/a.ts", 500)] }),
        makePr("2", ANA, oct(9), {
          state: "closed",
          mergedAt: undefined,
          closedAt: oct(9),
          files: [file("src/features/a.ts", 500)],
        }),
      ],
    });
    expect(of(score(raw), "Ana").breadth).toBe(0);
  });

  it("only credits the PR owner", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          landingCoAuthors: [BOB],
          files: [file("src/features/a.ts", 100)],
        }),
      ],
    });
    const scores = score(raw);
    expect(of(scores, "Ana").breadth).toBe(15);
    expect(of(scores, "Bob").breadth).toBe(0);
  });
});

describe("scorePeriod: review", () => {
  const base = () =>
    makeRaw({
      prs: [makePr("1", ANA, oct(8))],
    });

  it("gives 8 plus one per comment, capped at 5 comments", () => {
    const raw = {
      ...base(),
      reviews: [
        makeReview("1", BOB, oct(8), 3, "COMMENTED"),
        makeReview("1", CAT, oct(8), 50, "COMMENTED"),
      ],
    };
    const scores = score(raw);
    expect(of(scores, "Bob").review).toBe(11);
    expect(of(scores, "Cat").review).toBe(13);
    expect(of(scores, "Bob").reviewedPrs).toBe(1);
  });

  it("gives 8 for an approval without comments", () => {
    const raw = { ...base(), reviews: [makeReview("1", BOB, oct(8))] };
    expect(of(score(raw), "Bob").review).toBe(8);
  });

  it("counts several reviews by one reviewer on one PR once", () => {
    const raw = {
      ...base(),
      reviews: [
        makeReview("1", BOB, oct(7), 2, "CHANGES_REQUESTED"),
        makeReview("1", BOB, oct(8), 1, "COMMENTED"),
        makeReview("1", BOB, oct(9), 0, "APPROVED"),
      ],
    };
    const bob = of(score(raw), "Bob");
    expect(bob.review).toBe(8 + 3);
    expect(bob.reviewedPrs).toBe(1);
  });

  it("gives nothing for reviewing your own PR", () => {
    const raw = {
      ...base(),
      reviews: [makeReview("1", ANA, oct(8), 4)],
    };
    const ana = of(score(raw), "Ana");
    expect(ana.review).toBe(0);
    expect(ana.reviewedPrs).toBe(0);
  });

  it("gives nothing to a credited co-author for reviewing the PR", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(8), { landingCoAuthors: [BOB] })],
      reviews: [makeReview("1", BOB, oct(8), 2), makeReview("1", CAT, oct(8))],
    });
    const scores = score(raw);
    expect(of(scores, "Bob").review).toBe(0);
    expect(of(scores, "Cat").review).toBe(8);
  });

  it("does not let an agent-authored PR be reviewed by its human owner", () => {
    const raw = makeRaw({
      prs: [makePr("1", CLAUDE, oct(8), { committer: ANA })],
      reviews: [makeReview("1", ANA, oct(8), 2)],
    });
    expect(of(score(raw), "Ana").review).toBe(0);
  });

  it("matches a login-only reviewer to the person seen on commits", () => {
    const raw = {
      ...base(),
      prs: [...base().prs, makePr("2", BOB, oct(9))],
      reviews: [makeReview("1", ident("bob", "", "bob"), oct(8), 1)],
    };
    const scores = score(raw);
    expect(scores.people).toHaveLength(2);
    expect(of(scores, "Bob").review).toBe(9);
  });

  it("skips bot reviewers, unknown PRs and reviews outside the period", () => {
    const raw = {
      ...base(),
      reviews: [
        makeReview("1", DEPENDABOT, oct(8), 2),
        makeReview("nope", BOB, oct(8), 2),
        makeReview("1", CAT, new Date(2026, 8, 30, 23).toISOString(), 2),
        makeReview("1", CAT, new Date(2026, 10, 1).toISOString(), 2),
      ],
    };
    expect(score(raw).people.map((p) => p.name)).toEqual(["Ana"]);
  });

  it("is 0 and flagged when GitHub is unavailable", () => {
    const raw = {
      ...base(),
      ghAvailable: false,
      reviews: [makeReview("1", BOB, oct(8), 2)],
    };
    const scores = score(raw);
    expect(scores.ghAvailable).toBe(false);
    expect(scores.people.map((p) => p.name)).toEqual(["Ana"]);
    expect(of(scores, "Ana").review).toBe(0);
  });

  it("reports ghAvailable when it is", () => {
    expect(score(base()).ghAvailable).toBe(true);
  });
});

describe("scorePeriod: weights", () => {
  it("multiplies each dimension subtotal", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          files: [file("src/features/a.ts", 100), file("src/a.test.ts", 100)],
          branchCommits: [
            { authoredAt: oct(8), committedAt: oct(8), authors: [ANA] },
          ],
        }),
        makePr("2", BOB, oct(9)),
      ],
      reviews: [makeReview("2", ANA, oct(9), 2)],
      survival: [{ prId: "1", written: 100, alive: 100 }],
    });
    const config = withConfig({
      weights: { delivery: 2, review: 3, care: 4, consistency: 5, breadth: 6 },
    });
    const ana = of(score(raw, config), "Ana");
    expect(ana.score).toBeCloseTo(
      ana.delivery * 2 * ana.quality +
        ana.care * 4 +
        ana.review * 3 +
        ana.consistency * 5 +
        ana.breadth * 6,
      8,
    );
    const plain = of(score(raw), "Ana");
    expect(plain.score).toBeCloseTo(
      plain.deliveryFinal +
        plain.care +
        plain.review +
        plain.consistency +
        plain.breadth,
      8,
    );
    expect(ana.score).toBeGreaterThan(plain.score);
  });

  it("exposes weighted parts that add up to the score exactly", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(8), {
          files: [file("src/features/a.ts", 100), file("src/a.test.ts", 100)],
        }),
      ],
      reviews: [makeReview("1", BOB, oct(9), 2)],
      commits: [makeCommit("c1", BOB, oct(9))],
      survival: [{ prId: "1", written: 100, alive: 50 }],
    });
    const config = withConfig({
      weights: {
        delivery: 1.5,
        review: 0.5,
        care: 3,
        consistency: 2,
        breadth: 0.7,
      },
    });
    for (const person of score(raw, config).people) {
      const { parts } = person;
      expect(person.score).toBe(
        parts.delivery +
          parts.care +
          parts.review +
          parts.consistency +
          parts.breadth,
      );
    }
    const ana = of(score(raw, config), "Ana");
    expect(ana.parts.delivery).toBeCloseTo(ana.deliveryFinal * 1.5, 8);
    expect(ana.parts.care).toBeCloseTo(ana.care * 3, 8);
    expect(ana.parts.breadth).toBeCloseTo(ana.breadth * 0.7, 8);
    expect(of(score(raw, config), "Bob").parts.review).toBeCloseTo(5, 8);
  });

  it("lets a zero weight remove a dimension", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(8))],
    });
    const config = withConfig({
      weights: { delivery: 0, review: 1, care: 1, consistency: 0, breadth: 0 },
    });
    expect(of(score(raw, config), "Ana").score).toBe(0);
  });
});

describe("scorePeriod: AI assistance", () => {
  it("is the share of PRs with an agent author or co-author", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(6), { landingCoAuthors: [CLAUDE] }),
        makePr("2", ANA, oct(7)),
        makePr("3", CLAUDE, oct(8), { committer: ANA }),
        makePr("4", ANA, oct(9), {
          branchCommits: [
            { authoredAt: oct(9), committedAt: oct(9), authors: [ANA, CLAUDE] },
          ],
        }),
      ],
    });
    expect(of(score(raw), "Ana").aiAssistedPct).toBe(75);
  });

  it("looks at the landing commit of the PR", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(6)), makePr("2", ANA, oct(7))],
      commits: [
        makeCommit("c1", ANA, oct(6), { prId: "1", coAuthors: [CLAUDE] }),
        makeCommit("c2", ANA, oct(7), { prId: "2" }),
      ],
    });
    expect(of(score(raw), "Ana").aiAssistedPct).toBe(50);
  });

  it("counts commits that belong to no PR as units", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", ANA, oct(6), { coAuthors: [CLAUDE] }),
        makeCommit("c2", ANA, oct(7)),
        makeCommit("c3", ANA, oct(8)),
        makeCommit("c4", ANA, oct(9)),
      ],
    });
    expect(of(score(raw), "Ana").aiAssistedPct).toBe(25);
  });

  it("does not change the score", () => {
    const withAi = makeRaw({
      prs: [makePr("1", ANA, oct(6), { landingCoAuthors: [CLAUDE] })],
    });
    const without = makeRaw({ prs: [makePr("1", ANA, oct(6))] });
    expect(of(score(withAi), "Ana").score).toBe(
      of(score(without), "Ana").score,
    );
  });

  it("is 0 without units", () => {
    const raw = makeRaw({
      reviews: [makeReview("1", BOB, oct(8))],
      prs: [makePr("1", ANA, new Date(2026, 8, 3).toISOString())],
    });
    expect(of(score(raw), "Bob").aiAssistedPct).toBe(0);
  });
});

describe("scorePeriod: unattributed commits and newcomers", () => {
  it("counts agent commits nobody can own", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", CLAUDE, oct(6), { committer: GITHUB }),
        makeCommit("c2", CLAUDE, oct(7), { committer: GITHUB }),
        makeCommit("c3", CLAUDE, new Date(2026, 8, 7).toISOString(), {
          committer: GITHUB,
        }),
      ],
    });
    const scores = score(raw);
    expect(scores.unattributed.commits).toBe(2);
    expect(scores.people).toEqual([]);
  });

  it("flags people whose first activity is inside the period", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a1", ANA, new Date(2026, 8, 10).toISOString()),
        makeCommit("a2", ANA, oct(5)),
        makeCommit("b1", BOB, oct(6)),
        makeCommit("c1", CAT, new Date(2026, 8, 12).toISOString()),
      ],
    });
    const scores = score(raw);
    expect(scores.newcomers).toEqual(["bob@example.com"]);
  });

  it("does not guess when the raw data does not reach the previous period", () => {
    const raw = makeRaw({
      since: new Date(2026, 9, 1).toISOString(),
      commits: [makeCommit("b1", BOB, oct(6))],
    });
    expect(score(raw).newcomers).toEqual([]);
  });

  it("counts a review as earlier activity", () => {
    const raw = makeRaw({
      prs: [makePr("1", ANA, oct(8))],
      reviews: [
        makeReview("1", BOB, new Date(2026, 8, 20).toISOString()),
        makeReview("1", BOB, oct(8)),
      ],
      commits: [makeCommit("b1", BOB, oct(6))],
    });
    expect(score(raw).newcomers).toEqual(["ana@example.com"]);
  });
});

describe("scorePeriod: output", () => {
  it("sorts people by score then name and carries the period", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", BOB, oct(8)),
        makePr("2", ANA, oct(8)),
        makePr("3", CAT, oct(8), { files: [file("src/a.ts", 5000)] }),
      ],
    });
    const scores = score(raw);
    expect(scores.period.id).toBe("2026-10");
    expect(scores.people.map((p) => p.name)).toEqual(["Cat", "Ana", "Bob"]);
  });

  it("is deterministic", () => {
    const raw = makeRaw({
      prs: [makePr("1", BOB, oct(8)), makePr("2", ANA, oct(9))],
    });
    expect(score(raw)).toEqual(score(raw));
  });
});

describe("attributeCommits", () => {
  const attribute = (raw: RawData, config = DEFAULT_CONFIG) =>
    attributeCommits({ raw, config, resolver: makeResolver(raw, config) });

  it("credits human authors and drops excluded ones", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", ANA, oct(5)),
        makeCommit("c2", DEPENDABOT, oct(5)),
        makeCommit("c3", GITHUB, oct(5)),
      ],
    });
    const out = attribute(raw);
    expect(out.map((a) => [a.commit.hash, a.personId])).toEqual([
      ["c1", "ana@example.com"],
    ]);
  });

  it("credits an agent commit to the owner of its PR", () => {
    const raw = makeRaw({
      prs: [makePr("7", ANA, oct(5))],
      commits: [
        makeCommit("c1", CLAUDE, oct(5), { prId: "7", committer: GITHUB }),
      ],
    });
    const [only] = attribute(raw);
    expect(only.personId).toBe("ana@example.com");
    expect(only.aiAssisted).toBe(true);
  });

  it("falls back to a human committer, then a mapping, then nobody", () => {
    const config = withConfig({
      people: [
        {
          id: "p-dev",
          name: "Dev",
          identities: ["noreply@anthropic.com", "dev@example.com"],
        },
      ],
    });
    const raw = makeRaw({
      commits: [
        makeCommit("c1", CLAUDE, oct(5), { committer: BOB }),
        makeCommit("c2", CLAUDE, oct(6), { committer: GITHUB }),
      ],
    });
    expect(attribute(raw).map((a) => a.personId)).toEqual([
      "bob@example.com",
      null,
    ]);
    expect(attribute(raw, config).map((a) => a.personId)).toEqual([
      "bob@example.com",
      "p-dev",
    ]);
    const lonely = makeRaw({
      commits: [makeCommit("c3", CLAUDE, oct(6), { committer: GITHUB })],
    });
    expect(attribute(lonely)[0].personId).toBeNull();
  });

  it("uses the committer when the PR itself cannot be owned", () => {
    const raw = makeRaw({
      prs: [makePr("7", CLAUDE, oct(5), { committer: GITHUB })],
      commits: [
        makeCommit("c1", CLAUDE, oct(5), { prId: "7", committer: BOB }),
      ],
    });
    expect(attribute(raw)[0].personId).toBe("bob@example.com");
  });

  it("flags agent co-authors and filters files", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", ANA, oct(5), {
          coAuthors: [CLAUDE],
          files: [file("src/a.ts", 5, 1), file("Cargo.lock", 900)],
        }),
        makeCommit("c2", ANA, oct(6), { coAuthors: [BOB] }),
      ],
    });
    const [a, b] = attribute(raw);
    expect(a.aiAssisted).toBe(true);
    expect(a.files).toEqual([file("src/a.ts", 5, 1)]);
    expect(b.aiAssisted).toBe(false);
  });
});

describe("attributePrs", () => {
  it("credits the owner, reports agent-only PRs and drops bot PRs", () => {
    const raw = makeRaw({
      prs: [
        makePr("1", ANA, oct(5)),
        makePr("2", CLAUDE, oct(6), { committer: BOB }),
        makePr("3", CLAUDE, oct(7), { committer: GITHUB }),
        makePr("4", DEPENDABOT, oct(8), { committer: GITHUB }),
      ],
    });
    const out = attributePrs({
      raw,
      config: DEFAULT_CONFIG,
      resolver: makeResolver(raw),
    });
    expect(out.map((o) => [o.pr.id, o.personId])).toEqual([
      ["1", "ana@example.com"],
      ["2", "bob@example.com"],
      ["3", null],
    ]);
  });
});

describe("worked example (scoring-vi.md section 7)", () => {
  // Oct 2026: the 5th is a Monday. The 16 active days contain a run of 9
  // working days (5-9, 12-15); every other active day is isolated.
  const ACTIVE_DAYS = [
    1, 5, 6, 7, 8, 9, 12, 13, 14, 15, 19, 21, 23, 27, 29, 31,
  ];
  const X = ident("Member X", "x@example.com", "member-x");
  const Y = ident("Member Y", "y@example.com", "member-y");

  // Every final diff is 295 effective lines: size = log2(296) = 8.21,
  // 10 + 4 * 8.21 + 9 = 51.84 per PR. PR 5 is net negative (+100 -390),
  // PRs 1 and 2 touch test files. Three modules reach 30 lines.
  const diffs = [
    [
      file("src/features/model/a.ts", 235),
      file("src/features/model/a.test.ts", 60),
    ],
    [file("src-tauri/src/b.rs", 235), file("src-tauri/src/b_test.rs", 60)],
    [file("scripts/ci/c.sh", 295), file("Cargo.lock", 900)],
    [file("src/features/model/d.ts", 295), file("package-lock.json", 5000)],
    [file("src-tauri/src/e.rs", 100, 390)],
    [file("scripts/ci/f.sh", 295)],
  ];

  const branchCommits = (index: number) =>
    Array.from({ length: 7 }, (_, j) => {
      const at = oct(ACTIVE_DAYS[(index * 7 + j) % ACTIVE_DAYS.length], 10);
      return { authoredAt: at, committedAt: at, authors: [X] };
    });

  const mergedAt = (index: number) => oct(ACTIVE_DAYS[index], 18);

  const ownPrs = diffs.map((files, i) =>
    makePr(`x${i + 1}`, X, mergedAt(i), {
      title: `Feature ${i + 1}`,
      commitCount: 7,
      branchCommits: branchCommits(i),
      landingCoAuthors: i % 2 === 0 ? [CLAUDE] : [],
      files,
    }),
  );
  const othersPrs = Array.from({ length: 9 }, (_, i) =>
    makePr(`y${i + 1}`, Y, oct(2, 10 + i), {
      files: [file("src/features/y.ts", 10)],
    }),
  );
  // 14 meaningful comments over 9 PRs, none above the cap of 5 on a PR.
  const reviewComments = [2, 2, 2, 2, 2, 1, 1, 1, 1];
  const reviewer = ident("Member X", "", "member-x");
  const raw = makeRaw({
    prs: [...ownPrs, ...othersPrs],
    commits: ownPrs.map((pr, i) =>
      makeCommit(`land${i}`, X, mergedAt(i), { prId: pr.id }),
    ),
    reviews: [
      ...othersPrs.map((pr, i) =>
        makeReview(pr.id, reviewer, oct(3, 10 + i), reviewComments[i]),
      ),
      makeReview("x1", reviewer, oct(8), 5), // self review: worth nothing
    ],
    // 85% of 6000 written lines are still alive.
    survival: ownPrs.map((pr) => ({ prId: pr.id, written: 1000, alive: 850 })),
  });

  it("matches the documented numbers", () => {
    const x = of(score(raw), "Member X");
    expect(x.mergedPrs).toBe(6);
    expect(x.prs.every((p) => Math.abs(p.size - 8.2) < 0.05)).toBe(true);
    expect(x.prs.every((p) => Math.abs(p.iteration - 9) < 1e-9)).toBe(true);
    expect(x.delivery).toBeGreaterThan(310.8 - 1.5);
    expect(x.delivery).toBeLessThan(310.8 + 1.5);
    expect(x.survival).toBeCloseTo(0.85, 10);
    expect(x.quality).toBeCloseTo(1.14, 10);
    expect(x.deliveryFinal).toBeGreaterThan(354.3 - 1.5);
    expect(x.deliveryFinal).toBeLessThan(354.3 + 1.5);
    expect(Math.abs(x.care - 28.5)).toBeLessThan(1);
    expect(x.review).toBe(86);
    expect(x.reviewedPrs).toBe(9);
    expect(x.activeDays).toBe(16);
    expect(x.streak).toBe(9);
    expect(x.consistency).toBe(98);
    expect(x.modules.sort()).toEqual([
      "scripts/ci",
      "src-tauri/src",
      "src/features",
    ]);
    expect(x.breadth).toBe(45);
    expect(Math.abs(x.score - 612)).toBeLessThan(3);
    expect(x.parts.delivery).toBeCloseTo(x.deliveryFinal, 10);
    expect(x.aiAssistedPct).toBe(50);
    expect(x.netNegativePrs).toBe(1);
    expect(x.mainCommits).toBe(6);
    expect(x.branchCommits).toBe(42);
  });
});

describe("scorePeriod: period kinds", () => {
  const scoreIn = (raw: RawData, period: Period, now: Date) =>
    scorePeriod({
      raw,
      config: DEFAULT_CONFIG,
      resolver: makeResolver(raw, DEFAULT_CONFIG),
      period,
      now,
    });
  const at = (month: number, day: number) =>
    new Date(2026, month, day, 12).toISOString();
  const commit = (id: string, iso: string) =>
    makeCommit(id, ANA, iso, { files: [file("src/a.ts", 5)] });

  it("scores only the last 7 days for 7d", () => {
    const now = new Date(2026, 9, 20, 10);
    const raw = makeRaw({
      commits: [
        commit("old", oct(13)),
        commit("a", oct(14)),
        commit("b", oct(20)),
      ],
    });
    const scores = scoreIn(raw, currentPeriod("7d", now), now);
    expect(scores.period.id).toBe("7d");
    expect(of(scores, "Ana").activeDays).toBe(2);
  });

  it("counts every active day of a quarter and of a year", () => {
    const now = new Date(2026, 11, 20, 10);
    const raw = makeRaw({
      commits: [
        commit("q3", at(8, 30)),
        commit("q4a", at(9, 1)),
        commit("q4b", at(10, 15)),
        commit("q4c", at(11, 19)),
      ],
    });
    expect(
      of(scoreIn(raw, currentPeriod("quarter", now), now), "Ana").activeDays,
    ).toBe(3);
    expect(
      of(scoreIn(raw, currentPeriod("year", now), now), "Ana").activeDays,
    ).toBe(4);
  });

  it("flags newcomers against the previous period of the same kind", () => {
    const now = new Date(2026, 9, 20, 10);
    const week = currentPeriod("7d", now);
    const raw = makeRaw({
      since: new Date(2026, 9, 7).toISOString(),
      commits: [commit("a", oct(15))],
    });
    expect(scoreIn(raw, week, now).newcomers).toEqual(["ana@example.com"]);
    const short = makeRaw({
      since: new Date(2026, 9, 14).toISOString(),
      commits: [commit("a", oct(15))],
    });
    expect(scoreIn(short, week, now).newcomers).toEqual([]);
  });
});
