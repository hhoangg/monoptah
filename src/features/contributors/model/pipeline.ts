import { buildLeaderboard } from "./leaderboard";
import type { Leaderboard } from "./leaderboard";
import { buildResolver, identityKey } from "./identity";
import type { MailmapPerson, Resolver } from "./identity";
import { personColorMap } from "./palette";
import {
  isInPeriod,
  previousComparableSpan,
  previousPeriod,
  syncWindow,
} from "./period";
import type { Period } from "./period";
import { observedIdentities, scorePeriod } from "./scoring";
import type { PeriodScores } from "./scoring";
import { bucketGranularity, buildStats } from "./stats";
import type { Stats } from "./stats";
import { personRows, unmappedIdentities } from "./unmapped";
import type { UnmappedIdentity } from "./unmapped";
import type { ContributorsConfig, GitIdentity, RawData } from "./types";

/** What the tabs render; derived from raw data, config and the toggles. */
export type ContributorsModel = {
  raw: RawData;
  config: ContributorsConfig;
  mailmap: MailmapPerson[];
  merge: boolean;
  resolver: Resolver;
  /** The selected period; Stats and the Leaderboard both show it. */
  period: Period;
  /** The period of the same kind right before; KPI deltas and rank movement compare with it. */
  previousPeriod: Period;
  current: PeriodScores;
  previous: PeriodScores;
  leaderboard: Leaderboard;
  stats: Stats;
  unmapped: UnmappedIdentity[];
  /**
   * One color per person, shared by every tab so a person looks the same on
   * the chart, the board and the Identities list. Built from everyone in the
   * data, not from the chosen period.
   */
  colors: Record<string, string>;
  /** Person id of the repo's git user, when they appear in the data. */
  meId: string | undefined;
  /** Anything to show in the chosen period. */
  hasActivity: boolean;
};

/** Pure; the hook memoizes it, so rescoring never touches the network. */
export function buildContributorsModel(input: {
  raw: RawData;
  config: ContributorsConfig;
  mailmap: MailmapPerson[];
  merge: boolean;
  period: Period;
  now: Date;
  user?: { name: string; email: string };
}): ContributorsModel {
  const { raw, config, mailmap, merge, period, now, user } = input;
  const observed = observedIdentities(raw);
  const resolver = buildResolver(config, {
    mailmap,
    observedIdentities: observed,
    merge,
  });
  const before = previousPeriod(period);
  const current = scorePeriod({ raw, config, resolver, period, now });
  const previous = scorePeriod({
    raw,
    config,
    resolver,
    period: before,
    now,
  });

  // Only an identity seen in the data resolves, so the resolver does not grow
  // a person for a git user who never committed here.
  const email = user?.email.trim().toLowerCase();
  const mine: GitIdentity | undefined = email
    ? observed.find((o) => identityKey(o) === email)
    : undefined;
  const meId = mine ? resolver.personOf(mine) : undefined;

  const colors = personColorMap([
    ...personRows({ raw, config, resolver }).map((p) => p.id),
    ...current.people.map((p) => p.personId),
    ...previous.people.map((p) => p.personId),
  ]);
  const stats = buildStats({
    raw,
    config,
    resolver,
    rangeStart: period.start,
    // The current period runs to today, so the chart has no empty future buckets.
    rangeEnd: syncWindow(period, now).to,
    // Same elapsed days of the previous period, so a partly elapsed period is
    // not compared with a whole one. Rank movement still uses the whole of it.
    previous: previousComparableSpan(period, now),
    granularity: bucketGranularity(period),
    colors,
  });

  return {
    raw,
    config,
    mailmap,
    merge,
    resolver,
    period,
    previousPeriod: before,
    current,
    previous,
    leaderboard: buildLeaderboard(current, previous, { meId }),
    stats,
    unmapped: unmappedIdentities({ raw, config, mailmap }),
    colors,
    meId,
    hasActivity:
      raw.commits.some((c) => isInPeriod(period, c.committedAt)) ||
      stats.kpis.prsMerged.value > 0,
  };
}
