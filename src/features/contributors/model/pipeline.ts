import { buildLeaderboard } from "./leaderboard";
import type { Leaderboard } from "./leaderboard";
import { buildResolver, identityKey } from "./identity";
import type { MailmapPerson, Resolver } from "./identity";
import { personColorMap } from "./palette";
import { rangeBounds, rangeDays, rangeLabel } from "./range";
import type { RangeValue } from "./range";
import { observedIdentities, scoreSeason } from "./scoring";
import type { SeasonScores } from "./scoring";
import { previousSeason, seasonById, seasonOf } from "./season";
import type { Season } from "./season";
import { buildStats } from "./stats";
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
  range: RangeValue;
  resolver: Resolver;
  season: Season;
  /** Id of the month of `now`, whatever season is ranked. */
  currentSeasonId: string;
  previousSeason: Season;
  /** The ranked season: the current month unless another was selected. */
  current: SeasonScores;
  previous: SeasonScores;
  leaderboard: Leaderboard;
  stats: Stats;
  rangeStart: Date;
  rangeEnd: Date;
  /** `7d`, `30d`, or the two days of a custom range. */
  rangeLabel: string;
  /** Whole days in the range; the previous period is as long. */
  rangeDays: number;
  unmapped: UnmappedIdentity[];
  /**
   * One color per person, shared by every tab so a person looks the same on
   * the chart, the board and the Identities list. Built from everyone in the
   * data, not from the chosen range or season.
   */
  colors: Record<string, string>;
  /** Person id of the repo's git user, when they appear in the data. */
  meId: string | undefined;
  /** Anything to show in the chosen range. */
  hasActivity: boolean;
};

const inRange = (start: Date, end: Date, iso: string | undefined) => {
  if (!iso) return false;
  const t = Date.parse(iso);
  return t >= start.getTime() && t < end.getTime();
};

/** Pure; the hook memoizes it, so rescoring never touches the network. */
export function buildContributorsModel(input: {
  raw: RawData;
  config: ContributorsConfig;
  mailmap: MailmapPerson[];
  merge: boolean;
  range: RangeValue;
  now: Date;
  user?: { name: string; email: string };
  /** The season to rank, e.g. "2026-09"; the month of `now` when absent or unknown. */
  seasonId?: string;
}): ContributorsModel {
  const { raw, config, mailmap, merge, range, now, user, seasonId } = input;
  const observed = observedIdentities(raw);
  const resolver = buildResolver(config, {
    mailmap,
    observedIdentities: observed,
    merge,
  });
  const season = (seasonId && seasonById(seasonId)) || seasonOf(now);
  const before = previousSeason(season);
  const current = scoreSeason({ raw, config, resolver, season });
  const previous = scoreSeason({ raw, config, resolver, season: before });

  // Only an identity seen in the data resolves, so the resolver does not grow
  // a person for a git user who never committed here.
  const email = user?.email.trim().toLowerCase();
  const mine: GitIdentity | undefined = email
    ? observed.find((o) => identityKey(o) === email)
    : undefined;
  const meId = mine ? resolver.personOf(mine) : undefined;

  const { start, end } = rangeBounds(range, now);
  const colors = personColorMap([
    ...personRows({ raw, config, resolver }).map((p) => p.id),
    ...current.people.map((p) => p.personId),
    ...previous.people.map((p) => p.personId),
  ]);
  const stats = buildStats({
    raw,
    config,
    resolver,
    rangeStart: start,
    rangeEnd: end,
    colors,
  });

  return {
    raw,
    config,
    mailmap,
    merge,
    range,
    resolver,
    season,
    currentSeasonId: seasonOf(now).id,
    previousSeason: before,
    current,
    previous,
    leaderboard: buildLeaderboard(current, previous, { meId }),
    stats,
    rangeStart: start,
    rangeEnd: end,
    rangeLabel: rangeLabel(range, now),
    rangeDays: rangeDays(range, now),
    unmapped: unmappedIdentities({ raw, config, mailmap }),
    colors,
    meId,
    hasActivity:
      raw.commits.some((c) => inRange(start, end, c.committedAt)) ||
      stats.kpis.prsMerged.value > 0,
  };
}
