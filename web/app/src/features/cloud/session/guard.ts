// `send-guard` and `session-capabilities` of U-16 (workstreams.md 3.2, features.md X-11).
import type { CloudState } from "@monoptah/protocol";
import type { SendGuard, SendGuardResult, SessionCapabilityPredicate } from "@/ui/hooks";
import { COST_NOTICES, STARTING_REFUSAL, UNAVAILABLE_REFUSAL } from "../copy";
import { isBillableKind, isNonLocalKind } from "../model";
import { getLivePlacement } from "./liveState";

const allow: SendGuardResult = { kind: "allow" };

/**
 * What a send to a billable instance needs, from every state the web knows of it. The sources can disagree
 * (a catalog card a poll behind a watch frame), so the most billable-cautious answer wins:
 * `starting` is refused, `unavailable` is refused, `stopped`, `stopping` and `unknown` (also a billable
 * session with no known state at all) ask to confirm the wake cost, `available` goes through.
 */
export function decideSend(states: readonly CloudState[]): SendGuardResult {
  if (states.includes("starting")) return { kind: "refuse", reason: STARTING_REFUSAL };
  if (states.includes("unavailable")) return { kind: "refuse", reason: UNAVAILABLE_REFUSAL };
  if (states.includes("stopped")) return { kind: "confirm", notice: COST_NOTICES.wake };
  if (states.includes("stopping")) return { kind: "confirm", notice: COST_NOTICES.wakeStopping };
  if (states.includes("unknown") || states.length === 0) return { kind: "confirm", notice: COST_NOTICES.wakeUnknown };
  return allow;
}

/**
 * Sends to a cloud instance (see `decideSend`); an empty session id (the New session page) asks to confirm the
 * start cost. A session is billable when the card target or any published live source says so. Local or
 * absent targets with no live source pass.
 */
export const cloudSendGuard: SendGuard = ({ desktopId, sessionId, target }) => {
  const live = getLivePlacement(desktopId, sessionId);
  const cardTarget = target as { kind: string; state?: CloudState } | null | undefined;
  const billable = isBillableKind(cardTarget?.kind) || live.kinds.some(isBillableKind);
  if (!billable) return Promise.resolve(allow);
  if (sessionId === "") return Promise.resolve({ kind: "confirm", notice: COST_NOTICES.start });
  const states = [...live.states, ...(cardTarget?.state ? [cardTarget.state] : [])];
  return Promise.resolve(decideSend(states));
};

/** A session that is not local hides Delete, Files, Changes and Git (the desktop owns them). */
export const cloudSessionCapabilities: SessionCapabilityPredicate = (_capability, { target }) => !isNonLocalKind(target?.kind);
