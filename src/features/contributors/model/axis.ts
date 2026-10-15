/**
 * Indexes of the buckets that get an x-axis label: the first, the last and
 * about every Nth one between, so labels never overlap however many buckets
 * there are. A tick that would sit right next to the last one is dropped.
 */
export function axisTicks(count: number, maxLabels = 8): number[] {
  if (count <= 0) return [];
  const step = Math.max(1, Math.ceil(count / maxLabels));
  const last = count - 1;
  if (last === 0) return [0];
  // Every bucket is labelled only when each has its own slot; otherwise two
  // neighbours would crowd each other.
  const gap = step === 1 ? 1 : Math.max(2, Math.ceil(step / 2));
  const ticks: number[] = [];
  for (let i = 0; i < last; i += step) {
    if (last - i >= gap || i === 0) ticks.push(i);
  }
  if (last > 0) ticks.push(last);
  return ticks;
}
