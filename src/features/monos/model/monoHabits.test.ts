// @vitest-environment happy-dom
import { expect, it } from "vitest";
import type { Block } from "../../sessions/model/session";
import {
  HABIT_RUNS_KEPT,
  habitsToday,
  nextHabitAfterToday,
  habitStarting,
  setHabitRunning,
  subscribeHabitsRunning,
  afterRun,
  dropRelays,
  isRelayed,
  relayApproval,
  takeRelayedApproval,
  dueHabits,
  habitPostsContext,
  habitReport,
  habitRunPrompt,
  habitStopRequest,
  habitRunLaunches,
  habitRunStarted,
  noteHabitRunLaunch,
  markHabitRun,
  clearHabitRun,
  clearHabitStop,
  requestHabitStop,
  habitSchedule,
  habitScheduleLabel,
  MISSED_RUN_GRACE_MS,
  newHabit,
  nextHabitRunAt,
  parseHabits,
  QUIET_MARKER,
  serializeHabits,
  unseenHabitPosts,
  type Habit,
} from "./monoHabits";

const at = (day: number, hour: number, minute = 0) =>
  new Date(2026, 9, day, hour, minute).getTime();

const habit = (patch: Partial<Habit> = {}): Habit => ({
  id: "h1",
  name: "Morning CI check",
  instructions: "Check CI on main; tell me only if something is red.",
  schedule: {
    scheduleKind: "weekdays",
    time: "09:00",
    minute: 0,
    dayOfWeek: 1,
  },
  enabled: true,
  createdAt: at(1, 8),
  nextRunAt: at(5, 9),
  ...patch,
});

it("round-trips habits.json and drops anything malformed", () => {
  const text = serializeHabits([habit()]);
  expect(parseHabits(text)).toEqual([habit()]);
  expect(parseHabits('[{"id":"x"}, 3, null]')).toEqual([]);
  expect(parseHabits("not json")).toEqual([]);
  expect(parseHabits(null)).toEqual([]);
});

it("validates schedules from the CLI", () => {
  expect(habitSchedule({ kind: "weekdays", time: "09:30" })).toEqual({
    scheduleKind: "weekdays",
    time: "09:30",
    minute: 0,
    dayOfWeek: 1,
  });
  expect(() => habitSchedule({ kind: "monthly" })).toThrow("kind");
  expect(() => habitSchedule({ kind: "daily", time: "9am" })).toThrow("HH:MM");
  expect(() => habitSchedule({ kind: "weekly", dayOfWeek: 7 })).toThrow(
    "dayOfWeek",
  );
  expect(() => habitSchedule({ kind: "daily", cron: "* *" })).toThrow("cron");
});

const interval = (
  intervalHours: number,
  time = "00:00",
): Habit["schedule"] => ({
  scheduleKind: "interval",
  time,
  minute: 0,
  dayOfWeek: 1,
  intervalHours,
});

it("validates interval schedules from the CLI", () => {
  expect(habitSchedule({ kind: "interval", hours: 4, time: "09:00" })).toEqual(
    interval(4, "09:00"),
  );
  // The anchor defaults to midnight.
  expect(habitSchedule({ kind: "interval", hours: 6 })).toEqual(interval(6));
  expect(habitSchedule({ kind: "interval", hours: 1 })).toEqual(interval(1));
  expect(habitSchedule({ kind: "interval", hours: 24 })).toEqual(interval(24));
  for (const hours of [0, 25, -3, 1.5, "4", null, Number.NaN])
    expect(() => habitSchedule({ kind: "interval", hours })).toThrow(
      "schedule.hours must be an integer from 1 to 24",
    );
  expect(() => habitSchedule({ kind: "interval" })).toThrow(
    "schedule.hours is required",
  );
  expect(() =>
    habitSchedule({ kind: "interval", hours: 4, time: "9am" }),
  ).toThrow("HH:MM");
  expect(() =>
    habitSchedule({ kind: "interval", hours: 4, time: "24:00" }),
  ).toThrow("HH:MM");
  expect(() =>
    habitSchedule({ kind: "interval", hours: 4, cron: "* *" }),
  ).toThrow("cron");
  expect(() => habitSchedule({ kind: "daily", hours: 4 })).toThrow(
    'schedule.hours only applies to kind "interval"',
  );
  // The other kinds are unchanged and carry no interval.
  expect(habitSchedule({ kind: "daily", time: "08:00" })).not.toHaveProperty(
    "intervalHours",
  );
});

it("runs an interval habit at the anchor and every N hours after it", () => {
  const schedule = interval(4, "09:00");
  expect(nextHabitRunAt(schedule, at(5, 8))).toBe(at(5, 9));
  // Strictly after: a run that just fired moves on to the next slot.
  expect(nextHabitRunAt(schedule, at(5, 9))).toBe(at(5, 13));
  expect(nextHabitRunAt(schedule, at(5, 9) + 30_000)).toBe(at(5, 13));
  expect(nextHabitRunAt(schedule, at(5, 17, 1))).toBe(at(5, 21));
  // It continues across midnight rather than restarting at the anchor.
  expect(nextHabitRunAt(schedule, at(5, 21))).toBe(at(6, 1));
  expect(nextHabitRunAt(schedule, at(6, 1))).toBe(at(6, 5));
  expect(nextHabitRunAt(schedule, at(6, 5))).toBe(at(6, 9));
  // Slots before the anchor on its own day count too.
  expect(nextHabitRunAt(schedule, at(5, 0))).toBe(at(5, 1));
  expect(nextHabitRunAt(interval(1, "00:30"), at(5, 9, 31))).toBe(
    at(5, 10, 30),
  );
  expect(nextHabitRunAt(interval(6, "09:15"), at(5, 9, 15))).toBe(
    at(5, 15, 15),
  );
  // Every 24 hours is a daily habit.
  expect(nextHabitRunAt(interval(24, "07:00"), at(5, 7))).toBe(at(6, 7));
});

it("keeps the interval steady when N does not divide 24", () => {
  const schedule = interval(5, "09:00");
  const runs: number[] = [];
  for (let t = at(5, 0); runs.length < 25;)
    runs.push((t = nextHabitRunAt(schedule, t)));
  // Always N hours apart, across midnights.
  for (let i = 1; i < runs.length; i++)
    expect(runs[i] - runs[i - 1]).toBe(5 * 60 * 60 * 1000);
  // The same slot comes back every 5 days (120 hours), and so does the anchor.
  expect(
    runs.some(
      (run) =>
        run === at(5, 9) ||
        run === at(6, 9) ||
        run === at(7, 9) ||
        run === at(8, 9) ||
        run === at(9, 9),
    ),
  ).toBe(true);
  expect(runs[24] - runs[0]).toBe(120 * 60 * 60 * 1000);
  expect(new Date(runs[24]).getHours()).toBe(new Date(runs[0]).getHours());
  // Where it lands does not depend on when it is asked.
  expect(nextHabitRunAt(schedule, runs[3] - 1)).toBe(runs[3]);
  expect(nextHabitRunAt(schedule, runs[3] - 4 * 60 * 60 * 1000)).toBe(runs[3]);
  const seven = interval(7, "22:00");
  const first = nextHabitRunAt(seven, at(5, 12));
  expect(nextHabitRunAt(seven, first)).toBe(first + 7 * 60 * 60 * 1000);
});

it("starts a new interval habit on its anchor whatever N is", () => {
  const draft = (hours: number) => ({
    name: "n",
    instructions: "i",
    schedule: interval(hours, "09:00"),
  });
  // Made before the anchor on any day, the first run is the anchor itself.
  for (const hours of [1, 4, 5, 7, 9, 11, 24])
    for (let day = 5; day <= 14; day++)
      expect(newHabit(draft(hours), at(day, 8)).nextRunAt).toBe(at(day, 9));
  // Made after the anchor, it runs on from the anchor's own sequence.
  const made = newHabit(draft(5), at(5, 10));
  expect(made.schedule.intervalFrom).toBe("2026-10-05");
  expect(made.nextRunAt).toBe(at(5, 14));
  const second = afterRun(made, made.nextRunAt);
  expect(second.nextRunAt).toBe(at(5, 19));
  expect(afterRun(second, second.nextRunAt).nextRunAt).toBe(at(6, 0));
  // Other kinds are not pinned to a day.
  expect(
    newHabit(
      { ...draft(1), schedule: habitSchedule({ kind: "daily" }) },
      at(5, 8),
    ).schedule,
  ).not.toHaveProperty("intervalFrom");
  // The pin survives habits.json, and a bad one is ignored, not fatal.
  expect(parseHabits(serializeHabits([made]))).toEqual([made]);
  const garbled = JSON.parse(serializeHabits([made]));
  garbled[0].schedule.intervalFrom = "yesterday";
  expect(parseHabits(JSON.stringify(garbled))[0].schedule).not.toHaveProperty(
    "intervalFrom",
  );
});

it("drops interval habits with an unusable anchor and never hangs on one", () => {
  const saved = habit({ schedule: interval(4, "09:00") });
  for (const time of ["1e20:00", "25:99", "9:5", "", "noon"]) {
    const broken = { ...saved, schedule: { ...saved.schedule, time } };
    expect(parseHabits(JSON.stringify([broken]))).toEqual([]);
    // Built in code rather than read from disk, it still gives a sane time.
    const next = nextHabitRunAt({ ...saved.schedule, time }, at(5, 8));
    expect(next).toBeGreaterThan(at(5, 8));
    expect(next).toBeLessThanOrEqual(at(5, 8) + 4 * 60 * 60 * 1000);
  }
});

it("catches an interval habit up once and never queues a backlog", () => {
  const schedule = interval(4, "09:00");
  const due = habit({ schedule, nextRunAt: at(5, 13) });
  // 10 minutes late: runs once, then waits for the next slot.
  const now = at(5, 13, 10);
  expect(dueHabits([due], now).run.map((h) => h.id)).toEqual(["h1"]);
  expect(afterRun(due, now).nextRunAt).toBe(at(5, 17));
  // Asleep through 17:00 and 21:00 and woken at 21:30: one run, not three.
  const late = habit({ schedule, nextRunAt: at(5, 17) });
  const woke = at(5, 21, 30);
  const { run, skipped } = dueHabits([late], woke);
  expect(run).toEqual([]);
  expect(skipped.map((h) => h.id)).toEqual(["h1"]);
  expect(afterRun(late, woke).nextRunAt).toBe(at(6, 1));
  // Within the grace period the missed slot still runs, once.
  const recent = habit({ schedule, nextRunAt: at(5, 17) });
  expect(dueHabits([recent], at(5, 18, 30)).run).toHaveLength(1);
  expect(afterRun(recent, at(5, 18, 30)).nextRunAt).toBe(at(5, 21));
  // A new habit is first due at its next slot.
  expect(
    newHabit({ name: "n", instructions: "i", schedule }, at(5, 10)).nextRunAt,
  ).toBe(at(5, 13));
});

it("labels interval schedules readably", () => {
  expect(habitScheduleLabel(interval(4, "09:00"))).toBe(
    "Every 4 hours from 09:00",
  );
  expect(habitScheduleLabel(interval(1, "00:00"))).toBe(
    "Every hour from 00:00",
  );
  expect(habitScheduleLabel(interval(24, "07:30"))).toBe(
    "Every 24 hours from 07:30",
  );
});

it("round-trips interval habits and still loads saved habits without them", () => {
  const saved = habit({ schedule: interval(4, "09:00") });
  expect(parseHabits(serializeHabits([saved]))).toEqual([saved]);
  expect(JSON.parse(serializeHabits([saved]))[0].schedule).toEqual({
    scheduleKind: "interval",
    time: "09:00",
    minute: 0,
    dayOfWeek: 1,
    intervalHours: 4,
  });
  // habits.json written before the interval kind existed.
  const legacy = JSON.stringify([
    {
      id: "old",
      name: "Hourly peek",
      instructions: "Look.",
      schedule: {
        scheduleKind: "hourly",
        minute: 15,
        time: "09:00",
        dayOfWeek: 1,
      },
      enabled: true,
      createdAt: 1,
      nextRunAt: 2,
    },
    {
      id: "old2",
      name: "Daily",
      instructions: "Look.",
      schedule: {
        scheduleKind: "daily",
        minute: 0,
        time: "08:00",
        dayOfWeek: 1,
      },
      enabled: false,
      createdAt: 1,
      nextRunAt: 2,
    },
  ]);
  const loaded = parseHabits(legacy);
  expect(loaded.map((h) => h.id)).toEqual(["old", "old2"]);
  expect(loaded[0].schedule).toEqual({
    scheduleKind: "hourly",
    minute: 15,
    time: "09:00",
    dayOfWeek: 1,
  });
  expect(loaded[0].schedule).not.toHaveProperty("intervalHours");
  // An interval entry whose hours are not usable is left out, not guessed.
  for (const intervalHours of [0, 25, 1.5, "4", undefined]) {
    const broken = { ...saved, schedule: { ...saved.schedule, intervalHours } };
    expect(parseHabits(JSON.stringify([broken]))).toEqual([]);
  }
});

it("runs a recent missed habit once and skips stale ones", () => {
  const due = habit({ nextRunAt: at(5, 9) });
  const stale = habit({
    id: "h2",
    nextRunAt: at(5, 9) - MISSED_RUN_GRACE_MS - 1,
  });
  const paused = habit({ id: "h3", enabled: false, nextRunAt: at(5, 8) });
  const later = habit({ id: "h4", nextRunAt: at(5, 12) });
  const { run, skipped } = dueHabits([due, stale, paused, later], at(5, 9, 5));
  expect(run.map((h) => h.id)).toEqual(["h1"]);
  expect(skipped.map((h) => h.id)).toEqual(["h2"]);
});

it("moves to the next run and records how it went", () => {
  // Monday Oct 5 09:00 → next weekday is Tuesday Oct 6 09:00.
  const claimed = afterRun(habit(), at(5, 9));
  expect(claimed.nextRunAt).toBe(at(6, 9));
  expect(claimed.lastRunAt).toBeUndefined();
  const failed = afterRun(habit(), at(5, 9, 3), {
    kind: "failed",
    error: "timed out",
  });
  expect(failed).toMatchObject({
    lastOutcome: "failed",
    lastError: "timed out",
    lastRunAt: at(5, 9, 3),
  });
  expect(afterRun(failed, at(6, 9), { kind: "quiet" })).not.toHaveProperty(
    "lastError",
  );
});

it("stays silent when the run has nothing to say", () => {
  expect(habitReport(QUIET_MARKER)).toBeUndefined();
  expect(habitReport(`  **${QUIET_MARKER}**. `)).toBeUndefined();
  expect(
    habitReport(`CI is green, all good.\n\n${QUIET_MARKER}`),
  ).toBeUndefined();
  expect(habitReport("")).toBeUndefined();
  expect(habitReport("CI is red on main: the lint job fails since #712.")).toBe(
    "CI is red on main: the lint job fails since #712.",
  );
});

it("hands the Mono what its habits posted since the user last wrote", () => {
  const post = (id: string, text: string): Block => ({
    id,
    role: "assistant",
    text,
    monoHabit: { id: "h1", name: "Morning CI check", at: at(5, 9) },
  });
  const blocks: Block[] = [
    post("old", "seen before the user replied"),
    { id: "u1", role: "user", text: "thanks" },
    { id: "a1", role: "assistant", text: "anytime" },
    post("p1", "CI is red on main"),
    post("p2", "and a PR waits on you"),
  ];
  const posts = unseenHabitPosts(blocks);
  expect(posts.map((block) => block.id)).toEqual(["p1", "p2"]);
  const context = habitPostsContext(posts);
  expect(context).toContain("<while_you_were_away>");
  expect(context).toContain("Morning CI check: CI is red on main");
  expect(habitPostsContext([])).toBe("");
});

it("routes an answer in the Mono's chat back to the run that asked", () => {
  const id = relayApproval({
    monoId: "mono",
    runId: "run",
    requestId: 7,
    blockId: "b",
  });
  // Its own id, so it cannot collide with the Mono's own requests.
  expect(id).not.toBe(7);
  expect(isRelayed("run", 7)).toBe(true);
  expect(takeRelayedApproval("other-mono", id)).toBeUndefined();
  expect(takeRelayedApproval("mono", id)).toMatchObject({
    runId: "run",
    requestId: 7,
  });
  expect(takeRelayedApproval("mono", id)).toBeUndefined();
  relayApproval({ monoId: "mono", runId: "run", requestId: 8, blockId: "c" });
  expect(dropRelays("run").map((relay) => relay.blockId)).toEqual(["c"]);
  expect(isRelayed("run", 8)).toBe(false);
});

it("keeps a short history of runs, newest first", () => {
  let current = habit();
  for (let i = 0; i < HABIT_RUNS_KEPT + 3; i++)
    current = afterRun(current, at(5, 9, 2) + i * 60_000, {
      kind: i % 2 ? "quiet" : "posted",
      ...(i % 2 ? {} : { report: `report ${i}` }),
      startedAt: at(5, 9) + i * 60_000,
    });
  const runs = current.runs!;
  expect(runs).toHaveLength(HABIT_RUNS_KEPT);
  expect(runs[0]).toMatchObject({
    outcome: "posted",
    report: "report 22",
    durationMs: 120_000,
  });
  expect(runs[1]).not.toHaveProperty("report");
  // The history survives habits.json.
  expect(parseHabits(serializeHabits([current]))[0].runs).toEqual(runs);
  // Claiming a due run is not a run.
  expect(afterRun(habit(), at(5, 9)).runs).toBeUndefined();
});

it("never starts a habit again while it is still running", () => {
  const due = habit({ nextRunAt: at(5, 9) });
  setHabitRunning(due.id, at(5, 9));
  expect(dueHabits([due], at(5, 9, 5)).run).toEqual([]);
  setHabitRunning(due.id);
  expect(dueHabits([due], at(5, 9, 5)).run).toEqual([due]);
});

it("runs a requested habit once, even paused, without moving its schedule", () => {
  const requested = habit({
    enabled: false,
    runRequested: true,
    nextRunAt: at(6, 9),
  });
  expect(dueHabits([requested], at(5, 14)).run).toEqual([requested]);
  const claimed = afterRun(requested, at(5, 14));
  expect(claimed).toMatchObject({ enabled: false, nextRunAt: at(6, 9) });
  expect(claimed).not.toHaveProperty("runRequested");
  expect(habitStarting(requested, at(5, 14))).toBe(true);
  expect(habitStarting(claimed, at(5, 14))).toBe(false);
});

it("lays out today: live first, then the day in order, nothing from other days", () => {
  const now = at(5, 12);
  const ranThisMorning = habit({
    id: "morning",
    nextRunAt: at(6, 8, 30),
    runs: [{ at: at(5, 8, 30), durationMs: 60_000, outcome: "posted" }],
  });
  const later = habit({ id: "later", nextRunAt: at(5, 17) });
  const tomorrow = habit({ id: "tomorrow", nextRunAt: at(6, 9) });
  const running = habit({ id: "running", nextRunAt: at(6, 9) });
  const asked = habit({ id: "asked", nextRunAt: at(6, 9), runRequested: true });
  const paused = habit({ id: "paused", enabled: false, nextRunAt: at(5, 15) });
  const today = habitsToday(
    [later, tomorrow, ranThisMorning, running, asked, paused],
    now,
    (id) => (id === "running" ? at(5, 11, 58) : undefined),
  );
  expect(today.map((item) => [item.habit.id, item.state])).toEqual([
    ["running", "running"],
    ["asked", "starting"],
    ["morning", "done"],
    ["later", "upcoming"],
  ]);
  expect(today[2].outcome).toBe("posted");
  expect(nextHabitAfterToday([tomorrow, paused], now)?.habit.id).toBe(
    "tomorrow",
  );
});

it("starts a new habit enabled and due at its next time", () => {
  const now = new Date(2026, 9, 6, 8, 0).getTime();
  const habit = newHabit(
    {
      name: "Morning",
      instructions: "Say hi",
      schedule: {
        scheduleKind: "daily",
        time: "09:00",
        minute: 0,
        dayOfWeek: 1,
      },
    },
    now,
  );
  expect(habit).toMatchObject({ enabled: true, createdAt: now });
  expect(habit.id).toBeTruthy();
  expect(habit.nextRunAt).toBe(new Date(2026, 9, 6, 9, 0).getTime());
  expect(parseHabits(JSON.stringify([habit]))).toEqual([habit]);
});

it("starts a new habit enabled and due at its next scheduled time", () => {
  const created = newHabit(
    {
      name: "Standup",
      instructions: "Summarize yesterday's PRs.",
      schedule: {
        scheduleKind: "daily",
        time: "09:00",
        minute: 0,
        dayOfWeek: 1,
      },
    },
    at(6, 10),
  );
  expect(created).toMatchObject({
    name: "Standup",
    enabled: true,
    createdAt: at(6, 10),
    nextRunAt: at(7, 9),
  });
  expect(created.id).toBeTruthy();
});

it("tells a habit run that sessions it starts report to the Mono's chat", () => {
  const prompt = habitRunPrompt(
    {
      name: "Skull",
      mascot: "skull",
      color: "#fff",
      projects: [{ path: "/code/monocode", name: "monocode" }],
    },
    {
      id: "agent",
      dir: "/data/agents/agent",
      soul: "# Soul",
      soulHash: "s1",
      memory: "",
      memoryHash: "m1",
      memoryPath: "/data/agents/agent/MEMORY.md",
      topics: [],
    },
    habit(),
    at(5, 9),
  );
  const run = prompt.slice(prompt.indexOf("<habit_run>"));
  expect(run).toContain("report to your chat, not to this run");
  expect(run).toContain("still arrive after this run has ended");
  expect(run).toContain("hand the work off and finish");
  expect(run).toContain("do not wait for them or poll");
});

it("remembers a stop request per habit until it is cleared", () => {
  expect(habitStopRequest("h1")).toBeUndefined();
  requestHabitStop("h1", "run");
  requestHabitStop("h2", "all");
  expect(habitStopRequest("h1")).toBe("run");
  expect(habitStopRequest("h2")).toBe("all");
  clearHabitStop("h1");
  expect(habitStopRequest("h1")).toBeUndefined();
  expect(habitStopRequest("h2")).toBe("all");
  clearHabitStop("h2");
});

it("tells subscribers when a stop is requested or cleared", () => {
  let calls = 0;
  const off = subscribeHabitsRunning(() => calls++);
  requestHabitStop("h1", "run");
  clearHabitStop("h1");
  off();
  expect(calls).toBe(2);
});

it("tells the sessions a run started from the ones it only messaged", () => {
  const entry = (sessionId: string) => ({
    sessionId,
    cwd: "/code/app",
    title: sessionId,
    harness: "claude" as const,
    model: "claude:x",
  });
  markHabitRun("run-1", "mono-1");
  noteHabitRunLaunch("run-1", entry("messaged"));
  noteHabitRunLaunch("run-1", entry("started"), true);
  // Messaging a session it started earlier must not demote it.
  noteHabitRunLaunch("run-1", entry("started"));
  // Starting a session it had only messaged counts as started.
  noteHabitRunLaunch("run-1", entry("late"));
  noteHabitRunLaunch("run-1", entry("late"), true);
  expect(habitRunLaunches("run-1").map((e) => e.sessionId)).toEqual([
    "messaged",
    "started",
    "late",
  ]);
  expect(habitRunStarted("run-1").map((e) => e.sessionId)).toEqual([
    "started",
    "late",
  ]);
  // The entries stay plain Mono sessions, so they can go on a chat block.
  expect(Object.keys(habitRunLaunches("run-1")[1]).sort()).toEqual([
    "cwd",
    "harness",
    "model",
    "sessionId",
    "title",
  ]);
  clearHabitRun("run-1");
  expect(habitRunStarted("run-1")).toEqual([]);
});
