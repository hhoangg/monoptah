// Tells the desktop which chat this page is showing, so it does not send a push for what the user is reading.
// The desktop decides (it skips the push only for this device and exactly this target); the service worker never
// drops a push (iOS requires every push to show a notification). A report means "visible and focused": it is sent on
// start, on visibility, focus and page lifecycle changes, and every 25 s while it holds. Hidden, blurred, frozen or
// unmounted sends a cleared state. The desktop forgets a report 50 s after the last one and on every reconnect, so
// a page whose last message was lost never keeps suppressing.
import { useEffect } from "react";
import type { PresenceTarget } from "@monoptah/protocol";
import { useLinkConnection, useLinkState } from "./LinkProvider.tsx";

/** How often a visible, focused page repeats its report (the desktop treats one as stale after 50 s). */
export const PRESENCE_HEARTBEAT_MS = 25_000;

export interface PresenceSink {
  sendPresence(target: PresenceTarget | null): boolean;
}

export interface PresenceEnv {
  doc: Pick<Document, "visibilityState" | "hasFocus" | "addEventListener" | "removeEventListener">;
  win: Pick<Window, "addEventListener" | "removeEventListener">;
  setInterval: (fn: () => void, ms: number) => unknown;
  clearInterval: (handle: unknown) => void;
}

function browserEnv(): PresenceEnv | null {
  if (typeof document === "undefined" || typeof window === "undefined") return null;
  return {
    doc: document,
    win: window,
    setInterval: (fn, ms) => globalThis.setInterval(fn, ms),
    clearInterval: (handle) => globalThis.clearInterval(handle as ReturnType<typeof setInterval>),
  };
}

/**
 * Starts reporting `target` through `sink`. Returns a stop function that sends a cleared state. Free of React so a
 * test can drive it with a fake environment.
 */
export function startPresence(sink: PresenceSink, target: PresenceTarget, env: PresenceEnv): () => void {
  let timer: unknown = null;
  let cleared = false;

  const stopTimer = () => {
    if (timer !== null) env.clearInterval(timer);
    timer = null;
  };
  const clear = () => {
    stopTimer();
    if (!cleared) sink.sendPresence(null);
    cleared = true;
  };
  const report = () => {
    if (env.doc.visibilityState === "visible" && env.doc.hasFocus()) {
      sink.sendPresence(target);
      cleared = false;
      if (timer === null) timer = env.setInterval(report, PRESENCE_HEARTBEAT_MS);
    } else {
      clear();
    }
  };

  env.doc.addEventListener("visibilitychange", report);
  env.doc.addEventListener("freeze", clear);
  env.doc.addEventListener("resume", report);
  env.win.addEventListener("focus", report);
  env.win.addEventListener("blur", report);
  // The page is going away or suspended whatever `visibilityState` still says, so it is cleared unconditionally.
  env.win.addEventListener("pagehide", clear);
  env.win.addEventListener("pageshow", report);
  report();

  return () => {
    env.doc.removeEventListener("visibilitychange", report);
    env.doc.removeEventListener("freeze", clear);
    env.doc.removeEventListener("resume", report);
    env.win.removeEventListener("focus", report);
    env.win.removeEventListener("blur", report);
    env.win.removeEventListener("pagehide", clear);
    env.win.removeEventListener("pageshow", report);
    stopTimer();
    sink.sendPresence(null);
  };
}

/** For a chat screen: reports `{kind, id}` while it is mounted, visible and focused, and again after a reconnect. */
export function usePresence(kind: PresenceTarget["kind"], id: string): void {
  const connection = useLinkConnection();
  // A new session starts with no report on the desktop: the status change restarts the reporter.
  const status = useLinkState((state) => state.status);
  useEffect(() => {
    if (status !== "ready") return undefined;
    const env = browserEnv();
    if (!env) return undefined;
    return startPresence(connection, { kind, id }, env);
  }, [connection, kind, id, status]);
}
