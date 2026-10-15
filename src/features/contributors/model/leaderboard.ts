import type { PersonScore, PeriodScores } from "./scoring";

export type BadgeId =
  "streak" | "builder" | "cleaner" | "bug-hunter" | "reviewer" | "newcomer";

/** `icon` is a glyph key; the UI maps it to an icon. */
export type Badge = { id: BadgeId; label: string; icon: string };

export type LeaderboardRow = PersonScore & {
  rank: number;
  movement: number | null; // previous rank - rank, positive = moved up
  badges: Badge[];
  isMe: boolean;
};

const STREAK_BADGE_DAYS = 10;
const MIN_BADGE_PRS = 2;
const MAX_BADGES = 3;
const TREND_PERIODS = 6;

const BADGES: Record<BadgeId, Badge> = {
  streak: { id: "streak", label: "Streak", icon: "fire" },
  builder: { id: "builder", label: "Builder", icon: "hammer" },
  cleaner: { id: "cleaner", label: "Cleaner", icon: "broom" },
  "bug-hunter": { id: "bug-hunter", label: "Bug hunter", icon: "bug" },
  reviewer: { id: "reviewer", label: "Reviewer", icon: "eyes" },
  newcomer: { id: "newcomer", label: "Newcomer", icon: "sprout" },
};

const hasActivity = (p: PersonScore) =>
  p.score > 0 ||
  p.activeDays > 0 ||
  p.mergedPrs > 0 ||
  p.mainCommits > 0 ||
  p.reviewedPrs > 0;

function ranked(people: PersonScore[]): PersonScore[] {
  return people
    .filter(hasActivity)
    .sort(
      (a, b) =>
        Math.round(b.score) - Math.round(a.score) ||
        b.mergedPrs - a.mergedPrs ||
        a.name.localeCompare(b.name) ||
        a.personId.localeCompare(b.personId),
    );
}

/** The first person (in rank order) with the highest `value`, if it reaches `min`. */
function leader(
  people: PersonScore[],
  value: (p: PersonScore) => number,
  min: number,
): string | null {
  let best: PersonScore | null = null;
  for (const p of people) {
    if (value(p) >= min && (!best || value(p) > value(best))) best = p;
  }
  return best ? best.personId : null;
}

function awardBadges(
  people: PersonScore[],
  newcomers: string[],
): Map<string, Badge[]> {
  const winner: Partial<Record<BadgeId, string | null>> = {
    builder: leader(people, (p) => p.deliveryFinal, Number.MIN_VALUE),
    cleaner: leader(people, (p) => p.netNegativePrs, MIN_BADGE_PRS),
    "bug-hunter": leader(people, (p) => p.fixPrs, MIN_BADGE_PRS),
    reviewer: leader(people, (p) => p.review, Number.MIN_VALUE),
  };
  const earned = (p: PersonScore, id: BadgeId) => {
    if (id === "streak") return p.streak >= STREAK_BADGE_DAYS;
    if (id === "newcomer") return newcomers.includes(p.personId);
    return winner[id] === p.personId;
  };
  return new Map(
    people.map((p) => [
      p.personId,
      (Object.keys(BADGES) as BadgeId[])
        .filter((id) => earned(p, id))
        .slice(0, MAX_BADGES)
        .map((id) => BADGES[id]),
    ]),
  );
}

export type Leaderboard = { rows: LeaderboardRow[]; podium: LeaderboardRow[] };

export function buildLeaderboard(
  current: PeriodScores,
  previous: PeriodScores | null,
  opts: { meId?: string },
): Leaderboard {
  const people = ranked(current.people);
  const previousRank = new Map(
    previous
      ? ranked(previous.people).map((p, i) => [p.personId, i + 1] as const)
      : [],
  );
  const badges = awardBadges(people, current.newcomers);
  const rows = people.map((p, i): LeaderboardRow => {
    const before = previousRank.get(p.personId);
    return {
      ...p,
      rank: i + 1,
      movement: before === undefined ? null : before - (i + 1),
      badges: badges.get(p.personId) ?? [],
      isMe: p.personId === opts.meId,
    };
  });
  return { rows, podium: rows.slice(0, 3) };
}

/** Score per person over the last 6 periods, oldest first; 0 for a period they were absent. */
export function trend(
  periods: PeriodScores[],
): { personId: string; points: number[] }[] {
  const recent = [...periods]
    .sort((a, b) => a.period.start.getTime() - b.period.start.getTime())
    .slice(-TREND_PERIODS);
  const ids = new Set<string>();
  for (const s of [...recent].reverse()) {
    for (const p of s.people) ids.add(p.personId);
  }
  return Array.from(ids, (personId) => ({
    personId,
    points: recent.map(
      (s) => s.people.find((p) => p.personId === personId)?.score ?? 0,
    ),
  }));
}
