import { describe, expect, it } from "vitest";
import {
  relatedThreadsLabel,
  runningSessionCount,
} from "./inboxRunningSessions";

describe("runningSessionCount", () => {
  const sessions = [{ id: "a" }, { id: "b" }, { id: "c" }];

  it("counts only the related sessions that are busy", () => {
    expect(runningSessionCount(sessions, new Set(["b", "x"]))).toBe(1);
    expect(runningSessionCount(sessions, new Set(["a", "b", "c"]))).toBe(3);
  });

  it("is zero without a busy set or without related sessions", () => {
    expect(runningSessionCount(sessions, undefined)).toBe(0);
    expect(runningSessionCount(sessions, new Set())).toBe(0);
    expect(runningSessionCount([], new Set(["a"]))).toBe(0);
  });
});

describe("relatedThreadsLabel", () => {
  it("pluralises the thread count", () => {
    expect(relatedThreadsLabel(1, 0)).toBe("1 related thread");
    expect(relatedThreadsLabel(3, 0)).toBe("3 related threads");
  });

  it("appends how many are running", () => {
    expect(relatedThreadsLabel(1, 1)).toBe("1 related thread, 1 running");
    expect(relatedThreadsLabel(3, 2)).toBe("3 related threads, 2 running");
  });
});
