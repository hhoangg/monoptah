import { describe, expect, it } from "vitest";
import { MinuteCounters, buildSchedule, mulberry32, pingTimes, sampleFrameSize } from "../src/schedule.ts";

describe("replay schedule", () => {
  it("default 2 frames/s for 30 min is 3,600 frames, 120 per minute", () => {
    const s = buildSchedule(2, 1800, 1);
    expect(s).toHaveLength(3600);
    const perMinute = new Array<number>(30).fill(0);
    for (const f of s) perMinute[Math.floor(f.atMs / 60_000)]!++;
    expect(perMinute.every((n) => n === 120)).toBe(true);
    expect(s.at(-1)!.atMs).toBeLessThan(1_800_000);
  });

  it("is reproducible from the seed and sizes stay inside 60..16000", () => {
    expect(buildSchedule(2, 60, 7)).toEqual(buildSchedule(2, 60, 7));
    expect(buildSchedule(2, 60, 7)).not.toEqual(buildSchedule(2, 60, 8));
    const rng = mulberry32(3);
    for (let i = 0; i < 5000; i++) {
      const n = sampleFrameSize(rng);
      expect(n).toBeGreaterThanOrEqual(60);
      expect(n).toBeLessThan(16_000);
    }
  });

  it("mean size is near 1,200 bytes", () => {
    const rng = mulberry32(11);
    let sum = 0;
    for (let i = 0; i < 50_000; i++) sum += sampleFrameSize(rng);
    expect(sum / 50_000).toBeGreaterThan(1000);
    expect(sum / 50_000).toBeLessThan(1500);
  });

  it("ping schedule: 30 s over 30 min is 60 pings, 60 s is 30, 0 is none", () => {
    expect(pingTimes(30, 1800)).toHaveLength(60);
    expect(pingTimes(60, 1800)).toHaveLength(30);
    expect(pingTimes(0, 1800)).toEqual([]);
    expect(pingTimes(30, 100)).toEqual([30_000, 60_000, 90_000]);
  });

  it("degenerate inputs give an empty schedule", () => {
    expect(buildSchedule(0, 10, 1)).toEqual([]);
    expect(buildSchedule(2, 0, 1)).toEqual([]);
  });
});

describe("MinuteCounters", () => {
  it("counts frames, bytes and messages per minute", () => {
    const c = new MinuteCounters();
    c.frame(0, 100);
    c.frame(0, 50);
    c.message(0);
    c.frame(2, 10);
    expect(c.frames).toEqual([2, 0, 1]);
    expect(c.bytes).toEqual([150, 0, 10]);
    expect(c.messages).toEqual([3, 0, 1]);
    expect(c.total()).toEqual({ frames: 3, bytes: 160, messages: 4 });
  });
});
