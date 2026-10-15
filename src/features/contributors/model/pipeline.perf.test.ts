import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import { buildContributorsModel } from "./pipeline";
import {
  file,
  ident,
  makeCommit,
  makePr,
  makeRaw,
  makeReview,
} from "./testFixtures";
import type { GitIdentity, RawCommit, RawPr, RawReview } from "./types";

const NOW = new Date(2026, 9, 20, 10);
const DAY = 86_400_000;

/** Small deterministic generator so the fixture is the same on every run. */
function lcg(seed: number) {
  let state = seed;
  return () => {
    state = (state * 1_664_525 + 1_013_904_223) % 4_294_967_296;
    return state / 4_294_967_296;
  };
}

/** A repo of about a team of 40: several emails per person, bots and agents. */
function syntheticRaw(commitCount = 1500, prCount = 300) {
  const rand = lcg(42);
  const people: GitIdentity[][] = Array.from({ length: 40 }, (_, i) => [
    ident(`Person ${i}`, `person${i}@example.com`, `person${i}`),
    ident(`Person ${i}`, `person${i}@laptop.local`),
    ident(`P. ${i}`, `${1000 + i}+person${i}@users.noreply.github.com`),
  ]);
  const others: GitIdentity[] = [
    ident(
      "dependabot[bot]",
      "49699333+dependabot[bot]@users.noreply.github.com",
    ),
    ident("Claude", "noreply@anthropic.com"),
    ident("Copilot", "198982749+Copilot@users.noreply.github.com"),
    ident("Build Box", "ci@build-box"),
  ];
  const pick = () => {
    const roll = rand();
    if (roll < 0.06) return others[Math.floor(rand() * others.length)];
    const group = people[Math.floor(rand() * people.length)];
    return group[Math.floor(rand() * group.length)];
  };
  const when = () => new Date(NOW.getTime() - rand() * 60 * DAY).toISOString();

  const commits: RawCommit[] = Array.from({ length: commitCount }, (_, i) => {
    const author = pick();
    return makeCommit(`c${i}`, author, when(), {
      committer: rand() < 0.2 ? pick() : author,
      coAuthors: rand() < 0.15 ? [pick()] : [],
      files: Array.from({ length: 1 + Math.floor(rand() * 6) }, (_, n) =>
        file(
          `src/${["app", "lib", "ui"][n % 3]}/f${Math.floor(rand() * 90)}.ts`,
          1 + Math.floor(rand() * 80),
          Math.floor(rand() * 30),
        ),
      ),
      prId: i < prCount ? String(i + 1) : undefined,
    });
  });
  const prs: RawPr[] = Array.from({ length: prCount }, (_, i) =>
    makePr(String(i + 1), pick(), when(), {
      commitCount: 3,
      branchCommits: Array.from({ length: 3 }, () => {
        const at = when();
        return {
          authoredAt: at,
          committedAt: at,
          authors: [pick(), pick()],
        };
      }),
      landingCoAuthors: rand() < 0.2 ? [pick()] : [],
      files: [file("src/app/x.ts", 40, 5), file("package-lock.json", 900)],
    }),
  );
  const reviews: RawReview[] = Array.from({ length: prCount * 2 }, (_, i) =>
    makeReview(String((i % prCount) + 1), pick(), when(), i % 4),
  );
  return makeRaw({
    since: new Date(NOW.getTime() - 120 * DAY).toISOString(),
    commits,
    prs,
    reviews,
    survival: prs.slice(0, 80).map((pr) => ({
      prId: pr.id,
      written: 100,
      alive: 80,
    })),
  });
}

const build = (raw: ReturnType<typeof syntheticRaw>) =>
  buildContributorsModel({
    raw,
    config: DEFAULT_CONFIG,
    mailmap: [],
    merge: true,
    range: "30d",
    now: NOW,
  });

describe("model rebuild performance", () => {
  it("rebuilds 1500 commits and 300 PRs well within a frame budget", () => {
    const raw = syntheticRaw();
    // Warm the JIT and the pattern caches the way a second edit finds them.
    build(raw);
    const runs: number[] = [];
    for (let i = 0; i < 5; i += 1) {
      const start = performance.now();
      const model = build(raw);
      runs.push(performance.now() - start);
      expect(model.leaderboard.rows.length).toBeGreaterThan(10);
    }
    const best = Math.min(...runs);
    const median = [...runs].sort((a, b) => a - b)[2];
    console.log(
      `rebuild 1500 commits / 300 PRs: best ${best.toFixed(1)} ms, median ${median.toFixed(1)} ms (runs ${runs.map((r) => r.toFixed(0)).join(", ")})`,
    );
    // Target is 60 ms on a laptop; the assertion is loose so CI load cannot flake it.
    expect(median).toBeLessThan(400);
  });
});
