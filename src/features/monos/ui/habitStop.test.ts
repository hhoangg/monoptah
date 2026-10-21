// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  clearHabitStop,
  habitStopRequest,
  setHabitRunning,
  type Habit,
} from "../model/monoHabits";
import {
  HABIT_STOP_CHOICES,
  confirmHabitStop,
  stopHabit,
  useHabitStopping,
} from "./habitStop";

const habit = { id: "h1", name: "PR check" } as Habit;

beforeEach(() => setHabitRunning(habit.id, Date.now()));
afterEach(() => {
  clearHabitStop(habit.id);
  setHabitRunning(habit.id);
});

it("offers the three choices as the dialog's buttons", async () => {
  const show = vi.fn(async () => HABIT_STOP_CHOICES.keep);
  await confirmHabitStop(habit, show);
  expect(show).toHaveBeenCalledTimes(1);
  const [text, options] = show.mock.calls[0] as unknown as [
    string,
    { buttons: Record<string, string> },
  ];
  expect(text).toContain("PR check");
  expect(options.buttons).toEqual({
    yes: "Stop run only",
    no: "Stop run and its sessions",
    cancel: "Keep running",
  });
});

it("maps each button to a stop mode, and keeping to none", async () => {
  const answer = (label: string) => confirmHabitStop(habit, async () => label);
  expect(await answer("Stop run only")).toBe("run");
  expect(await answer("Stop run and its sessions")).toBe("all");
  expect(await answer("Keep running")).toBeUndefined();
  // Closing the sheet some other way is also keeping it running.
  expect(await answer("Cancel")).toBeUndefined();
});

it("requests the chosen stop, and nothing when the user keeps it running", async () => {
  await stopHabit(habit, async () => undefined);
  expect(habitStopRequest("h1")).toBeUndefined();
  await stopHabit(habit, async () => "all");
  expect(habitStopRequest("h1")).toBe("all");
});

it("does not ask again once a stop was requested", async () => {
  await stopHabit(habit, async () => "run");
  const confirm = vi.fn(async () => "all" as const);
  await stopHabit(habit, confirm);
  expect(confirm).not.toHaveBeenCalled();
  expect(habitStopRequest("h1")).toBe("run");
});

it("tells the user instead when the run ended while they were choosing", async () => {
  const notify = vi.fn(async () => undefined);
  await stopHabit(
    habit,
    async () => {
      setHabitRunning(habit.id);
      return "all";
    },
    notify,
  );
  expect(habitStopRequest("h1")).toBeUndefined();
  expect(notify).toHaveBeenCalledTimes(1);
  expect(notify.mock.calls[0][0]).toContain("already finished");
});

it("does not tell the user anything when they keep it running", async () => {
  const notify = vi.fn(async () => undefined);
  await stopHabit(habit, async () => undefined, notify);
  expect(notify).not.toHaveBeenCalled();
});

it("reports whether a stop is pending", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const root = createRoot(document.createElement("div"));
  const seen: boolean[] = [];
  function Probe() {
    seen.push(useHabitStopping(habit.id));
    return null;
  }
  act(() => root.render(createElement(Probe)));
  await act(async () => {
    await stopHabit(habit, async () => "run");
  });
  expect(seen.at(-1)).toBe(true);
  act(() => clearHabitStop(habit.id));
  expect(seen.at(-1)).toBe(false);
  act(() => root.unmount());
  vi.unstubAllGlobals();
});
