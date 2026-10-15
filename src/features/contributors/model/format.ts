const SCORE_FORMAT = new Intl.NumberFormat("en-US", {
  maximumFractionDigits: 0,
});

export function formatScore(n: number): string {
  // `+ 0` turns -0 into 0 so a tiny negative never prints as "-0".
  return SCORE_FORMAT.format(Math.round(n) + 0);
}

/** A fraction as a signed percentage: 0.18 -> "+18%". */
export function formatDelta(frac: number): string {
  const pct = Math.round(frac * 100);
  if (pct === 0) return "0%";
  return `${pct > 0 ? "+" : "-"}${Math.abs(pct)}%`;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export function relativeTime(iso: string, now: Date): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const diff = now.getTime() - then;
  if (diff < MINUTE) return "just now";
  if (diff < HOUR) return `${Math.floor(diff / MINUTE)}m ago`;
  if (diff < DAY) return `${Math.floor(diff / HOUR)}h ago`;
  const days = Math.floor(diff / DAY);
  if (days < 60) return `${days}d ago`;
  if (days < 365) return `${Math.floor(days / 30)}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}
