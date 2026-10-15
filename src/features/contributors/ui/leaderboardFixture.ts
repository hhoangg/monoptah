// A small team in October 2026 shared by the leaderboard tests. Not imported by app code.
import { DEFAULT_CONFIG } from "../model/config";
import { buildContributorsModel } from "../model/pipeline";
import type { ContributorsModel } from "../model/pipeline";
import { sinceFor } from "../model/range";
import {
  ANA,
  BOB,
  CAT,
  file,
  ident,
  makeCommit,
  makePr,
  makeRaw,
  makeReview,
  oct,
} from "../model/testFixtures";
import type { ContributorsConfig, RawData } from "../model/types";

export const NOW = new Date(2026, 9, 20, 10);

const DAN = ident("Dan", "dan@example.com", "dan");

const sept = (day: number) => new Date(2026, 8, day, 12).toISOString();

/**
 * October: Ana (the git user) 1st with a merged PR, Bob 2nd, Cat 3rd, Dan 4th
 * (new). September: Bob 1st, Ana 2nd, Cat 3rd, Dan absent. So the movements
 * are Ana up 1, Bob down 1, Cat unchanged, Dan new.
 */
export function leaderboardRaw(extra: Partial<RawData> = {}): RawData {
  const days = (
    prefix: string,
    who: typeof ANA,
    list: number[],
    at: (d: number) => string,
  ) =>
    list.map((d) =>
      makeCommit(`${prefix}${d}`, who, at(d), {
        files: [file("src/features/a.ts", 5)],
      }),
    );
  return makeRaw({
    since: sinceFor("30d", NOW),
    commits: [
      ...days("ana-o", ANA, [1, 2, 5, 6, 7, 8], oct),
      ...days("bob-o", BOB, [1, 2, 5, 6], oct),
      ...days("cat-o", CAT, [1, 2, 5], oct),
      ...days("dan-o", DAN, [1], oct),
      ...days("bob-s", BOB, [1, 2, 3, 4, 7], sept),
      ...days("ana-s", ANA, [1, 2, 3], sept),
      ...days("cat-s", CAT, [1, 2], sept),
    ],
    prs: [
      makePr("1", ANA, oct(8), {
        commitCount: 3,
        files: [
          file("src/features/a.ts", 100),
          file("src/features/a.test.ts", 20),
        ],
      }),
    ],
    reviews: [makeReview("1", BOB, oct(9), 3)],
    survival: [{ prId: "1", written: 100, alive: 85 }],
    ...extra,
  });
}

export function leaderboardModel(
  overrides: Partial<Parameters<typeof buildContributorsModel>[0]> & {
    config?: ContributorsConfig;
  } = {},
): ContributorsModel {
  return buildContributorsModel({
    raw: leaderboardRaw(),
    config: DEFAULT_CONFIG,
    mailmap: [],
    merge: true,
    range: "30d",
    now: NOW,
    user: { name: "Ana", email: "ana@example.com" },
    ...overrides,
  });
}
