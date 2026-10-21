import { useSyncExternalStore } from "react";
import { message } from "@tauri-apps/plugin-dialog";
import {
  habitRunningSince,
  habitStopRequest,
  requestHabitStop,
  subscribeHabitsRunning,
  type Habit,
  type HabitStopMode,
} from "../model/monoHabits";

export const HABIT_STOP_CHOICES = {
  run: "Stop run only",
  all: "Stop run and its sessions",
  keep: "Keep running",
} as const;

/** The native sheet; injectable so the choice can be tested without Tauri. */
type ShowDialog = (
  text: string,
  options: {
    title: string;
    kind: "warning";
    buttons: { yes: string; no: string; cancel: string };
  },
) => Promise<string>;

/**
 * Asks how to stop a running habit. Resolves to the stop mode, or undefined
 * when the user keeps it running (including closing the sheet).
 */
export async function confirmHabitStop(
  habit: Habit,
  show: ShowDialog = message,
): Promise<HabitStopMode | undefined> {
  const answer = await show(
    `Stop "${habit.name}"? Sessions it already started can keep running.`,
    {
      title: "Monoptah",
      kind: "warning",
      buttons: {
        yes: HABIT_STOP_CHOICES.run,
        no: HABIT_STOP_CHOICES.all,
        cancel: HABIT_STOP_CHOICES.keep,
      },
    },
  );
  if (answer === HABIT_STOP_CHOICES.run) return "run";
  if (answer === HABIT_STOP_CHOICES.all) return "all";
  return undefined;
}

/** Tells the user something plainly; injectable like the confirm. */
function tellUser(text: string): Promise<unknown> {
  return message(text, { title: "Monoptah" });
}

/**
 * Asks, then asks the scheduler to stop the habit's run. A run that ended
 * while the sheet was open has nothing left to stop, and a request recorded
 * then would be dropped without a word, so the user is told instead.
 */
export async function stopHabit(
  habit: Habit,
  confirm: (
    habit: Habit,
  ) => Promise<HabitStopMode | undefined> = confirmHabitStop,
  notify: (text: string) => Promise<unknown> = tellUser,
): Promise<void> {
  if (habitStopRequest(habit.id)) return;
  const mode = await confirm(habit);
  if (!mode) return;
  if (habitRunningSince(habit.id) == null) {
    await notify(
      `"${habit.name}" already finished, so there was nothing to stop.`,
    );
    return;
  }
  requestHabitStop(habit.id, mode);
}

/** True from the moment the user asks to stop until the run has ended. */
export function useHabitStopping(habitId: string): boolean {
  return useSyncExternalStore(
    subscribeHabitsRunning,
    () => habitStopRequest(habitId) != null,
  );
}
