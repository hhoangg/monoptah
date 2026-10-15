import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import { buildLeaderboard } from "./leaderboard";
import { SCORING, scorePeriod } from "./scoring";
import {
  buildAgentPrompt,
  buildSnapshot,
  snapshotFileName,
  suggestedQuestions,
} from "./snapshot";
import type { SnapshotScope } from "./snapshot";
import { buildStats } from "./stats";
import {
  ANA,
  BOB,
  CLAUDE,
  PERIOD,
  PERIOD_NOW,
  file,
  makeCommit,
  makePr,
  makeRaw,
  makeResolver,
  makeReview,
  oct,
  withConfig,
} from "./testFixtures";
import type { ContributorsConfig, RawData } from "./types";

const NOW = new Date(2026, 9, 15, 9, 30);

function build(
  raw: RawData,
  scope: SnapshotScope,
  include = { perPr: false, rawCommits: false },
  config: ContributorsConfig = DEFAULT_CONFIG,
  withStats = false,
) {
  const resolver = makeResolver(raw, config);
  const scores = scorePeriod({
    raw,
    config,
    resolver,
    period: PERIOD,
    now: PERIOD_NOW,
  });
  const leaderboard = buildLeaderboard(scores, null, {});
  return buildSnapshot({
    project: "monoptah",
    branch: "main",
    period: PERIOD,
    scores,
    leaderboard,
    stats: withStats
      ? buildStats({
          raw,
          config,
          resolver,
          rangeStart: PERIOD.start,
          rangeEnd: PERIOD.end,
        })
      : undefined,
    config,
    scope,
    include,
    raw,
    ghAvailable: raw.ghAvailable,
    resolver,
    now: NOW,
  });
}

const sample = () =>
  makeRaw({
    prs: [
      makePr("1", ANA, oct(8), {
        title: "fix: crash",
        landingCoAuthors: [CLAUDE],
        files: [file("src/a.ts", 100), file("src/a.test.ts", 20)],
      }),
      makePr("2", BOB, oct(9)),
    ],
    commits: [
      makeCommit("aaaaaaaaaaaaaaaaaaaa", ANA, oct(8), {
        prId: "1",
        files: [file("src/a.ts", 1), file("Cargo.lock", 500)],
      }),
      makeCommit("bbbbbbbbbbbbbbbbbbbb", BOB, oct(9), { prId: "2" }),
    ],
    reviews: [makeReview("1", BOB, oct(8), 2)],
  });

const ANA_ID = "ana@example.com";

describe("snapshotFileName", () => {
  it("names the file after the period", () => {
    expect(snapshotFileName(PERIOD)).toBe("contributors-2026-10.json");
  });
});

describe("suggestedQuestions", () => {
  it("has the team suggestions", () => {
    expect(suggestedQuestions({ kind: "team" })).toEqual([
      "Summarize this period: who drove what, and where are the risks?",
      "Which modules depend on a single person (bus factor)?",
      "Compare this period with the last 3 and explain the trend.",
      "Are any scores inflated by generated code or huge PRs?",
    ]);
  });

  it("has the suggestions about yourself", () => {
    expect(suggestedQuestions({ kind: "me", personId: "x" })).toEqual([
      "Why did my rank change since the previous period?",
      "What would raise my Quality factor?",
      "Which of my PRs had the most rework, and why?",
    ]);
  });

  it("interpolates the name for a person", () => {
    expect(
      suggestedQuestions({ kind: "person", personId: "x" }, "Tuan Nguyen"),
    ).toEqual([
      "Summarize Tuan Nguyen's contributions this period.",
      "What are Tuan Nguyen's strengths across the 5 dimensions?",
      "Show Tuan Nguyen's PRs with the most iteration and what slowed them down.",
      "Is Tuan Nguyen's Delivery low because of review or mentoring work?",
    ]);
  });

  it("falls back to a neutral name", () => {
    expect(suggestedQuestions({ kind: "person", personId: "x" })[0]).toBe(
      "Summarize this person's contributions this period.",
    );
  });
});

describe("buildAgentPrompt", () => {
  const base = {
    question: "Why is Ana first?",
    fileName: "contributors-2026-10.json",
    projectName: "monoptah",
    periodLabel: "Oct 2026",
  };

  it("states project, period, file and question", () => {
    const text = buildAgentPrompt({
      ...base,
      scope: { kind: "team" },
      allowTools: true,
    });
    expect(text).toContain("monoptah");
    expect(text).toContain("Oct 2026");
    expect(text).toContain("contributors-2026-10.json");
    expect(text).toContain("exact data");
    expect(text).toContain("Why is Ana first?");
    expect(text).toContain("the whole team");
  });

  it("allows read-only git and gh when asked", () => {
    const text = buildAgentPrompt({
      ...base,
      scope: { kind: "team" },
      allowTools: true,
    });
    expect(text).toContain(
      "You may run read-only git and gh commands inside this project to dig deeper",
    );
    expect(text).not.toContain("Do not run commands");
  });

  it("forbids commands otherwise", () => {
    const text = buildAgentPrompt({
      ...base,
      scope: { kind: "team" },
      allowTools: false,
    });
    expect(text).toContain(
      "Do not run commands; answer only from the attached snapshot",
    );
    expect(text).not.toContain("You may run");
  });

  it("names the person in scope", () => {
    const person = buildAgentPrompt({
      ...base,
      scope: { kind: "person", personId: ANA_ID },
      personName: "Ana",
      allowTools: false,
    });
    expect(person).toContain("Ana");
    expect(person).toContain(ANA_ID);
    const me = buildAgentPrompt({
      ...base,
      scope: { kind: "me", personId: ANA_ID },
      personName: "Ana",
      allowTools: false,
    });
    expect(me).toContain("their own score");
    expect(me).toContain(ANA_ID);
  });
});

describe("buildSnapshot", () => {
  it("describes the project, period and generation time", () => {
    const snap = build(sample(), { kind: "team" });
    expect(snap.generatedAt).toBe(NOW.toISOString());
    expect(snap.project).toBe("monoptah");
    expect(snap.branch).toBe("main");
    expect(snap.period).toEqual({
      kind: "month",
      id: "2026-10",
      label: "Oct 2026",
      start: PERIOD.start.toISOString(),
      end: PERIOD.end.toISOString(),
    });
    expect(snap.scope).toEqual({ kind: "team" });
    expect(snap.ghAvailable).toBe(true);
  });

  it("carries the formula in use", () => {
    const config = withConfig({
      weights: { delivery: 2, review: 1, care: 1, consistency: 1, breadth: 1 },
      ignore: ["dist/**"],
    });
    const { formula } = build(
      sample(),
      { kind: "team" },
      { perPr: false, rawCommits: false },
      config,
    );
    expect(formula.constants).toEqual(SCORING);
    expect(formula.weights).toEqual(config.weights);
    expect(formula.ignore).toEqual(["dist/**"]);
    expect(formula.description.length).toBeGreaterThan(5);
    expect(formula.description.join(" ")).toContain("Delivery");
  });

  it("lists ranked people with their breakdown", () => {
    const snap = build(sample(), { kind: "team" });
    expect(snap.people.map((p) => p.name)).toEqual(["Ana", "Bob"]);
    const ana = snap.people[0];
    expect(ana.rank).toBe(1);
    expect(ana.movement).toBeNull();
    expect(ana.personId).toBe(ANA_ID);
    expect(ana.identities).toEqual([ANA_ID]);
    expect(ana.aiAssistedPct).toBe(100);
    expect(ana.breakdown).toMatchObject({
      quality: 1,
      survival: null,
    });
    expect(ana.breakdown.delivery).toBeGreaterThan(0);
    expect(ana.breakdown.care).toBeGreaterThan(0);
    expect(snap.people[1].breakdown.review).toBe(10);
    expect(ana.mergedPrs).toBe(1);
    expect(ana.badges).toEqual(expect.any(Array));
  });

  it("rounds numbers to 2 decimals", () => {
    const snap = build(sample(), { kind: "team" });
    const score = snap.people[0].score;
    expect(Math.round(score * 100) / 100).toBe(score);
  });

  it("sets focus only for a person or me scope", () => {
    expect("focus" in build(sample(), { kind: "team" })).toBe(false);
    const person = build(sample(), { kind: "person", personId: ANA_ID });
    expect(person.focus?.name).toBe("Ana");
    const me = build(sample(), { kind: "me", personId: "nobody" });
    expect(me.focus).toBeNull();
  });

  it("includes PRs only when asked", () => {
    expect("prs" in build(sample(), { kind: "team" })).toBe(false);
    const snap = build(
      sample(),
      { kind: "team" },
      { perPr: true, rawCommits: false },
    );
    expect(snap.prs?.map((p) => [p.personId, p.id])).toEqual([
      [ANA_ID, "1"],
      ["bob@example.com", "2"],
    ]);
    expect(snap.prs?.[0]).toMatchObject({
      personName: "Ana",
      title: "fix: crash",
      fix: true,
      testFiles: true,
      aiAssisted: true,
    });
  });

  it("limits PRs to the focus person", () => {
    const snap = build(
      sample(),
      { kind: "person", personId: ANA_ID },
      { perPr: true, rawCommits: false },
    );
    expect(snap.prs?.map((p) => p.id)).toEqual(["1"]);
  });

  it("lists only the focus person's commits for a person scope", () => {
    const snap = build(
      sample(),
      { kind: "person", personId: ANA_ID },
      { perPr: false, rawCommits: true },
    );
    expect(snap.commits?.map((c) => c.person)).toEqual(["Ana"]);
    expect(snap.people.map((p) => p.name)).toEqual(["Ana", "Bob"]);
  });

  it("includes weighted parts that add up to the score, and a weights note", () => {
    const config = withConfig({
      weights: { delivery: 2, review: 1, care: 1, consistency: 1, breadth: 1 },
    });
    const snap = build(
      sample(),
      { kind: "team" },
      { perPr: false, rawCommits: false },
      config,
    );
    const ana = snap.people[0];
    const sum = Object.values(ana.parts).reduce((a, b) => a + b, 0);
    expect(sum).toBeCloseTo(ana.score, 1);
    expect(ana.parts.delivery).toBeCloseTo(ana.breakdown.deliveryFinal * 2, 1);
    expect(snap.notes.join(" ")).toContain("Custom weights");
    expect(build(sample(), { kind: "team" }).notes.join(" ")).not.toContain(
      "Custom weights",
    );
  });

  it("rounds each part and sums the rounded parts into the score", () => {
    const config = withConfig({
      weights: {
        delivery: 1.37,
        review: 0.33,
        care: 2.71,
        consistency: 1.13,
        breadth: 0.77,
      },
    });
    const snap = build(
      sample(),
      { kind: "team" },
      { perPr: false, rawCommits: false },
      config,
    );
    for (const person of snap.people) {
      const sum = Object.values(person.parts).reduce((a, b) => a + b, 0);
      expect(Math.round(sum * 100) / 100).toBe(person.score);
      for (const part of Object.values(person.parts)) {
        expect(Math.round(part * 100) / 100).toBe(part);
      }
    }
  });

  it("includes in-period commits only when asked", () => {
    expect("commits" in build(sample(), { kind: "team" })).toBe(false);
    const snap = build(
      sample(),
      { kind: "team" },
      { perPr: false, rawCommits: true },
    );
    expect(snap.commits).toEqual([
      {
        hash: "bbbbbbbbbbbb",
        author: "Bob <bob@example.com>",
        person: "Bob",
        date: oct(9),
        files: 0,
      },
      {
        hash: "aaaaaaaaaaaa",
        author: "Ana <ana@example.com>",
        person: "Ana",
        date: oct(8),
        files: 1,
      },
    ]);
  });

  it("caps commits at 2000 and says so", () => {
    const raw = makeRaw({
      commits: Array.from({ length: 2100 }, (_, i) =>
        makeCommit(`h${i}`, ANA, oct(1 + (i % 28))),
      ),
    });
    const snap = build(
      raw,
      { kind: "team" },
      { perPr: false, rawCommits: true },
    );
    expect(snap.commits).toHaveLength(2000);
    expect(snap.notes.join(" ")).toContain("2000");
  });

  it("adds stats when given", () => {
    const snap = build(
      sample(),
      { kind: "team" },
      { perPr: false, rawCommits: false },
      DEFAULT_CONFIG,
      true,
    );
    expect(snap.stats?.kpis.commits.value).toBe(2);
    expect(snap.stats?.rows.map((r) => r.name)).toEqual(["Ana", "Bob"]);
    expect("stats" in build(sample(), { kind: "team" })).toBe(false);
  });

  it("notes when the GitHub CLI is unavailable", () => {
    const raw = {
      ...sample(),
      ghAvailable: false,
      ghNote: "gh is not installed",
    };
    const snap = build(raw, { kind: "team" });
    expect(snap.ghAvailable).toBe(false);
    expect(snap.notes).toContain(
      "Review points are not counted because the GitHub CLI is unavailable (gh is not installed)",
    );
  });

  it("notes unattributed work and truncated history", () => {
    const raw = {
      ...sample(),
      truncated: true,
      commits: [
        makeCommit("c", CLAUDE, oct(6), {
          committer: { name: "GitHub", email: "noreply@github.com" },
        }),
      ],
    };
    const notes = build(raw, { kind: "team" }).notes.join(" ");
    expect(notes).toContain("1 commit");
    expect(notes).toContain("safety cap");
  });

  it("is plain JSON without dashes that are not hyphens", () => {
    const snap = build(
      sample(),
      { kind: "me", personId: ANA_ID },
      { perPr: true, rawCommits: true },
      DEFAULT_CONFIG,
      true,
    );
    const text = JSON.stringify(snap);
    expect(JSON.parse(text)).toEqual(snap);
    expect(text).not.toMatch(new RegExp("[\\u2013\\u2014]"));
  });
});
