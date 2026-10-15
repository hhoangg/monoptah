/** 38200 -> "38.2k", 2500000 -> "2.5M"; small numbers stay whole. */
export function formatCompact(n: number): string {
  if (!Number.isFinite(n)) return "0";
  const value = Math.round(n);
  const abs = Math.abs(value);
  const sign = value < 0 ? "-" : "";
  if (abs < 1000) return String(value + 0);
  const trim = (x: number) => String(Math.round(x * 10) / 10);
  if (abs < 999_950) return `${sign}${trim(abs / 1000)}k`;
  return `${sign}${trim(abs / 1_000_000)}M`;
}
