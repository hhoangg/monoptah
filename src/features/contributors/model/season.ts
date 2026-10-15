export type Season = {
  id: string; // "2026-10"
  label: string; // "Oct 2026"
  start: Date;
  end: Date; // exclusive
};

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

const pad2 = (n: number) => String(n).padStart(2, "0");

function seasonFor(year: number, month: number): Season {
  // Normalize overflow such as month -1 or 12 through Date.
  const start = new Date(year, month, 1);
  const y = start.getFullYear();
  const m = start.getMonth();
  return {
    id: `${y}-${pad2(m + 1)}`,
    label: `${MONTHS[m]} ${y}`,
    start,
    end: new Date(y, m + 1, 1),
  };
}

/** The calendar month of `date` in local time. */
export function seasonOf(date: Date): Season {
  return seasonFor(date.getFullYear(), date.getMonth());
}

export function previousSeason(season: Season): Season {
  return seasonFor(season.start.getFullYear(), season.start.getMonth() - 1);
}

export function seasonById(id: string): Season | null {
  const m = /^(\d{4})-(\d{2})$/.exec(id);
  if (!m) return null;
  const month = Number(m[2]);
  if (month < 1 || month > 12) return null;
  return seasonFor(Number(m[1]), month - 1);
}

export function isInSeason(season: Season, iso: string): boolean {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return false;
  return t >= season.start.getTime() && t < season.end.getTime();
}

/** "YYYY-MM-DD" of the local calendar day, or "" for an unparseable timestamp. */
export function localDayKey(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

export function isWeekend(dayKey: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(dayKey);
  if (!m) return false;
  const day = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getDay();
  return day === 0 || day === 6;
}

export function seasonDayKeys(season: Season): string[] {
  const year = season.start.getFullYear();
  const month = season.start.getMonth();
  const days = new Date(year, month + 1, 0).getDate();
  return Array.from(
    { length: days },
    (_, i) => `${year}-${pad2(month + 1)}-${pad2(i + 1)}`,
  );
}
