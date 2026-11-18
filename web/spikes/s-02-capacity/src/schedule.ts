/** Seeded PRNG (mulberry32): the replay is reproducible from `--seed`. */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Frame size distribution of one coalesced watch frame (outer frame bytes, 20 byte header included).
 * ASSUMPTION, not a recording: no recorded transcript stream exists in this repo yet. Mixture:
 *   70%  token deltas / small patches   uniform 60..400
 *   25%  tool output chunks             uniform 400..4000
 *    5%  large snapshots (file view)    uniform 4000..16000 (the 16 KiB packing budget of capacity.md 3)
 * Mean about 1,200 bytes. Replace by a real capture before trusting byte totals (frame COUNT does not depend on it).
 */
export function sampleFrameSize(rng: () => number): number {
  const r = rng();
  const u = rng();
  if (r < 0.7) return Math.floor(60 + u * 340);
  if (r < 0.95) return Math.floor(400 + u * 3600);
  return Math.floor(4000 + u * 12000);
}

export interface ScheduledFrame {
  /** Offset from the start of the replay, ms. */
  atMs: number;
  size: number;
}

/** `rate` frames per second for `durationS` seconds, evenly spaced, sizes from {@link sampleFrameSize}. */
export function buildSchedule(rate: number, durationS: number, seed: number): ScheduledFrame[] {
  if (!(rate > 0) || !(durationS > 0)) return [];
  const rng = mulberry32(seed);
  const count = Math.floor(rate * durationS);
  const out: ScheduledFrame[] = [];
  for (let i = 0; i < count; i++) out.push({ atMs: Math.round((i * 1000) / rate), size: sampleFrameSize(rng) });
  return out;
}

/** Ping offsets (ms) for a keepalive interval; 0 means no keepalive. The first ping is one interval in. */
export function pingTimes(intervalS: number, durationS: number): number[] {
  if (!(intervalS > 0)) return [];
  const out: number[] = [];
  for (let t = intervalS; t <= durationS; t += intervalS) out.push(t * 1000);
  return out;
}

/** Per-minute counters of frames, bytes and messages for one side of the relay. */
export class MinuteCounters {
  readonly frames: number[] = [];
  readonly bytes: number[] = [];
  readonly messages: number[] = [];

  private slot(arr: number[], minute: number): void {
    while (arr.length <= minute) arr.push(0);
  }

  /** Records one binary frame (counts as a message too). */
  frame(minute: number, size: number): void {
    for (const a of [this.frames, this.bytes, this.messages]) this.slot(a, minute);
    this.frames[minute] = (this.frames[minute] ?? 0) + 1;
    this.bytes[minute] = (this.bytes[minute] ?? 0) + size;
    this.messages[minute] = (this.messages[minute] ?? 0) + 1;
  }

  /** Records a non-frame message (ping or pong text). */
  message(minute: number): void {
    this.slot(this.messages, minute);
    this.messages[minute] = (this.messages[minute] ?? 0) + 1;
  }

  total() {
    const sum = (a: number[]) => a.reduce((x, y) => x + y, 0);
    return { frames: sum(this.frames), bytes: sum(this.bytes), messages: sum(this.messages) };
  }
}
