// F-27 activity tracker. Ported from mobile/src/features/sessions/activityStore.ts with the same rules:
// working sessions keep the order they started in, a session that stops working goes onto a shelf of
// the last 8 finished (dropped after 60 s missing from the list), and one that starts working away from
// this device is an arrival for 12 s. Held in a zustand store, scoped to one desktop at a time.
import { create } from "zustand";
import { useMemo } from "react";
import type { SessionCard } from "@monoptah/protocol";

/** How long a session that starts working elsewhere is marked as new. */
export const ARRIVAL_MS = 12_000;
/** Finished sessions kept on the shelf, newest first. */
export const FINISHED_KEPT = 8;
/** How long a finished session may be missing from the list before it is taken as deleted. */
export const MISSING_MS = 60_000;

export type ActivitySession = SessionCard;
export type FinishedSession = { session: ActivitySession; at: number };

interface Activity {
  desktopId: string | null;
  /** Sessions at work, in the order they started, so a list of them stays put as their times change. */
  working: ActivitySession[];
  /** Sessions that stopped working since they were seen at it, newest first, until opened or put away. */
  finished: FinishedSession[];
  /** Sessions that started working away from this device, by when. */
  arrivals: Record<string, number>;
}

const EMPTY: Activity = { desktopId: null, working: [], finished: [], arrivals: {} };

export const useActivityStore = create<Activity>(() => EMPTY);

/** Each session as last seen; null until the first list, which is no news. */
let seen: Map<string, ActivitySession> | null = null;
const startedHere = new Set<string>();
const stoppedHere = new Set<string>();
const viewing = new Map<string, number>();
const arrivalTimers = new Map<string, ReturnType<typeof setTimeout>>();
const missing = new Map<string, { since: number; timer: ReturnType<typeof setTimeout> }>();

export const isWorking = (session: Pick<SessionCard, "busy" | "waiting">): boolean => !!session.busy || !!session.waiting;

function publish(next: Activity): void {
  for (const id of missing.keys()) {
    if (!next.finished.some((entry) => entry.session.id === id)) clearMissing(id);
  }
  useActivityStore.setState(next, true);
}

function clearMissing(sessionId: string): void {
  clearTimeout(missing.get(sessionId)?.timer);
  missing.delete(sessionId);
}

/** A message or a new session sent from here: its work is no arrival. */
export function markStartedHere(sessionId: string): void {
  startedHere.add(sessionId);
}

/** A stop pressed here: the session will not land on the finished shelf. */
export function markStoppedHere(sessionId: string): void {
  stoppedHere.add(sessionId);
}

/** A session's screen is up; until the returned function runs, its finish is no news. */
export function markViewing(sessionId: string): () => void {
  viewing.set(sessionId, (viewing.get(sessionId) ?? 0) + 1);
  dismissFinished(sessionId);
  return () => {
    const left = (viewing.get(sessionId) ?? 1) - 1;
    if (left > 0) viewing.set(sessionId, left);
    else viewing.delete(sessionId);
  };
}

/** Takes a session off the finished shelf, as it is opened or put away. */
export function dismissFinished(sessionId: string): void {
  const state = useActivityStore.getState();
  if (!state.finished.some((entry) => entry.session.id === sessionId)) return;
  publish({ ...state, finished: state.finished.filter((entry) => entry.session.id !== sessionId) });
}

export function dismissAllFinished(projectId: string): void {
  const state = useActivityStore.getState();
  const finished = state.finished.filter((entry) => entry.session.projectId !== projectId);
  if (finished.length !== state.finished.length) publish({ ...state, finished });
}

function endArrival(sessionId: string): void {
  clearTimeout(arrivalTimers.get(sessionId));
  arrivalTimers.delete(sessionId);
  const state = useActivityStore.getState();
  if (!(sessionId in state.arrivals)) return;
  const { [sessionId]: _gone, ...arrivals } = state.arrivals;
  publish({ ...state, arrivals });
}

function sameList(a: ActivitySession[], b: ActivitySession[]): boolean {
  return a.length === b.length && a.every((session, index) => session === b[index]);
}

/** Forgets everything, as another desktop is opened. */
export function resetSessionActivity(): void {
  seen = null;
  startedHere.clear();
  stoppedHere.clear();
  viewing.clear();
  for (const id of [...missing.keys()]) clearMissing(id);
  arrivalTimers.forEach(clearTimeout);
  arrivalTimers.clear();
  publish(EMPTY);
}

/**
 * Compares the desktop's latest session list with the last: a session that starts working joins the end
 * of `working` (an arrival, unless this device set it going), and one that stops goes onto the finished
 * shelf (unless stopped from here, or open on screen).
 */
export function trackSessions(desktopId: string, sessions: readonly ActivitySession[], now = Date.now()): void {
  if (useActivityStore.getState().desktopId !== desktopId) {
    resetSessionActivity();
    useActivityStore.setState({ desktopId });
  }
  const before = seen;
  const byId = new Map(sessions.map((session) => [session.id, session]));
  seen = byId;
  const state = useActivityStore.getState();
  if (!before) {
    publish({ ...EMPTY, desktopId, working: sessions.filter(isWorking) });
    return;
  }

  const arrivals = { ...state.arrivals };
  let finished = state.finished;
  const started: ActivitySession[] = [];
  for (const session of sessions) {
    const last = before.get(session.id);
    const wasWorking = !!last && isWorking(last);
    if (isWorking(session) && !wasWorking) {
      started.push(session);
      finished = finished.filter((entry) => entry.session.id !== session.id);
      if (!startedHere.has(session.id)) {
        arrivals[session.id] = now;
        clearTimeout(arrivalTimers.get(session.id));
        arrivalTimers.set(session.id, setTimeout(() => endArrival(session.id), ARRIVAL_MS));
      }
    } else if (!isWorking(session) && wasWorking) {
      startedHere.delete(session.id);
      delete arrivals[session.id];
      if (stoppedHere.delete(session.id) || viewing.has(session.id)) continue;
      finished = [{ session, at: now }, ...finished.filter((entry) => entry.session.id !== session.id)].slice(0, FINISHED_KEPT);
    }
  }

  // The ones still at work keep their places, with their latest details.
  const working = [
    ...state.working.flatMap((session) => {
      const latest = byId.get(session.id);
      return latest && isWorking(latest) ? [latest] : [];
    }),
    ...started,
  ];
  // Gone from the desktop a while, gone from the shelf; the rest take their latest titles.
  finished = finished.flatMap((entry) => {
    const id = entry.session.id;
    const latest = byId.get(id);
    if (!latest) {
      let absent = missing.get(id);
      if (!absent) {
        absent = { since: now, timer: setTimeout(() => dismissFinished(id), MISSING_MS) };
        missing.set(id, absent);
      }
      return now - absent.since >= MISSING_MS ? [] : [entry];
    }
    clearMissing(id);
    return [latest === entry.session ? entry : { ...entry, session: latest }];
  });
  for (const id of Object.keys(arrivals)) if (!byId.has(id)) delete arrivals[id];

  const changed =
    !sameList(working, state.working) ||
    !sameList(
      finished.map((entry) => entry.session),
      state.finished.map((entry) => entry.session),
    ) ||
    Object.keys(arrivals).join() !== Object.keys(state.arrivals).join();
  if (changed) publish({ desktopId, working, finished, arrivals });
}

/** A project's sessions at work, finished, and newly arrived. */
export function useSessionActivity(projectId: string) {
  const working = useActivityStore((state) => state.working);
  const finished = useActivityStore((state) => state.finished);
  const arrivals = useActivityStore((state) => state.arrivals);
  return useMemo(
    () => ({
      working: working.filter((session) => session.projectId === projectId),
      finished: finished.filter((entry) => entry.session.projectId === projectId),
      arrivals,
    }),
    [working, finished, arrivals, projectId],
  );
}
