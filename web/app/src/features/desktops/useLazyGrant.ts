// The lazy grant request of `/d/:desktopId` (W-04). Owner: WS-M.
//
// The relay refuses a device that has no grant row for the desktop (close 4003). The link's pre-connect
// hook (preConnect.ts) now requests the grant before the relay is joined, so that race is gone; this hook
// keeps the UI phases around the same request:
//  - on mount it registers the device (`ensureDevice`) and requests the grant once for THIS desktop
//    only (never for the others); a marker in localStorage lets later opens skip the POST;
//  - while the request is in flight a `revoked` block is not a verdict ("requesting");
//  - a close 4003 never reconnects: after the POST answered with a grant, a `revoked` block is a real revoke
//    and the revoked screen shows (`raceOpen` only keeps the screen on "requesting" until the POST answered);
//  - a 409 `conflict` means the grant itself is revoked; a 403 or 404 means this device is unknown to the
//    server (forgotten): the revoked screen shows and nothing reconnects;
//  - when the marker says it was requested but the link reports a race `revoked`, one POST re-checks it.
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { isApiError } from "@/features/accounts/api";
import { ensureDevice, getLinkStore, useLinkState, type LinkState } from "@/link";
import { requestGrantAndRefresh } from "./api";
import { markGrantRequested, wasGrantRequested } from "./storage";

export type GrantPhase =
  /** The device is being registered. */
  | "init"
  /** A marker says the grant was requested before; no POST yet. */
  | "known"
  /** The POST is in flight. */
  | "requesting"
  /** The grant exists (pending or confirmed). */
  | "posted"
  /** The server answered 409 (revoked grant) or 403/404 (device forgotten or unknown): no access. */
  | "revoked"
  /** The request failed for another reason (network, server). */
  | "failed";

/** `missing`: the server does not know the desktop or the device (404, 403); `revoked`: the grant itself is revoked (409). */
export type GrantOutcome = "posted" | "revoked" | "missing" | "failed";

const inFlight = new Map<string, Promise<GrantOutcome>>();

/** One POST per desktop and device at a time (a remount or a strict-mode double effect shares it). */
export function requestOnce(queryClient: QueryClient, desktopId: string, deviceId: string): Promise<GrantOutcome> {
  const key = `${desktopId}:${deviceId}`;
  const existing = inFlight.get(key);
  if (existing) return existing;
  const run = requestGrantAndRefresh(queryClient, desktopId, deviceId)
    .then((): "posted" => {
      markGrantRequested(desktopId, deviceId);
      return "posted";
    })
    .catch((error: unknown): "revoked" | "missing" | "failed" => (isApiError(error, "conflict") ? "revoked" : isApiError(error, "not_found") || isApiError(error, "forbidden") ? "missing" : "failed"))
    .finally(() => inFlight.delete(key));
  inFlight.set(key, run);
  return run;
}

/** The relay's close code for "no grant row" (CloseCodes.REVOKED). */
const RELAY_REVOKED_CLOSE = 4003;

export interface LazyGrant {
  phase: GrantPhase;
  /**
   * True while a `revoked` block may still be the first-open race (close 4003 before the link was ever
   * pending or ready in this mount). When false, a `revoked` block is final.
   */
  raceOpen: boolean;
  /** Runs the request again after a failure. */
  retry: () => void;
}

/** Call inside a `LinkProvider`; remount (`key`) per desktop. */
export function useLazyGrant(desktopId: string): LazyGrant {
  const queryClient = useQueryClient();
  const blocked = useLinkState((state) => state.blocked);
  const closeCode = useLinkState((state) => state.lastError?.closeCode ?? null);
  const [seenLive, setSeenLive] = useState(false);
  const [phase, setPhase] = useState<GrantPhase>("init");
  const [attempt, setAttempt] = useState(0);
  const deviceIdRef = useRef<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // Remember that the link was pending or ready at least once in this mount: a later `revoked` is final.
  useEffect(() => {
    const store = getLinkStore(desktopId);
    const note = (state: LinkState) => {
      if (state.status === "ready" || state.status === "pending") setSeenLive(true);
    };
    note(store.getState());
    return store.subscribe(note);
  }, [desktopId]);
  const raceOpen = !seenLive && closeCode === RELAY_REVOKED_CLOSE;

  const request = useCallback(
    async (deviceId: string) => {
      setPhase("requesting");
      const outcome = await requestOnce(queryClient, desktopId, deviceId);
      if (alive.current) setPhase(outcome === "missing" ? "revoked" : outcome);
    },
    [queryClient, desktopId],
  );

  // First open: register the device, then request the grant unless it was requested before.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const { deviceId } = await ensureDevice();
        if (cancelled) return;
        deviceIdRef.current = deviceId;
        if (wasGrantRequested(desktopId, deviceId)) setPhase("known");
        else await request(deviceId);
      } catch {
        if (!cancelled) setPhase("failed");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [desktopId, request, attempt]);

  // The marker was set but the link says `revoked`: one POST tells whether the grant really is revoked (409).
  useEffect(() => {
    const deviceId = deviceIdRef.current;
    if (phase === "known" && blocked === "revoked" && raceOpen && deviceId) void request(deviceId);
  }, [phase, blocked, raceOpen, request]);

  const retry = useCallback(() => {
    const deviceId = deviceIdRef.current;
    if (deviceId) void request(deviceId);
    else {
      setPhase("init");
      setAttempt((count) => count + 1);
    }
  }, [request]);

  return { phase, raceOpen, retry };
}
