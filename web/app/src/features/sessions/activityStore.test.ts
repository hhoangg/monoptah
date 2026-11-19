import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionCard } from "@monoptah/protocol";
import {
  ARRIVAL_MS,
  FINISHED_KEPT,
  MISSING_MS,
  markStartedHere,
  markStoppedHere,
  markViewing,
  resetSessionActivity,
  trackSessions,
  useActivityStore,
} from "./activityStore";

const card = (id: string, extra: Partial<SessionCard> = {}): SessionCard => ({
  id,
  projectId: "p1",
  title: id,
  harness: "claude",
  model: "m",
  branch: "main",
  updatedAt: 1,
  ...extra,
});
const state = () => useActivityStore.getState();

beforeEach(() => {
  vi.useFakeTimers();
  resetSessionActivity();
});
afterEach(() => {
  resetSessionActivity();
  vi.useRealTimers();
});

describe("activity tracker (F-27)", () => {
  it("the first list is no news: working sessions are listed, nothing finished or arrived", () => {
    trackSessions("d", [card("a", { busy: true }), card("b")]);
    expect(state().working.map((s) => s.id)).toEqual(["a"]);
    expect(state().finished).toEqual([]);
    expect(state().arrivals).toEqual({});
  });

  it("a session that starts working is an arrival for 12 s, unless started here", () => {
    trackSessions("d", [card("a"), card("b")]);
    markStartedHere("b");
    trackSessions("d", [card("a", { busy: true }), card("b", { busy: true })]);
    expect(state().working.map((s) => s.id)).toEqual(["a", "b"]);
    expect(Object.keys(state().arrivals)).toEqual(["a"]);
    vi.advanceTimersByTime(ARRIVAL_MS + 1);
    expect(state().arrivals).toEqual({});
  });

  it("working sessions keep their order as their details change", () => {
    trackSessions("d", [card("a"), card("b")]);
    trackSessions("d", [card("a", { busy: true }), card("b", { busy: true })]);
    trackSessions("d", [card("b", { busy: true, updatedAt: 9 }), card("a", { busy: true, updatedAt: 5 })]);
    expect(state().working.map((s) => s.id)).toEqual(["a", "b"]);
    expect(state().working[1]?.updatedAt).toBe(9);
  });

  it("a session that stops lands on the finished shelf, newest first, at most 8", () => {
    const ids = Array.from({ length: FINISHED_KEPT + 2 }, (_, i) => `s${i}`);
    trackSessions("d", ids.map((id) => card(id, { busy: true })));
    for (const id of ids) trackSessions("d", ids.map((other) => card(other, { busy: ids.indexOf(other) > ids.indexOf(id) })));
    expect(state().finished).toHaveLength(FINISHED_KEPT);
    expect(state().finished[0]?.session.id).toBe(ids.at(-1));
  });

  it("a stop pressed here or a session on screen does not land on the shelf", () => {
    trackSessions("d", [card("a", { busy: true }), card("b", { busy: true })]);
    markStoppedHere("a");
    const release = markViewing("b");
    trackSessions("d", [card("a"), card("b")]);
    expect(state().finished).toEqual([]);
    release();
  });

  it("a finished session missing for 60 s is dropped", () => {
    trackSessions("d", [card("a", { busy: true })]);
    trackSessions("d", [card("a")]);
    expect(state().finished).toHaveLength(1);
    trackSessions("d", []);
    vi.advanceTimersByTime(MISSING_MS + 1);
    expect(state().finished).toEqual([]);
  });

  it("a session that works again leaves the shelf, and another desktop starts fresh", () => {
    trackSessions("d", [card("a", { busy: true })]);
    trackSessions("d", [card("a")]);
    trackSessions("d", [card("a", { busy: true })]);
    expect(state().finished).toEqual([]);
    trackSessions("other", [card("z")]);
    expect(state().working).toEqual([]);
    expect(state().desktopId).toBe("other");
  });

  it("a waiting session counts as working", () => {
    trackSessions("d", [card("a", { waiting: "approval" })]);
    expect(state().working.map((s) => s.id)).toEqual(["a"]);
  });
});
