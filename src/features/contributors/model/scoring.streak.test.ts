import { beforeEach, describe, expect, it, vi } from "vitest";

// Wraps the real function so the calls can be counted without changing it.
vi.mock("./period", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./period")>();
  return { ...actual, periodDayKeys: vi.fn(actual.periodDayKeys) };
});

import { DEFAULT_CONFIG } from "./config";
import { currentPeriod, periodDayKeys } from "./period";
import { scorePeriod, streakOf } from "./scoring";
import {
  ANA,
  BOB,
  CAT,
  makeCommit,
  makeRaw,
  makeResolver,
  oct,
} from "./testFixtures";
import type { GitIdentity } from "./types";

const NOW = new Date(2026, 9, 20, 10);

const run = (
  days: Record<string, number[]>,
  kind: "month" | "all" = "month",
) => {
  const who: Record<string, GitIdentity> = { ana: ANA, bob: BOB, cat: CAT };
  const raw = makeRaw({
    since: new Date(2026, 7, 1).toISOString(),
    commits: Object.entries(days).flatMap(([name, list]) =>
      list.map((d) => makeCommit(`${name}${d}`, who[name], oct(d))),
    ),
  });
  const period = currentPeriod(kind, NOW, new Date(2026, 8, 25));
  return {
    period,
    scores: scorePeriod({
      raw,
      config: DEFAULT_CONFIG,
      resolver: makeResolver(raw, DEFAULT_CONFIG),
      period,
      now: NOW,
    }),
  };
};

beforeEach(() => {
  vi.mocked(periodDayKeys).mockClear();
});

describe("scorePeriod streaks", () => {
  const days = {
    ana: [1, 2, 5, 6, 7, 8, 9],
    bob: [1, 5, 6],
    cat: [12, 13, 14, 15, 16, 19],
  };

  it("builds the period's day keys once, however many people there are", () => {
    const { scores } = run(days);
    expect(scores.people).toHaveLength(3);
    expect(periodDayKeys).toHaveBeenCalledTimes(1);
  });

  it("gives every person the streak streakOf finds for their days", () => {
    const expected = { Ana: 7, Bob: 2, Cat: 6 };
    for (const kind of ["month", "all"] as const) {
      const { scores, period } = run(days, kind);
      for (const person of scores.people) {
        const key = person.name.toLowerCase() as keyof typeof days;
        const active = days[key].map(
          (d) => `2026-10-${String(d).padStart(2, "0")}`,
        );
        expect(person.streak).toBe(streakOf(active, period, NOW));
        expect(person.streak).toBe(
          expected[person.name as keyof typeof expected],
        );
      }
    }
  });
});
